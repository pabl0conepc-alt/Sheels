//! Resource monitor for the dashboard (CPU/GPU usage + temps, memory,
//! storage, network throughput). It only runs while the dashboard is open;
//! otherwise the task sleeps on a Notify and costs nothing.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::app::app;
use crate::util::{human_bytes, read_num, read_trim};

#[derive(Default)]
struct Inner {
    last_cpu: Option<(u64, u64)>,
    last_net: Option<(u64, u64, Instant)>,
    net_down_hist: VecDeque<f64>,
    net_up_hist: VecDeque<f64>,
    cpu_temp_path: Option<PathBuf>,
    gpu_kind: Option<String>,
    gpu_busy_path: Option<PathBuf>,
    gpu_temp_path: Option<PathBuf>,
    gpu_name: String,
    cpu_name: String,
    storage: Value,
    storage_at: Option<Instant>,
    total_down: u64,
    total_up: u64,
}

#[derive(Default)]
pub struct SysMon {
    inner: Mutex<Inner>,
    wake: tokio::sync::Notify,
}

fn cpu_times() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/stat").ok()?;
    let line = s.lines().next()?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 5 {
        return None;
    }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    let total: u64 = v.iter().take(8).sum();
    Some((idle, total))
}

fn meminfo() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0;
    let mut avail = 0;
    for l in s.lines() {
        let mut it = l.split_whitespace();
        match (it.next(), it.next().and_then(|v| v.parse::<u64>().ok())) {
            (Some("MemTotal:"), Some(v)) => total = v * 1024,
            (Some("MemAvailable:"), Some(v)) => avail = v * 1024,
            _ => {}
        }
    }
    (total.saturating_sub(avail), total)
}

fn net_bytes() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/net/dev").unwrap_or_default();
    let (mut rx, mut tx) = (0u64, 0u64);
    for l in s.lines().skip(2) {
        let Some((name, rest)) = l.split_once(':') else { continue };
        let name = name.trim();
        if name == "lo" || name.starts_with("veth") || name.starts_with("docker") || name.starts_with("br-") || name.starts_with("virbr") {
            continue;
        }
        let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() >= 9 {
            rx += v[0];
            tx += v[8];
        }
    }
    (rx, tx)
}

fn find_cpu_temp() -> Option<PathBuf> {
    let wanted = ["k10temp", "coretemp", "zenpower", "cpu_thermal", "acpitz"];
    let mut found: Vec<(usize, PathBuf)> = vec![];
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let name = read_trim(e.path().join("name")).unwrap_or_default();
        if let Some(rank) = wanted.iter().position(|w| *w == name) {
            let p = e.path().join("temp1_input");
            if p.exists() {
                found.push((rank, p));
            }
        }
    }
    found.sort();
    found.into_iter().next().map(|x| x.1).or_else(|| {
        for e in std::fs::read_dir("/sys/class/thermal").ok()?.flatten() {
            if read_trim(e.path().join("type")).map(|t| t.contains("pkg") || t.contains("cpu")).unwrap_or(false) {
                return Some(e.path().join("temp"));
            }
        }
        None
    })
}

fn detect_gpu(pref: &str) -> (Option<String>, Option<PathBuf>, Option<PathBuf>, String) {
    if pref == "none" {
        return (None, None, None, String::new());
    }
    // AMD (and recent Intel xe) expose gpu_busy_percent in sysfs: free to read.
    if pref == "auto" || pref == "amd" || pref == "intel" {
        if let Ok(rd) = std::fs::read_dir("/sys/class/drm") {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if !n.starts_with("card") || n.contains('-') {
                    continue;
                }
                let dev = e.path().join("device");
                let busy = dev.join("gpu_busy_percent");
                if busy.exists() {
                    let temp = std::fs::read_dir(dev.join("hwmon"))
                        .ok()
                        .and_then(|mut r| r.next())
                        .and_then(|h| h.ok())
                        .map(|h| h.path().join("temp1_input"));
                    let name = read_trim(dev.join("product_name")).unwrap_or_else(|| "GPU".into());
                    return (Some("amd".into()), Some(busy), temp, name);
                }
            }
        }
    }
    if (pref == "auto" || pref == "nvidia") && crate::util::which("nvidia-smi") {
        return (Some("nvidia".into()), None, None, "NVIDIA GPU".into());
    }
    (None, None, None, String::new())
}

fn cpu_name() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split_once(':')).map(|x| x.1.trim().to_string()))
        .map(|n| n.replace("(R)", "").replace("(TM)", "").replace(" CPU", "").split('@').next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| "CPU".into())
}

fn storage() -> Value {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let fs_ok = ["ext4", "ext3", "btrfs", "xfs", "f2fs", "zfs", "bcachefs", "ntfs3", "exfat", "vfat"];
    let mut seen_dev = std::collections::HashSet::new();
    let mut out = vec![];
    for l in mounts.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || !fs_ok.contains(&f[2]) {
            continue;
        }
        let (dev, mnt) = (f[0], f[1].replace("\\040", " "));
        if mnt.starts_with("/boot") || mnt.starts_with("/nix/store") || mnt.starts_with("/efi") {
            continue;
        }
        // ZFS: group per pool; others per device.
        let key = if f[2] == "zfs" { dev.split('/').next().unwrap_or(dev).to_string() } else { dev.to_string() };
        if !seen_dev.insert(key.clone()) {
            continue;
        }
        let c = std::ffi::CString::new(mnt.clone()).unwrap_or_default();
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            continue;
        }
        let total = st.f_blocks as f64 * st.f_frsize as f64;
        let free = st.f_bavail as f64 * st.f_frsize as f64;
        if total < 1e9 {
            continue;
        }
        let used = total - free;
        out.push(json!({
            "mount": mnt, "name": if mnt == "/" { "Root".to_string() } else { mnt.rsplit('/').next().unwrap_or(&mnt).to_string() },
            "used": human_bytes(used, true), "total": human_bytes(total, true), "pct": (used / total * 100.0).round(),
        }));
        if out.len() >= 4 {
            break;
        }
    }
    Value::Array(out)
}

impl SysMon {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    async fn sample(&self) {
        let (cpu_pct, cpu_temp, mem, net, gpu_kind, gpu_busy, gpu_temp) = {
            let mut i = self.inner.lock().unwrap();
            let cpu_pct = match (cpu_times(), i.last_cpu) {
                (Some((idle, total)), Some((pi, pt))) if total > pt => {
                    let d = (total - pt) as f64;
                    let r = 1.0 - (idle.saturating_sub(pi)) as f64 / d;
                    i.last_cpu = Some((idle, total));
                    r
                }
                (Some(c), _) => {
                    i.last_cpu = Some(c);
                    0.0
                }
                _ => 0.0,
            };
            let cpu_temp = i.cpu_temp_path.as_ref().and_then(|p| read_num::<f64>(p)).map(|t| t / 1000.0);
            let mem = meminfo();
            let (rx, tx) = net_bytes();
            let now = Instant::now();
            let (down, up) = match i.last_net {
                Some((prx, ptx, t)) => {
                    let dt = now.duration_since(t).as_secs_f64().max(0.001);
                    (rx.saturating_sub(prx) as f64 / dt, tx.saturating_sub(ptx) as f64 / dt)
                }
                None => (0.0, 0.0),
            };
            if let Some((prx, ptx, _)) = i.last_net {
                i.total_down += rx.saturating_sub(prx);
                i.total_up += tx.saturating_sub(ptx);
            }
            i.last_net = Some((rx, tx, now));
            i.net_down_hist.push_back(down);
            i.net_up_hist.push_back(up);
            while i.net_down_hist.len() > 30 {
                i.net_down_hist.pop_front();
                i.net_up_hist.pop_front();
            }
            if i.storage_at.map(|t| t.elapsed() > Duration::from_secs(10)).unwrap_or(true) {
                i.storage = storage();
                i.storage_at = Some(now);
            }
            let gpu_busy = i.gpu_busy_path.as_ref().and_then(|p| read_num::<f64>(p)).map(|v| v / 100.0);
            let gpu_temp = i.gpu_temp_path.as_ref().and_then(|p| read_num::<f64>(p)).map(|t| t / 1000.0);
            (cpu_pct, cpu_temp, mem, (down, up), i.gpu_kind.clone(), gpu_busy, gpu_temp)
        };
        let (gpu_busy, gpu_temp) = if gpu_kind.as_deref() == Some("nvidia") {
            let out = crate::util::run(
                &["nvidia-smi", "--query-gpu=utilization.gpu,temperature.gpu", "--format=csv,noheader,nounits"],
                1500,
            )
            .await
            .unwrap_or_default();
            let v: Vec<f64> = out.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.first().map(|x| x / 100.0), v.get(1).copied())
        } else {
            (gpu_busy, gpu_temp)
        };
        let fahrenheit = crate::app::cfg().services.sensor_units == "fahrenheit";
        let temp = |t: Option<f64>| -> Value {
            match t {
                Some(c) if fahrenheit => json!(format!("{:.0}°F", c * 9.0 / 5.0 + 32.0)),
                Some(c) => json!(format!("{c:.0}°C")),
                None => json!(""),
            }
        };
        let temp_frac = |t: Option<f64>| t.map(|c| (c / 100.0).clamp(0.0, 1.0)).unwrap_or(0.0);
        let i = self.inner.lock().unwrap();
        let maxn = i.net_down_hist.iter().chain(i.net_up_hist.iter()).cloned().fold(1024.0, f64::max);
        let v = json!({
            "cpu": (cpu_pct * 100.0).round(),
            "cpuName": i.cpu_name,
            "cpuTemp": temp(cpu_temp),
            "cpuTempPct": (temp_frac(cpu_temp) * 100.0).round(),
            "gpu": gpu_busy.map(|g| (g * 100.0).round()),
            "gpuName": i.gpu_name,
            "gpuTemp": temp(gpu_temp),
            "gpuTempPct": (temp_frac(gpu_temp) * 100.0).round(),
            "hasGpu": gpu_kind.is_some(),
            "memUsed": human_bytes(mem.0 as f64, true),
            "memTotal": human_bytes(mem.1 as f64, true),
            "mem": if mem.1 > 0 { (mem.0 as f64 / mem.1 as f64 * 100.0).round() } else { 0.0 },
            "storage": i.storage,
            "down": format!("{}/s", human_bytes(net.0, true)),
            "up": format!("{}/s", human_bytes(net.1, true)),
            "totalDown": human_bytes(i.total_down as f64, true),
            "totalUp": human_bytes(i.total_up as f64, true),
            "downHist": i.net_down_hist.iter().map(|x| (x / maxn * 100.0).round()).collect::<Vec<_>>(),
            "upHist": i.net_up_hist.iter().map(|x| (x / maxn * 100.0).round()).collect::<Vec<_>>(),
            "uptime": uptime(),
        });
        drop(i);
        app().hub.publish("sys", &v);
    }
}

pub fn uptime() -> String {
    let secs = read_trim("/proc/uptime").and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok())).unwrap_or(0.0)
        as u64;
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    let mut parts = vec![];
    if d > 0 {
        parts.push(format!("{d} day{}", if d == 1 { "" } else { "s" }));
    }
    if h > 0 {
        parts.push(format!("{h} hour{}", if h == 1 { "" } else { "s" }));
    }
    if d == 0 {
        parts.push(format!("{m} minute{}", if m == 1 { "" } else { "s" }));
    }
    format!("up {}", parts.join(", "))
}

pub async fn run() {
    let sm = &app().sys;
    {
        let pref = crate::app::cfg().services.gpu_type.clone();
        let (kind, busy, temp, name) = detect_gpu(&pref);
        let mut i = sm.inner.lock().unwrap();
        i.cpu_temp_path = find_cpu_temp();
        i.gpu_kind = kind;
        i.gpu_busy_path = busy;
        i.gpu_temp_path = temp;
        i.gpu_name = name;
        i.cpu_name = cpu_name();
    }
    loop {
        if !app().hub.demand("sys") {
            sm.wake.notified().await;
            continue;
        }
        sm.sample().await;
        let ms = crate::app::cfg().dashboard.resource_update_interval.max(250);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {},
            _ = sm.wake.notified() => {},
        }
    }
}

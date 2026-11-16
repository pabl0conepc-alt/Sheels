//! Backlight brightness. Changes are detected with poll(POLLPRI) on
//! `actual_brightness` (the kernel calls sysfs_notify on it), so there is no
//! polling loop at all. Writes go through brightnessctl (logind, no root).

use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::app::app;
use crate::util::{read_num, run_ok};

#[derive(Default)]
struct Inner {
    dev: Option<PathBuf>,
    cur: f64,
    max: f64,
}

#[derive(Default)]
pub struct Brightness {
    inner: Mutex<Inner>,
}

fn find_device() -> Option<PathBuf> {
    let mut best: Option<(PathBuf, u64)> = None;
    for e in std::fs::read_dir("/sys/class/backlight").ok()?.flatten() {
        let p = e.path();
        let max: u64 = read_num(p.join("max_brightness")).unwrap_or(0);
        // Prefer firmware/platform interfaces with the finest granularity.
        if best.as_ref().map(|b| max > b.1).unwrap_or(true) {
            best = Some((p, max));
        }
    }
    best.map(|b| b.0)
}

impl Brightness {
    pub fn json(&self) -> Value {
        let i = self.inner.lock().unwrap();
        let pct = if i.max > 0.0 { (i.cur / i.max * 100.0).round() } else { 0.0 };
        json!({
            "available": i.dev.is_some(),
            "pct": pct,
            "icon": if pct >= 66.0 { "brightness_high" } else if pct >= 33.0 { "brightness_medium" } else { "brightness_low" },
        })
    }

    fn read(&self) -> bool {
        let mut i = self.inner.lock().unwrap();
        let Some(dev) = i.dev.clone() else { return false };
        let cur: f64 = read_num(dev.join("actual_brightness")).or_else(|| read_num(dev.join("brightness"))).unwrap_or(0.0);
        let max: f64 = read_num(dev.join("max_brightness")).unwrap_or(0.0);
        let changed = cur != i.cur || max != i.max;
        i.cur = cur;
        i.max = max;
        changed
    }

    pub async fn set(&self, frac: f64) {
        let pct = (frac.clamp(0.0, 1.0) * 100.0).round().max(1.0);
        let dev = self.inner.lock().unwrap().dev.clone();
        let name = dev.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned());
        let p = format!("{pct}%");
        let ok = match &name {
            Some(n) => run_ok(&["brightnessctl", "-q", "-d", n, "set", &p], 2000).await,
            None => false,
        };
        if !ok {
            // External monitors (DDC/CI) as a fallback, like Caelestia.
            run_ok(&["ddcutil", "setvcp", "10", &format!("{pct}")], 5000).await;
        }
        if self.read() {
            app().publish_status();
            app().ui.osd().await;
        }
    }

    pub async fn step(&self, dir: f64) {
        let inc = crate::app::cfg().services.brightness_increment;
        let cur = {
            let i = self.inner.lock().unwrap();
            if i.max > 0.0 { i.cur / i.max } else { 0.5 }
        };
        self.set(cur + dir * inc).await;
    }
}

pub async fn run() {
    let b = &app().bright;
    let dev = find_device();
    b.inner.lock().unwrap().dev = dev.clone();
    b.read();
    app().publish_status();
    let Some(dev) = dev else { return };
    // Blocking poll() on a dedicated thread: it sleeps in the kernel until the
    // backlight driver notifies a change.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    std::thread::Builder::new()
        .name("sheels-backlight".into())
        .spawn(move || {
            let path = dev.join("actual_brightness");
            loop {
                let Ok(mut f) = std::fs::File::open(&path) else { return };
                use std::io::{Read, Seek};
                let mut buf = [0u8; 32];
                let _ = f.read(&mut buf);
                loop {
                    let mut pfd = libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLPRI | libc::POLLERR, revents: 0 };
                    let r = unsafe { libc::poll(&mut pfd, 1, -1) };
                    if r < 0 {
                        break;
                    }
                    let _ = f.seek(std::io::SeekFrom::Start(0));
                    let _ = f.read(&mut buf);
                    if tx.send(()).is_err() {
                        return;
                    }
                    // Safety valve against drivers that report POLLPRI
                    // continuously: never wake more than ~60 times a second.
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
            }
        })
        .ok();
    let mut first = true;
    while rx.recv().await.is_some() {
        if b.read() {
            app().publish_status();
            if !first {
                app().ui.osd().await;
            }
        }
        first = false;
    }
}

//! Battery / AC from sysfs, plus Caelestia's low-battery warnings and power
//! profiles (power-profiles-daemon). Also tracks Caps/Num Lock LEDs for the
//! bar's lock-status icon.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app::app;
use crate::util::{read_num, read_trim};

#[derive(Default, Clone, PartialEq)]
struct Snapshot {
    present: bool,
    pct: f64,
    charging: bool,
    full: bool,
    on_ac: bool,
    secs_left: Option<u64>,
    power_w: f64,
    health: Option<f64>,
}

#[derive(Default)]
struct Inner {
    bats: Vec<PathBuf>,
    acs: Vec<PathBuf>,
    snap: Snapshot,
    warned: Vec<u8>,
    profile: String,
    profiles: Vec<String>,
    caps: bool,
    num: bool,
    leds: Vec<(PathBuf, bool)>, // path, is_caps
    read_once: bool,
    leds_once: bool,
}

#[derive(Default)]
pub struct Battery {
    inner: Mutex<Inner>,
}

/// Same mapping as Caelestia's `Icons.getBatteryIcon`.
fn battery_icon(pct: f64, charging: bool) -> String {
    let p = pct / 100.0;
    if p >= 0.995 {
        return if charging { "battery_charging_full".into() } else { "battery_full".into() };
    }
    let mut level = (p * 7.0).floor().clamp(0.0, 6.0) as i32;
    if charging && (level == 4 || level == 1) {
        level -= 1;
    }
    if charging { format!("battery_charging_{}", (level + 3) * 10) } else { format!("battery_{level}_bar") }
}

fn fmt_time(s: u64) -> String {
    let h = s / 3600;
    let m = (s % 3600) / 60;
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

impl Battery {
    pub fn json(&self) -> Value {
        let i = self.inner.lock().unwrap();
        let s = &i.snap;
        json!({
            "present": s.present,
            "pct": s.pct.round() as i64,
            "charging": s.charging,
            "full": s.full,
            "onAc": s.on_ac,
            "icon": battery_icon(s.pct, s.charging),
            "time": s.secs_left.map(fmt_time).unwrap_or_default(),
            "power": (s.power_w * 10.0).round() / 10.0,
            "health": s.health.map(|h| h.round()),
            "profile": i.profile,
            "profiles": i.profiles,
            "caps": i.caps,
            "num": i.num,
        })
    }

    pub fn lock_keys(&self) -> (bool, bool) {
        let i = self.inner.lock().unwrap();
        (i.caps, i.num)
    }

    fn scan(&self) {
        let mut i = self.inner.lock().unwrap();
        i.bats.clear();
        i.acs.clear();
        if let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") {
            for e in rd.flatten() {
                let p = e.path();
                let ty = read_trim(p.join("type")).unwrap_or_default();
                let scope = read_trim(p.join("scope")).unwrap_or_default();
                if ty == "Battery" && scope != "Device" {
                    i.bats.push(p);
                } else if ty == "Mains" || ty == "USB" {
                    i.acs.push(p);
                }
            }
        }
        i.leds.clear();
        if let Ok(rd) = std::fs::read_dir("/sys/class/leds") {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with("::capslock") {
                    i.leds.push((e.path().join("brightness"), true));
                } else if n.ends_with("::numlock") {
                    i.leds.push((e.path().join("brightness"), false));
                }
            }
        }
    }

    /// Returns true when something user-visible changed.
    fn read(&self) -> bool {
        let mut i = self.inner.lock().unwrap();
        let mut s = Snapshot::default();
        let (mut e_now, mut e_full, mut e_design, mut power) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut statuses = vec![];
        for b in &i.bats {
            s.present = true;
            let get = |n: &str| read_num::<f64>(b.join(n));
            // energy_* (µWh) or charge_* (µAh) depending on the driver.
            let now = get("energy_now").or_else(|| get("charge_now"));
            let full = get("energy_full").or_else(|| get("charge_full"));
            let design = get("energy_full_design").or_else(|| get("charge_full_design"));
            let pw = get("power_now").or_else(|| {
                let c = get("current_now")?;
                let v = get("voltage_now")?;
                Some(c * v / 1e6)
            });
            match (now, full) {
                (Some(n), Some(f)) if f > 0.0 => {
                    e_now += n;
                    e_full += f;
                }
                _ => {
                    if let Some(c) = get("capacity") {
                        e_now += c;
                        e_full += 100.0;
                    }
                }
            }
            e_design += design.unwrap_or(0.0);
            power += pw.unwrap_or(0.0).abs();
            statuses.push(read_trim(b.join("status")).unwrap_or_default());
        }
        if s.present && e_full > 0.0 {
            s.pct = (e_now / e_full * 100.0).clamp(0.0, 100.0);
            s.charging = statuses.iter().any(|x| x == "Charging");
            s.full = statuses.iter().all(|x| x == "Full" || x == "Not charging") && s.pct > 95.0;
            s.power_w = power / 1e6;
            if power > 0.0 {
                let hours = if s.charging { (e_full - e_now) / power } else { e_now / power };
                if hours.is_finite() && hours > 0.0 && hours < 48.0 {
                    s.secs_left = Some((hours * 3600.0) as u64);
                }
            }
            if e_design > 0.0 {
                s.health = Some((e_full / e_design * 100.0).min(100.0));
            }
        }
        s.on_ac = i.acs.iter().any(|a| read_num::<u8>(a.join("online")).unwrap_or(0) == 1);
        // Round so tiny power fluctuations do not cause republishing.
        s.power_w = (s.power_w * 10.0).round() / 10.0;
        s.secs_left = s.secs_left.map(|x| x / 60 * 60);
        let changed = s != i.snap;
        let was_charging = i.snap.charging || i.snap.on_ac;
        if i.read_once && s.present && (s.charging || s.on_ac) != was_charging {
            let plugged = s.charging || s.on_ac;
            let t = crate::app::cfg().utilities.toasts.charging_changed;
            app().notifs.toast(
                t,
                if plugged { "Charger plugged in" } else { "Charger unplugged" },
                &format!("Battery at {:.0}%", s.pct),
                if plugged { "power" } else { "power_off" },
            );
        }
        i.read_once = true;
        i.snap = s.clone();
        if (s.charging || s.on_ac) && !was_charging {
            i.warned.clear();
        }
        changed
    }

    fn read_leds(&self) -> bool {
        let mut i = self.inner.lock().unwrap();
        let (mut caps, mut num) = (false, false);
        for (p, is_caps) in &i.leds {
            let on = read_num::<u32>(p).unwrap_or(0) > 0;
            if *is_caps {
                caps |= on;
            } else {
                num |= on;
            }
        }
        let changed = caps != i.caps || num != i.num;
        if i.leds_once {
            let t = &crate::app::cfg().utilities.toasts;
            if caps != i.caps {
                app().notifs.toast(t.caps_lock_changed, if caps { "Caps lock on" } else { "Caps lock off" }, "", "keyboard_capslock_badge");
            }
            if num != i.num {
                app().notifs.toast(t.num_lock_changed, if num { "Num lock on" } else { "Num lock off" }, "", "looks_one");
            }
        }
        i.leds_once = true;
        i.caps = caps;
        i.num = num;
        changed
    }

    fn check_warnings(&self) {
        let cfg = crate::app::cfg();
        let levels = cfg.general.battery_warn_levels.clone();
        let critical = cfg.general.battery_critical;
        drop(cfg);
        let mut i = self.inner.lock().unwrap();
        if !i.snap.present || i.snap.charging || i.snap.on_ac {
            return;
        }
        let pct = i.snap.pct;
        for lvl in levels {
            if pct <= lvl as f64 && !i.warned.contains(&lvl) {
                i.warned.push(lvl);
                let (icon, title, crit) = match lvl {
                    l if l <= 5 => ("battery_android_alert", "Critical battery", true),
                    l if l <= 10 => ("battery_android_frame_1", "Very low battery", false),
                    _ => ("battery_android_frame_2", "Low battery", false),
                };
                let body = format!("{}% remaining. Connect the charger.", pct.round());
                drop(i);
                app().notifs.internal(title, &body, icon, crit);
                return;
            }
        }
        if pct <= critical as f64 && !i.warned.contains(&0) {
            i.warned.push(0);
            drop(i);
            crate::util::spawn_argv(&["systemctl", "hibernate"]);
        }
    }

    pub async fn refresh_profile(&self) {
        if !crate::util::which("powerprofilesctl") {
            return;
        }
        let cur = crate::util::run(&["powerprofilesctl", "get"], 2000).await.unwrap_or_default();
        let list = crate::util::run(&["powerprofilesctl", "list"], 2000).await.unwrap_or_default();
        let profiles: Vec<String> = list
            .lines()
            .filter(|l| l.trim_end().ends_with(':'))
            .map(|l| l.trim().trim_start_matches('*').trim().trim_end_matches(':').to_string())
            .collect();
        {
            let mut i = self.inner.lock().unwrap();
            i.profile = cur.trim().to_string();
            i.profiles = profiles;
        }
        app().publish_status();
    }
}

pub async fn run() {
    let b = &app().bat;
    b.scan();
    b.read();
    b.read_leds();
    app().publish_status();
    b.refresh_profile().await;
    let mut tick: u64 = 0;
    loop {
        // LEDs every second (two tiny reads), battery every 15 s.
        tokio::time::sleep(Duration::from_secs(1)).await;
        tick += 1;
        let mut changed = b.read_leds();
        if tick.is_multiple_of(15) {
            if tick.is_multiple_of(300) {
                b.scan();
            }
            changed |= b.read();
            b.check_warnings();
        }
        if changed {
            app().publish_status();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn battery_icons_match_caelestia() {
        assert_eq!(super::battery_icon(100.0, false), "battery_full");
        assert_eq!(super::battery_icon(100.0, true), "battery_charging_full");
        assert_eq!(super::battery_icon(50.0, false), "battery_3_bar");
        assert_eq!(super::battery_icon(5.0, false), "battery_0_bar");
        // Caelestia skips some charging levels (4 → 3, 1 → 0).
        assert_eq!(super::battery_icon(60.0, true), "battery_charging_60");
        assert_eq!(super::battery_icon(20.0, true), "battery_charging_30");
    }
}

//! Slow-changing info for the dashboard/lock: user, host, distro, uptime and
//! weather (Open-Meteo, refreshed every 30 minutes).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::app::app;

#[derive(Default)]
struct Inner {
    cal_offset: i32,
    weather: Value,
    weather_at: Option<Instant>,
    /// Last fetch attempt (failed ones are retried after 5 min, not on every wake).
    tried_at: Option<Instant>,
    loc: Option<(f64, f64, String)>,
}

#[derive(Default)]
pub struct Info {
    inner: Mutex<Inner>,
    wake: tokio::sync::Notify,
}

pub fn weather_icon(code: i64, day: bool) -> &'static str {
    match code {
        0 | 1 => {
            if day {
                "clear_day"
            } else {
                "clear_night"
            }
        }
        2 => {
            if day {
                "partly_cloudy_day"
            } else {
                "partly_cloudy_night"
            }
        }
        3 => "cloud",
        45 | 48 => "foggy",
        51..=67 | 80..=82 => "rainy",
        71 | 73 | 77 | 85 => "cloudy_snowing",
        75 | 86 => "snowing_heavy",
        95..=99 => "thunderstorm",
        _ => "air",
    }
}

fn weather_desc(code: i64) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 | 73 => "Snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80 | 81 => "Rain showers",
        82 => "Violent showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

fn os_release() -> (String, String, String) {
    let s = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let get = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(&format!("{k}=")))
            .map(|l| l[k.len() + 1..].trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let name = if get("PRETTY_NAME").is_empty() { get("NAME") } else { get("PRETTY_NAME") };
    (name, get("ID"), get("LOGO"))
}

fn hostname() -> String {
    crate::util::read_trim("/proc/sys/kernel/hostname").unwrap_or_default()
}

fn username() -> String {
    std::env::var("USER").unwrap_or_else(|_| "user".into())
}

impl Info {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Location/units changed: forget the cached place and fetch again now.
    pub fn refresh_weather(&self) {
        {
            let mut i = self.inner.lock().unwrap();
            i.loc = None;
            i.weather_at = None;
            i.tried_at = None;
        }
        self.wake.notify_one();
    }

    /// Month grid (Monday first, 6 weeks) like Caelestia's dashboard calendar.
    fn calendar(&self) -> Value {
        let off = self.inner.lock().unwrap().cal_offset;
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&now, &mut tm) };
        let (ty, tmon, tday) = (tm.tm_year, tm.tm_mon, tm.tm_mday);
        // First day of the displayed month.
        let mut first: libc::tm = unsafe { std::mem::zeroed() };
        first.tm_year = ty;
        first.tm_mon = tmon + off;
        first.tm_mday = 1;
        first.tm_hour = 12;
        unsafe { libc::mktime(&mut first) };
        let (y, m) = (first.tm_year, first.tm_mon);
        let lead = (first.tm_wday + 6) % 7; // Monday = 0
        const MONTHS: [&str; 12] =
            ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let mut weeks = vec![];
        for w in 0..6 {
            let mut days = vec![];
            for d in 0..7 {
                let mut t: libc::tm = unsafe { std::mem::zeroed() };
                t.tm_year = y;
                t.tm_mon = m;
                t.tm_mday = 1 + w * 7 + d - lead;
                t.tm_hour = 12;
                unsafe { libc::mktime(&mut t) };
                days.push(json!({
                    "d": t.tm_mday,
                    "cur": t.tm_mon == m,
                    "today": t.tm_year == ty && t.tm_mon == tmon && t.tm_mday == tday,
                    "weekend": d >= 5,
                }));
            }
            weeks.push(Value::Array(days));
        }
        json!({ "title": format!("{} {}", MONTHS[m.clamp(0, 11) as usize], y + 1900), "weeks": weeks, "offset": off })
    }

    pub fn cal_cmd(&self, sub: &str) {
        {
            let mut i = self.inner.lock().unwrap();
            match sub {
                "next" => i.cal_offset += 1,
                "prev" => i.cal_offset -= 1,
                _ => i.cal_offset = 0,
            }
        }
        self.publish();
    }

    fn publish(&self) {
        let (distro, id, logo) = os_release();
        let face = crate::util::expand(&crate::app::cfg().paths.face);
        let mut w = self.inner.lock().unwrap().weather.clone();
        if !w.is_object() {
            w = json!({"available": false, "temp": "--°C", "icon": "cloud_alert", "desc": "No weather", "city": "",
                       "feels": "--", "humidity": "--", "wind": "--", "precip": "--", "sunrise": "--", "sunset": "--",
                       "days": [], "hours": []});
        }
        let v = json!({
            "user": username(),
            "host": hostname(),
            "distro": distro,
            "distroId": id,
            "distroLogo": logo,
            "face": if face.exists() { face.to_string_lossy().into_owned() } else { String::new() },
            "uptime": crate::sysmon::uptime(),
            "kernel": crate::util::read_trim("/proc/sys/kernel/osrelease").unwrap_or_default(),
            "wm": "driftwm",
            "weather": w,
            "cal": self.calendar(),
        });
        app().hub.publish("info", &v);
    }

    async fn locate(&self) -> Option<(f64, f64, String)> {
        if let Some(l) = self.inner.lock().unwrap().loc.clone() {
            return Some(l);
        }
        let cfg_loc = crate::app::cfg().services.weather_location.clone();
        let loc = if let Some((a, b)) = cfg_loc.split_once(',').and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
        {
            Some((a, b, String::new()))
        } else if !cfg_loc.is_empty() {
            let url = format!(
                "https://geocoding-api.open-meteo.com/v1/search?count=1&name={}",
                cfg_loc.replace(' ', "+")
            );
            let s = crate::util::run(&["curl", "-sf", "--max-time", "10", &url], 12000).await?;
            let v: Value = serde_json::from_str(&s).ok()?;
            let r = &v["results"][0];
            Some((r["latitude"].as_f64()?, r["longitude"].as_f64()?, r["name"].as_str().unwrap_or("").to_string()))
        } else {
            let s = crate::util::run(&["curl", "-sf", "--max-time", "10", "https://ipinfo.io/json"], 12000).await?;
            let v: Value = serde_json::from_str(&s).ok()?;
            let (a, b) = v["loc"].as_str()?.split_once(',')?;
            Some((a.parse().ok()?, b.parse().ok()?, v["city"].as_str().unwrap_or("").to_string()))
        };
        self.inner.lock().unwrap().loc = loc.clone();
        loc
    }

    async fn fetch_weather(&self) {
        let Some((lat, lon, city)) = self.locate().await else { return };
        let units = crate::app::cfg().services.weather_units.clone();
        let fahrenheit = units == "fahrenheit"
            || (units == "auto" && std::env::var("LANG").map(|l| l.contains("en_US")).unwrap_or(false));
        let url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
             &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day,precipitation_probability\
             &daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset\
             &hourly=temperature_2m,weather_code&forecast_days=5&timezone=auto{}",
            if fahrenheit { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" }
        );
        let Some(s) = crate::util::run(&["curl", "-sf", "--max-time", "15", &url], 17000).await else { return };
        let Ok(v) = serde_json::from_str::<Value>(&s) else { return };
        let c = &v["current"];
        let unit = if fahrenheit { "°F" } else { "°C" };
        let code = c["weather_code"].as_i64().unwrap_or(-1);
        let day = c["is_day"].as_i64().unwrap_or(1) == 1;
        let d = &v["daily"];
        let days: Vec<Value> = (0..5)
            .filter_map(|k| {
                let date = d["time"][k].as_str()?;
                let wc = d["weather_code"][k].as_i64().unwrap_or(-1);
                Some(json!({
                    "day": weekday(date),
                    "icon": weather_icon(wc, true),
                    "max": format!("{:.0}{unit}", d["temperature_2m_max"][k].as_f64()?),
                    "min": format!("{:.0}{unit}", d["temperature_2m_min"][k].as_f64()?),
                }))
            })
            .collect();
        // Next hours from "now".
        let h = &v["hourly"];
        let now_iso = c["time"].as_str().unwrap_or("").get(..13).unwrap_or("").to_string();
        let start = h["time"].as_array().and_then(|a| a.iter().position(|t| t.as_str().map(|t| t.starts_with(&now_iso)).unwrap_or(false))).unwrap_or(0);
        let hours: Vec<Value> = (start..start + 6)
            .filter_map(|k| {
                let t = h["time"][k].as_str()?;
                Some(json!({
                    "hour": t.get(11..16).unwrap_or(""),
                    "icon": weather_icon(h["weather_code"][k].as_i64().unwrap_or(-1), true),
                    "temp": format!("{:.0}{unit}", h["temperature_2m"][k].as_f64()?),
                }))
            })
            .collect();
        let w = json!({
            "available": true,
            "city": city,
            "temp": format!("{:.0}{unit}", c["temperature_2m"].as_f64().unwrap_or(0.0)),
            "feels": format!("{:.0}{unit}", c["apparent_temperature"].as_f64().unwrap_or(0.0)),
            "humidity": format!("{:.0}%", c["relative_humidity_2m"].as_f64().unwrap_or(0.0)),
            "wind": format!("{:.0} {}", c["wind_speed_10m"].as_f64().unwrap_or(0.0), if fahrenheit { "mph" } else { "km/h" }),
            "precip": format!("{:.0}%", c["precipitation_probability"].as_f64().unwrap_or(0.0)),
            "icon": weather_icon(code, day),
            "desc": weather_desc(code),
            "sunrise": d["sunrise"][0].as_str().and_then(|s| s.get(11..16)).unwrap_or(""),
            "sunset": d["sunset"][0].as_str().and_then(|s| s.get(11..16)).unwrap_or(""),
            "days": days,
            "hours": hours,
        });
        {
            let mut i = self.inner.lock().unwrap();
            i.weather = w;
            i.weather_at = Some(Instant::now());
        }
        // Cache for the lock screen / next start.
        let _ = crate::util::write_atomic(
            &crate::util::state_dir().join("weather.json"),
            self.inner.lock().unwrap().weather.to_string().as_bytes(),
        );
        self.publish();
    }
}

fn weekday(date: &str) -> String {
    // Zeller-free: use libc mktime on the date.
    let p: Vec<i32> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 3 {
        return String::new();
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = p[0] - 1900;
    tm.tm_mon = p[1] - 1;
    tm.tm_mday = p[2];
    tm.tm_hour = 12;
    unsafe { libc::mktime(&mut tm) };
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][tm.tm_wday.clamp(0, 6) as usize].to_string()
}

pub async fn run() {
    let info = &app().info;
    if let Ok(s) = std::fs::read_to_string(crate::util::state_dir().join("weather.json"))
        && let Ok(v) = serde_json::from_str::<Value>(&s) {
            info.inner.lock().unwrap().weather = v;
        }
    info.publish();
    loop {
        let stale = {
            let i = info.inner.lock().unwrap();
            i.weather_at.map(|t| t.elapsed() > Duration::from_secs(1800)).unwrap_or(true)
                && i.tried_at.map(|t| t.elapsed() > Duration::from_secs(300)).unwrap_or(true)
        };
        if stale {
            info.inner.lock().unwrap().tried_at = Some(Instant::now());
            info.fetch_weather().await;
        }
        // Uptime is refreshed while the dashboard is visible (once a minute).
        if app().hub.demand("info") {
            info.publish();
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(if app().hub.demand("info") { 60 } else { 1800 })) => {},
            _ = info.wake.notified() => {},
        }
    }
}

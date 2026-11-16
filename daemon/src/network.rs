//! NetworkManager over D-Bus. We listen to NM's signals (debounced) and only
//! re-read the handful of properties the shell shows. Access-point strength
//! signals are ignored except for the AP we are connected to, so a busy
//! neighbourhood does not wake us up.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::app::app;
use crate::dbus::{get, get_all, s, signals_from};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";

#[derive(Default, Clone, PartialEq)]
struct Snapshot {
    available: bool,
    wifi_enabled: bool,
    kind: String, // wifi | ethernet | none
    name: String,
    strength: i64,
    vpn: Option<String>,
    ap_path: String,
    wifi_dev: String,
}

#[derive(Default)]
struct Inner {
    snap: Snapshot,
    networks: Vec<Value>,
    list_demand: bool,
    scanning: bool,
}

#[derive(Default)]
pub struct Network {
    inner: Mutex<Inner>,
    kick: tokio::sync::Notify,
}

pub fn wifi_icon(strength: i64, secure: bool) -> String {
    const ICONS: [&str; 5] = ["signal_wifi_0_bar", "network_wifi_1_bar", "network_wifi_2_bar", "network_wifi_3_bar", "network_wifi"];
    let level = (strength / 20).clamp(0, 4) as usize;
    if secure && level > 0 { format!("{}_locked", ICONS[level]) } else { ICONS[level].to_string() }
}

impl Network {
    pub fn json(&self) -> Value {
        let i = self.inner.lock().unwrap();
        let s = &i.snap;
        let icon = match s.kind.as_str() {
            "ethernet" => "cable".to_string(),
            "wifi" => wifi_icon(s.strength, false),
            _ => "wifi_off".to_string(),
        };
        json!({
            "available": s.available,
            "wifiEnabled": s.wifi_enabled,
            "type": s.kind,
            "connected": s.kind != "none" && !s.kind.is_empty(),
            "name": s.name,
            "strength": s.strength,
            "icon": icon,
            "vpn": s.vpn,
            "networks": i.networks,
            "scanning": i.scanning,
        })
    }

    pub fn set_list_demand(&self, on: bool) {
        let changed = {
            let mut i = self.inner.lock().unwrap();
            let c = i.list_demand != on;
            i.list_demand = on;
            c
        };
        if changed && on {
            self.kick.notify_one();
            tokio::spawn(async { app().net.scan().await });
        }
    }

    async fn refresh(&self) {
        let Some(conn) = app().system_bus.get() else { return };
        let mut snap = Snapshot::default();
        let Ok(nm) = get_all(conn, NM, NM_PATH, NM).await else {
            self.inner.lock().unwrap().snap = snap;
            app().publish_status();
            return;
        };
        snap.available = true;
        snap.wifi_enabled = nm["WirelessEnabled"].as_bool().unwrap_or(false);
        snap.kind = "none".into();
        let primary = s(&nm["PrimaryConnection"]);
        let actives: Vec<String> = nm["ActiveConnections"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default();
        for ac in actives {
            let Ok(p) = get_all(conn, NM, &ac, "org.freedesktop.NetworkManager.Connection.Active").await else { continue };
            let ty = s(&p["Type"]);
            let id = s(&p["Id"]);
            let state = p["State"].as_u64().unwrap_or(0);
            if ty == "vpn" || ty == "wireguard" {
                if state == 2 {
                    snap.vpn = Some(id);
                }
                continue;
            }
            let is_primary = ac == primary || snap.kind == "none";
            if !is_primary || state != 2 {
                continue;
            }
            match ty.as_str() {
                "802-11-wireless" => {
                    snap.kind = "wifi".into();
                    snap.name = id;
                    let ap = s(&p["SpecificObject"]);
                    if ap.len() > 1 {
                        if let Ok(apv) = get_all(conn, NM, &ap, "org.freedesktop.NetworkManager.AccessPoint").await {
                            let ssid = s(&apv["Ssid"]);
                            if !ssid.is_empty() {
                                snap.name = ssid;
                            }
                            snap.strength = apv["Strength"].as_i64().unwrap_or(0);
                        }
                        snap.ap_path = ap;
                    }
                }
                "802-3-ethernet" => {
                    snap.kind = "ethernet".into();
                    snap.name = id;
                }
                _ => {}
            }
        }
        // Wireless device (for scans/lists).
        if let Ok(devs) = get(conn, NM, NM_PATH, NM, "Devices").await {
            for d in devs.as_array().cloned().unwrap_or_default() {
                let d = s(&d);
                if get(conn, NM, &d, "org.freedesktop.NetworkManager.Device", "DeviceType").await.ok().and_then(|v| v.as_u64())
                    == Some(2)
                {
                    snap.wifi_dev = d;
                    break;
                }
            }
        }
        let want_list = {
            let mut i = self.inner.lock().unwrap();
            i.snap = snap;
            i.list_demand
        };
        if want_list {
            self.refresh_list().await;
        }
        app().publish_status();
    }

    async fn refresh_list(&self) {
        let Some(conn) = app().system_bus.get() else { return };
        let (dev, cur) = {
            let i = self.inner.lock().unwrap();
            (i.snap.wifi_dev.clone(), i.snap.ap_path.clone())
        };
        if dev.is_empty() {
            return;
        }
        let Ok(msg) = conn
            .call_method(Some(NM), dev.as_str(), Some("org.freedesktop.NetworkManager.Device.Wireless"), "GetAllAccessPoints", &())
            .await
        else {
            return;
        };
        let body = msg.body();
        let aps: Vec<zbus::OwnedObjectPath> = body.deserialize().unwrap_or_default();
        let mut best: std::collections::HashMap<String, Value> = Default::default();
        for ap in aps {
            let Ok(v) = get_all(conn, NM, ap.as_str(), "org.freedesktop.NetworkManager.AccessPoint").await else { continue };
            let ssid = s(&v["Ssid"]);
            if ssid.is_empty() {
                continue;
            }
            let strength = v["Strength"].as_i64().unwrap_or(0);
            let secure = v["WpaFlags"].as_u64().unwrap_or(0) != 0 || v["RsnFlags"].as_u64().unwrap_or(0) != 0;
            let active = ap.as_str() == cur;
            let freq = v["Frequency"].as_u64().unwrap_or(0);
            let e = json!({
                "ssid": ssid, "strength": strength, "secure": secure, "active": active,
                "icon": wifi_icon(strength, secure), "band": if freq > 4900 { "5 GHz" } else { "2.4 GHz" }, "i": 0,
            });
            let keep = best.get(&ssid).map(|o| active || o["strength"].as_i64().unwrap_or(0) < strength).unwrap_or(true);
            if keep {
                best.insert(ssid, e);
            }
        }
        let mut list: Vec<Value> = best.into_values().collect();
        list.sort_by(|a, b| {
            b["active"].as_bool().cmp(&a["active"].as_bool()).then(b["strength"].as_i64().cmp(&a["strength"].as_i64()))
        });
        list.truncate(12);
        for (i, n) in list.iter_mut().enumerate() {
            n["i"] = json!(i);
        }
        self.inner.lock().unwrap().networks = list;
    }

    pub async fn scan(&self) {
        let Some(conn) = app().system_bus.get() else { return };
        let dev = self.inner.lock().unwrap().snap.wifi_dev.clone();
        if dev.is_empty() {
            return;
        }
        self.inner.lock().unwrap().scanning = true;
        app().publish_status();
        let opts: std::collections::HashMap<&str, zbus::Variant> = Default::default();
        let _ = conn
            .call_method(Some(NM), dev.as_str(), Some("org.freedesktop.NetworkManager.Device.Wireless"), "RequestScan", &(opts,))
            .await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        self.refresh_list().await;
        self.inner.lock().unwrap().scanning = false;
        app().publish_status();
    }

    pub async fn command(&self, sub: &str, rest: &[String]) -> Result<String> {
        let arg = |i: usize| rest.get(i).map(String::as_str).unwrap_or("");
        match sub {
            "toggle" | "on" | "off" => {
                let on = match sub {
                    "on" => true,
                    "off" => false,
                    _ => !self.inner.lock().unwrap().snap.wifi_enabled,
                };
                if let Some(conn) = app().system_bus.get()
                    && crate::dbus::set(conn, NM, NM_PATH, NM, "WirelessEnabled", zbus::Variant::from(on)).await.is_err() {
                        crate::util::run_ok(&["nmcli", "radio", "wifi", if on { "on" } else { "off" }], 5000).await;
                    }
            }
            "scan" => {
                tokio::spawn(async { app().net.scan().await });
            }
            "connect-idx" => {
                // UI path: SSIDs are attacker-controlled, so EWW passes an index.
                let ssid = {
                    let i = self.inner.lock().unwrap();
                    arg(0).parse::<usize>().ok().and_then(|k| i.networks.get(k)).and_then(|n| n["ssid"].as_str().map(str::to_string))
                };
                let Some(ssid) = ssid else { bail!("no such network") };
                let secure_unknown = !crate::util::run_ok(&["nmcli", "-t", "-f", "NAME", "connection", "show", "id", &ssid], 3000).await;
                let secure = self.inner.lock().unwrap().networks.iter().any(|n| n["ssid"] == ssid.as_str() && n["secure"] == true);
                if secure && secure_unknown {
                    app().ui.set_misc("wifiAskPassword", Value::from(ssid));
                    return Ok(String::new());
                }
                return Box::pin(self.command("connect", &[ssid])).await;
            }
            "connect" => {
                let ssid = arg(0);
                if ssid.is_empty() {
                    bail!("wifi connect <ssid> [password]");
                }
                let pw = arg(1);
                // Known network → bring the saved profile up; otherwise create.
                let ok = crate::util::run_ok(&["nmcli", "connection", "up", "id", ssid], 20000).await
                    || if pw.is_empty() {
                        crate::util::run_ok(&["nmcli", "device", "wifi", "connect", ssid], 30000).await
                    } else {
                        crate::util::run_ok(&["nmcli", "device", "wifi", "connect", ssid, "password", pw], 30000).await
                    };
                if !ok {
                    app().ui.set_misc("wifiAskPassword", Value::from(ssid));
                    bail!("could not connect to {ssid}");
                }
                app().ui.set_misc("wifiAskPassword", Value::from(""));
            }
            "password" => {
                // stdin payload: the password for the SSID awaiting one.
                let ssid = app().hub.get("ui").and_then(|u| serde_json::from_str::<Value>(&u).ok())
                    .and_then(|u| u["wifiAskPassword"].as_str().map(str::to_string))
                    .unwrap_or_default();
                if ssid.is_empty() {
                    bail!("no network is waiting for a password");
                }
                let pw = arg(0).to_string();
                return Box::pin(self.command("connect", &[ssid, pw])).await;
            }
            "disconnect" => {
                let name = self.inner.lock().unwrap().snap.name.clone();
                crate::util::run_ok(&["nmcli", "connection", "down", "id", &name], 10000).await;
            }
            "forget" => {
                crate::util::run_ok(&["nmcli", "connection", "delete", "id", arg(0)], 10000).await;
            }
            "settings" => {
                app().ui.close_all().await;
                crate::util::spawn_sh("nm-connection-editor");
            }
            _ => bail!("wifi toggle|on|off|scan|connect <ssid> [pw]|disconnect|forget <name>"),
        }
        self.kick.notify_one();
        Ok(String::new())
    }
}

pub async fn run() {
    let net = &app().net;
    let Some(conn) = app().system_bus.get() else { return };
    net.refresh().await;
    let mut stream = match signals_from(conn, NM).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sheels: NetworkManager signals unavailable: {e}");
            return;
        }
    };
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            m = stream.next() => {
                let Some(Ok(m)) = m else { break };
                let h = m.header();
                let path = h.path().map(|p| p.as_str().to_string()).unwrap_or_default();
                let iface = h.interface().map(|i| i.as_str().to_string()).unwrap_or_default();
                let relevant = if !path.starts_with("/org/freedesktop/NetworkManager") {
                    false
                } else if path.contains("/AccessPoint/") {
                    // Only our AP (or any AP while the list is shown).
                    let i = net.inner.lock().unwrap();
                    path == i.snap.ap_path || i.list_demand
                } else {
                    !iface.contains("IP4Config") && !iface.contains("IP6Config") && !iface.contains("DHCP")
                };
                if relevant && deadline.is_none() {
                    deadline = Some(tokio::time::Instant::now() + Duration::from_millis(300));
                }
            }
            _ = net.kick.notified() => {
                deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(50));
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                deadline = None;
                net.refresh().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wifi_icons() {
        assert_eq!(super::wifi_icon(100, false), "network_wifi");
        assert_eq!(super::wifi_icon(10, true), "signal_wifi_0_bar");
        assert_eq!(super::wifi_icon(45, true), "network_wifi_2_bar_locked");
    }
}

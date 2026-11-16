//! BlueZ over D-Bus (ObjectManager snapshot + debounced signal refresh).

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::app::app;
use crate::dbus::{managed_objects, s, signals_from};

const BLUEZ: &str = "org.bluez";

#[derive(Default)]
struct Inner {
    available: bool,
    adapter: String,
    powered: bool,
    discovering: bool,
    discoverable: bool,
    pairable: bool,
    devices: Vec<Value>,
}

#[derive(Default)]
pub struct Bluetooth {
    inner: Mutex<Inner>,
    kick: tokio::sync::Notify,
}

/// Caelestia's `Icons.getBluetoothIcon` rules.
fn device_icon(icon: &str) -> &'static str {
    let rules: [(&[&str], &str); 5] = [
        (&["headset", "headphones"], "headphones"),
        (&["audio"], "speaker"),
        (&["phone"], "smartphone"),
        (&["mouse"], "mouse"),
        (&["keyboard"], "keyboard"),
    ];
    for (needles, r) in rules {
        if needles.iter().any(|n| icon.contains(n)) {
            return r;
        }
    }
    "bluetooth"
}

impl Bluetooth {
    pub fn json(&self) -> Value {
        let i = self.inner.lock().unwrap();
        let connected: Vec<&Value> = i.devices.iter().filter(|d| d["connected"] == true).collect();
        let icon = if !i.powered {
            "bluetooth_disabled"
        } else if !connected.is_empty() {
            "bluetooth_connected"
        } else {
            "bluetooth"
        };
        json!({
            "available": i.available,
            "powered": i.powered,
            "discovering": i.discovering,
            "discoverable": i.discoverable,
            "pairable": i.pairable,
            "icon": icon,
            "connected": connected.len(),
            "devices": i.devices,
        })
    }

    async fn refresh(&self) {
        let Some(conn) = app().system_bus.get() else { return };
        let Ok(objs) = managed_objects(conn, BLUEZ, "/").await else {
            *self.inner.lock().unwrap() = Inner::default();
            app().publish_status();
            return;
        };
        let mut inner = Inner { available: true, ..Default::default() };
        let mut devs = vec![];
        if let Some(map) = objs.as_object() {
            for (path, ifaces) in map {
                if let Some(a) = ifaces.get("org.bluez.Adapter1")
                    && inner.adapter.is_empty() {
                        inner.adapter = path.clone();
                        inner.powered = a["Powered"].as_bool().unwrap_or(false);
                        inner.discovering = a["Discovering"].as_bool().unwrap_or(false);
                        inner.discoverable = a["Discoverable"].as_bool().unwrap_or(false);
                        inner.pairable = a["Pairable"].as_bool().unwrap_or(false);
                    }
                if let Some(d) = ifaces.get("org.bluez.Device1") {
                    let name = s(&d["Alias"]);
                    let paired = d["Paired"].as_bool().unwrap_or(false);
                    let connected = d["Connected"].as_bool().unwrap_or(false);
                    // Hide nameless unpaired devices (MAC-only noise).
                    if !paired && !connected && (name.is_empty() || name.replace('-', ":") == s(&d["Address"])) {
                        continue;
                    }
                    let battery = ifaces.get("org.bluez.Battery1").and_then(|b| b["Percentage"].as_u64());
                    let status = match (connected, battery) {
                        (true, Some(b)) => format!("Connected · {b}%"),
                        (true, None) => "Connected".to_string(),
                        _ if paired => "Paired".to_string(),
                        _ => "Available".to_string(),
                    };
                    devs.push(json!({
                        "path": path,
                        "name": name,
                        "address": s(&d["Address"]),
                        "paired": paired,
                        "connected": connected,
                        "trusted": d["Trusted"].as_bool().unwrap_or(false),
                        "icon": device_icon(&s(&d["Icon"])),
                        "battery": battery,
                        "status": status,
                    }));
                }
            }
        }
        devs.sort_by(|a, b| {
            b["connected"]
                .as_bool()
                .cmp(&a["connected"].as_bool())
                .then(b["paired"].as_bool().cmp(&a["paired"].as_bool()))
                .then(s(&a["name"]).cmp(&s(&b["name"])))
        });
        devs.truncate(16);
        inner.devices = devs;
        *self.inner.lock().unwrap() = inner;
        app().publish_status();
    }

    fn dev_path(&self, addr: &str) -> Option<String> {
        let i = self.inner.lock().unwrap();
        i.devices.iter().find(|d| d["address"] == addr || d["path"] == addr).map(|d| s(&d["path"]))
    }

    pub async fn command(&self, sub: &str, arg: &str) -> Result<String> {
        let Some(conn) = app().system_bus.get() else { bail!("no system bus") };
        let adapter = self.inner.lock().unwrap().adapter.clone();
        if adapter.is_empty() {
            bail!("no bluetooth adapter");
        }
        match sub {
            "toggle" | "on" | "off" => {
                let on = match sub {
                    "on" => true,
                    "off" => false,
                    _ => !self.inner.lock().unwrap().powered,
                };
                crate::dbus::set(conn, BLUEZ, &adapter, "org.bluez.Adapter1", "Powered", zbus::Variant::from(on)).await?;
            }
            "discoverable" | "pairable" => {
                let (prop, cur) = {
                    let i = self.inner.lock().unwrap();
                    if sub == "discoverable" { ("Discoverable", i.discoverable) } else { ("Pairable", i.pairable) }
                };
                crate::dbus::set(conn, BLUEZ, &adapter, "org.bluez.Adapter1", prop, zbus::Variant::from(!cur)).await?;
            }
            "scan" => {
                let method = if self.inner.lock().unwrap().discovering { "StopDiscovery" } else { "StartDiscovery" };
                conn.call_method(Some(BLUEZ), adapter.as_str(), Some("org.bluez.Adapter1"), method, &()).await?;
                if method == "StartDiscovery" {
                    // Stop automatically after 15 s like most UIs.
                    let a = adapter.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(15)).await;
                        if let Some(c) = app().system_bus.get() {
                            let _ = c.call_method(Some(BLUEZ), a.as_str(), Some("org.bluez.Adapter1"), "StopDiscovery", &()).await;
                        }
                    });
                }
            }
            "connect" | "disconnect" | "pair" | "forget" => {
                let Some(p) = self.dev_path(arg) else { bail!("unknown device {arg}") };
                let conn = conn.clone();
                let sub = sub.to_string();
                // Connecting can take many seconds: do it in the background.
                tokio::spawn(async move {
                    let r = match sub.as_str() {
                        "connect" => {
                            let _ = crate::dbus::set(&conn, BLUEZ, &p, "org.bluez.Device1", "Trusted", zbus::Variant::from(true)).await;
                            conn.call_method(Some(BLUEZ), p.as_str(), Some("org.bluez.Device1"), "Connect", &()).await
                        }
                        "disconnect" => conn.call_method(Some(BLUEZ), p.as_str(), Some("org.bluez.Device1"), "Disconnect", &()).await,
                        "pair" => conn.call_method(Some(BLUEZ), p.as_str(), Some("org.bluez.Device1"), "Pair", &()).await,
                        _ => {
                            let adapter = app().bt.inner.lock().unwrap().adapter.clone();
                            let path = zbus::ObjectPath::try_from(p.as_str()).map(|o| o.into_owned());
                            match path {
                                Ok(op) => {
                                    conn.call_method(Some(BLUEZ), adapter.as_str(), Some("org.bluez.Adapter1"), "RemoveDevice", &(op,)).await
                                }
                                Err(e) => Err(e),
                            }
                        }
                    };
                    if let Err(e) = r {
                        eprintln!("sheels: bluetooth {sub}: {e}");
                    }
                    app().bt.kick.notify_one();
                });
            }
            _ => bail!("bluetooth toggle|on|off|scan|discoverable|pairable|connect|disconnect|pair|forget <addr>"),
        }
        self.kick.notify_one();
        Ok(String::new())
    }
}

pub async fn run() {
    let bt = &app().bt;
    let Some(conn) = app().system_bus.get() else { return };
    bt.refresh().await;
    let Ok(mut stream) = signals_from(conn, BLUEZ).await else { return };
    // BlueZ may start after us: also watch for the name appearing.
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut owner_tick = tokio::time::interval(Duration::from_secs(60));
    owner_tick.tick().await;
    loop {
        tokio::select! {
            m = stream.next() => {
                let Some(Ok(m)) = m else { break };
                // zbus cannot filter well-known senders client-side: make sure
                // this is really BlueZ (and not NetworkManager on the same bus).
                let h = m.header();
                let path = h.path().map(|p| p.as_str().to_string()).unwrap_or_default();
                let member = h.member().map(|p| p.as_str().to_string()).unwrap_or_default();
                if path.starts_with("/org/bluez") || (path == "/" && member.starts_with("Interfaces")) {
                    deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(200));
                }
            }
            _ = bt.kick.notified() => {
                deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(50));
            }
            _ = owner_tick.tick() => {
                if !bt.inner.lock().unwrap().available { bt.refresh().await; }
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                deadline = None;
                bt.refresh().await;
            }
        }
    }
}

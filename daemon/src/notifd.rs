//! Built-in notification daemon (org.freedesktop.Notifications).
//!
//! Popups use a fixed number of *stable slots* instead of an EWW `for` loop:
//! EWW rebuilds every child of a `for` on each update, which would kill the
//! slide animations. A notification keeps its slot for its whole life, so only
//! that slot's revealer animates when it appears or leaves.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;
use zbus::{OwnedVariant, Variant};

use crate::app::app;

pub const SLOTS: usize = 5;
const HISTORY_MAX: usize = 100;
const PATH: &str = "/org/freedesktop/Notifications";

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Notif {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub icon: String,  // icon theme name
    pub image: String, // file path
    pub actions: Vec<(String, String)>,
    pub urgency: u8,
    pub time: u64,
    #[serde(default)]
    pub transient: bool,
    #[serde(default)]
    pub resident: bool,
    #[serde(default)]
    pub mat_icon: String, // Material symbol fallback
    /// Shell event toast (utilities.toasts.*): transient, counted against utilities.maxToasts.
    #[serde(default)]
    pub toast: bool,
}

#[derive(Clone, Default)]
struct Slot {
    id: u32,
    closing: bool,
}

#[derive(Default)]
struct Inner {
    list: Vec<Notif>,
    next_id: u32,
    slots: [Option<Slot>; SLOTS],
    window_mapped: bool,
    generation: u64,
    expanded: HashMap<String, bool>,
}

#[derive(Default)]
pub struct Notifd {
    inner: Mutex<Inner>,
    save_kick: tokio::sync::Notify,
}

/// Strip (most of) the HTML-ish markup notifications may carry; EWW labels
/// show plain text.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#39;", "'").replace("&amp;", "&")
}

/// Caelestia `Icons.getNotifIcon`.
fn notif_mat_icon(summary: &str, urgency: u8) -> String {
    let s = summary.to_lowercase();
    let rules: [(&[&str], &str); 11] = [
        (&["reboot"], "restart_alt"),
        (&["recording"], "screen_record"),
        (&["battery"], "power"),
        (&["screenshot"], "screenshot_monitor"),
        (&["welcome"], "waving_hand"),
        (&["time", "a break"], "schedule"),
        (&["installed"], "download"),
        (&["update"], "update"),
        (&["unable to"], "deployed_code_alert"),
        (&["profile"], "person"),
        (&["file"], "folder_copy"),
    ];
    for (needles, r) in rules {
        if needles.iter().any(|n| s.contains(n)) {
            return r.to_string();
        }
    }
    if urgency == 2 { "release_alert".into() } else { "chat".into() }
}

fn rel_time(t: u64) -> String {
    let d = (crate::util::now_ms().saturating_sub(t)) / 1000;
    if d < 60 {
        "now".into()
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else {
        format!("{}d", d / 86400)
    }
}

fn save_image_data(id: u32, v: &Variant<'_>) -> Option<String> {
    let v = if let Variant::Variant(b) = v { b.as_ref() } else { v };
    let Variant::Structure(st) = v else { return None };
    let f = st.fields();
    if f.len() < 7 {
        return None;
    }
    let num = |x: &Variant| -> Option<i64> {
        match x {
            Variant::I32(n) => Some(*n as i64),
            Variant::U32(n) => Some(*n as i64),
            _ => None,
        }
    };
    let (w, h, stride) = (num(&f[0])? as u32, num(&f[1])? as u32, num(&f[2])? as usize);
    let alpha = matches!(f[3], Variant::Bool(true));
    let ch = num(&f[5])? as usize;
    let Variant::Array(arr) = &f[6] else { return None };
    let data: Vec<u8> = arr.inner().iter().filter_map(|b| if let Variant::U8(x) = b { Some(*x) } else { None }).collect();
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let mut rgba = image::RgbaImage::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let o = y * stride + x * ch;
            if o + ch > data.len() {
                break;
            }
            let a = if alpha && ch >= 4 { data[o + 3] } else { 255 };
            rgba.put_pixel(x as u32, y as u32, image::Rgba([data[o], data[o + 1], data[o + 2], a]));
        }
    }
    let img = image::DynamicImage::ImageRgba8(rgba).thumbnail(128, 128);
    let dir = crate::util::cache_dir().join("notifs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{id}.png"));
    img.save(&path).ok()?;
    crate::util::trim_heap();
    Some(path.to_string_lossy().into_owned())
}

fn is_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("file://")
}

impl Notifd {
    pub fn has_popups(&self) -> bool {
        self.inner.lock().unwrap().slots.iter().any(Option::is_some)
    }

    fn notif_json(n: &Notif) -> Value {
        json!({
            "id": n.id, "app": n.app, "summary": n.summary, "body": n.body,
            "icon": n.icon, "image": n.image, "matIcon": n.mat_icon,
            "actions": n.actions.iter().enumerate().filter(|(_, (k, _))| k != "default").map(|(i, (_, l))| json!({"i": i, "label": l})).collect::<Vec<_>>(),
            "hasDefault": n.actions.iter().any(|(k, _)| k == "default"),
            "urgency": n.urgency, "critical": n.urgency == 2, "time": rel_time(n.time),
            "clock": format_clock(n.time),
        })
    }

    pub fn publish(&self) {
        let i = self.inner.lock().unwrap();
        let slots: Vec<Value> = i
            .slots
            .iter()
            .map(|s| match s {
                Some(s) => match i.list.iter().find(|n| n.id == s.id) {
                    Some(n) => {
                        let mut v = Self::notif_json(n);
                        v["show"] = Value::Bool(!s.closing);
                        v["used"] = Value::Bool(true);
                        v
                    }
                    None => json!({"show": false, "used": false}),
                },
                None => json!({"show": false, "used": false}),
            })
            .collect();
        // Group history by app (newest group first), like Caelestia's sidebar.
        let mut order: Vec<String> = vec![];
        let mut groups: HashMap<String, Vec<&Notif>> = HashMap::new();
        for n in i.list.iter().filter(|n| !n.transient) {
            if !groups.contains_key(&n.app) {
                order.push(n.app.clone());
            }
            groups.entry(n.app.clone()).or_default().push(n);
        }
        let preview = crate::app::cfg().notifs.group_preview_num.max(1);
        let groups: Vec<Value> = order
            .iter()
            .take(6)
            .enumerate()
            .map(|(gi, app)| {
                let ns = &groups[app];
                let expanded = *i.expanded.get(app).unwrap_or(&false);
                let shown: Vec<Value> = ns.iter().take(if expanded { preview.max(6) } else { preview }).map(|n| Self::notif_json(n)).collect();
                json!({
                    "app": if app.is_empty() { "Unknown" } else { app },
                    "key": app,
                    "icon": ns[0].icon, "image": ns[0].image, "matIcon": ns[0].mat_icon,
                    "count": ns.len(), "expanded": expanded, "idx": gi, "time": rel_time(ns[0].time),
                    "critical": ns.iter().any(|n| n.urgency == 2),
                    "notifs": shown,
                })
            })
            .collect();
        let v = json!({
            "count": i.list.len(),
            "slots": slots,
            "groups": groups,
        });
        drop(i);
        app().hub.publish("notifs", &v);
    }

    pub async fn remap(&self) {
        let mapped = self.inner.lock().unwrap().window_mapped;
        if mapped {
            app().eww.close(&["notif-popups"]).await;
            let screen = app().ui.active_output();
            app().eww.open("notif-popups", "notif-popups", screen.as_deref(), &[]).await;
        }
    }

    async fn ensure_window(&self, want: bool) {
        let (mapped, generation) = {
            let mut i = self.inner.lock().unwrap();
            i.generation += 1;
            (i.window_mapped, i.generation)
        };
        if want && !mapped {
            let screen = app().ui.active_output();
            app().eww.open("notif-popups", "notif-popups", screen.as_deref(), &[]).await;
            self.inner.lock().unwrap().window_mapped = true;
            tokio::time::sleep(Duration::from_millis(25)).await;
        } else if !want && mapped {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let n = &app().notifs;
                let close = {
                    let mut i = n.inner.lock().unwrap();
                    if i.generation == generation && i.slots.iter().all(Option::is_none) && i.window_mapped {
                        i.window_mapped = false;
                        true
                    } else {
                        false
                    }
                };
                if close {
                    app().eww.close(&["notif-popups"]).await;
                }
            });
        }
    }

    /// Insert or replace a notification; returns its id.
    pub async fn add(&self, mut n: Notif, replaces: u32, timeout_ms: i32) -> u32 {
        let (id, popup) = {
            let mut i = self.inner.lock().unwrap();
            if replaces != 0 && i.list.iter().any(|x| x.id == replaces) {
                n.id = replaces;
                i.list.retain(|x| x.id != replaces);
            } else {
                i.next_id = i.next_id.wrapping_add(1).max(1);
                n.id = i.next_id;
            }
            let id = n.id;
            let dnd = app().misc.lock().unwrap().dnd;
            let critical = n.urgency == 2;
            // notifs.fullscreen / utilities.toastsFullscreen: popups over fullscreen apps.
            let fs_ok = !app().ui.is_fullscreen() || {
                let c = crate::app::cfg();
                if n.toast {
                    c.utilities.toasts_fullscreen
                } else {
                    match c.notifs.fullscreen.as_str() {
                        "off" => false,
                        "critical" => critical,
                        _ => true,
                    }
                }
            };
            let popup = (!dnd || critical || n.toast) && fs_ok;
            // Keep at most utilities.maxToasts toast popups on screen.
            if popup && n.toast {
                let max = crate::app::cfg().utilities.max_toasts.max(1);
                let toast_slots: Vec<usize> = (0..SLOTS)
                    .filter(|k| {
                        i.slots[*k].as_ref().is_some_and(|s| !s.closing && i.list.iter().any(|x| x.id == s.id && x.toast))
                    })
                    .collect();
                if toast_slots.len() >= max {
                    // Drop the oldest toast outright (it is transient).
                    if let Some(k) = toast_slots.iter().copied().min_by_key(|k| i.slots[*k].as_ref().map(|s| s.id).unwrap_or(0)) {
                        let old = i.slots[k].take().map(|s| s.id);
                        if let Some(old) = old {
                            i.list.retain(|x| x.id != old);
                        }
                    }
                }
            }
            i.list.insert(0, n);
            i.list.truncate(HISTORY_MAX);
            if popup && !i.slots.iter().flatten().any(|s| s.id == id) {
                if let Some(free) = i.slots.iter().position(Option::is_none) {
                    i.slots[free] = Some(Slot { id, closing: false });
                } else {
                    // All slots busy: recycle the oldest popup's slot.
                    let oldest = (0..SLOTS)
                        .min_by_key(|k| i.slots[*k].as_ref().map(|s| s.id).unwrap_or(u32::MAX))
                        .unwrap_or(0);
                    i.slots[oldest] = Some(Slot { id, closing: false });
                }
            }
            (id, popup)
        };
        if popup {
            self.ensure_window(true).await;
        }
        self.publish();
        self.save_kick.notify_one();
        if popup {
            let cfg = crate::app::cfg();
            let critical = self.inner.lock().unwrap().list.first().map(|n| n.urgency == 2).unwrap_or(false);
            let ms = if !cfg.notifs.expire || (critical && timeout_ms <= 0) {
                0
            } else if timeout_ms > 0 {
                timeout_ms as u64
            } else {
                cfg.notifs.default_expire_timeout
            };
            drop(cfg);
            if ms > 0 {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    app().notifs.hide_popup(id).await;
                });
            }
        }
        id
    }

    /// Slide a popup out (keeps it in history).
    pub async fn hide_popup(&self, id: u32) {
        let k = {
            let mut i = self.inner.lock().unwrap();
            let Some(k) = i.slots.iter().position(|s| s.as_ref().map(|s| s.id == id && !s.closing).unwrap_or(false)) else {
                return;
            };
            if let Some(s) = i.slots[k].as_mut() {
                s.closing = true;
            }
            k
        };
        self.publish();
        tokio::time::sleep(Duration::from_millis(450)).await;
        let empty = {
            let mut i = self.inner.lock().unwrap();
            if i.slots[k].as_ref().map(|s| s.id == id && s.closing).unwrap_or(false) {
                i.slots[k] = None;
            }
            // Transient notifications vanish from history with the popup.
            if i.list.iter().any(|n| n.id == id && n.transient) {
                i.list.retain(|n| n.id != id);
            }
            i.slots.iter().all(Option::is_none)
        };
        self.publish();
        if empty {
            self.ensure_window(false).await;
        }
    }

    pub async fn dismiss(&self, id: u32, reason: u32) {
        self.hide_popup(id).await;
        let existed = {
            let mut i = self.inner.lock().unwrap();
            let before = i.list.len();
            i.list.retain(|n| n.id != id);
            before != i.list.len()
        };
        let _ = std::fs::remove_file(crate::util::cache_dir().join("notifs").join(format!("{id}.png")));
        self.publish();
        self.save_kick.notify_one();
        if existed {
            emit_closed(id, reason).await;
        }
    }

    /// Event toast (Caelestia utilities toasts): transient popup gated by
    /// the matching `utilities.toasts.*` switch.
    pub fn toast(&self, enabled: bool, title: &str, body: &str, mat_icon: &str) {
        if !enabled {
            return;
        }
        let n = Notif {
            app: "Sheels".into(),
            summary: title.into(),
            body: body.into(),
            mat_icon: mat_icon.into(),
            urgency: 1,
            time: crate::util::now_ms(),
            transient: true,
            toast: true,
            ..Default::default()
        };
        tokio::spawn(async move {
            app().notifs.add(n, 0, 3000).await;
        });
    }

    pub fn internal(&self, title: &str, body: &str, mat_icon: &str, critical: bool) {
        let n = Notif {
            app: "Sheels".into(),
            summary: title.into(),
            body: body.into(),
            mat_icon: mat_icon.into(),
            urgency: if critical { 2 } else { 1 },
            time: crate::util::now_ms(),
            ..Default::default()
        };
        tokio::spawn(async move {
            app().notifs.add(n, 0, -1).await;
        });
    }

    pub async fn command(&self, sub: &str, rest: &[String]) -> Result<String> {
        let id: u32 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        match sub {
            "clear" => {
                let ids: Vec<u32> = self.inner.lock().unwrap().list.iter().filter(|n| !n.resident).map(|n| n.id).collect();
                for id in ids {
                    Box::pin(self.dismiss(id, 2)).await;
                }
            }
            "expand-idx" | "clear-app-idx" => {
                // Group addressed by index (app names are app-provided text).
                let k: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
                let app_name = {
                    let i = self.inner.lock().unwrap();
                    let mut order: Vec<String> = vec![];
                    for n in &i.list {
                        if !order.contains(&n.app) {
                            order.push(n.app.clone());
                        }
                    }
                    order.get(k).cloned()
                };
                if let Some(a) = app_name {
                    let sub2 = if sub == "expand-idx" { "expand" } else { "clear-app" };
                    return Box::pin(self.command(sub2, &[a])).await;
                }
            }
            "clear-app" => {
                let app_name = rest.first().cloned().unwrap_or_default();
                let ids: Vec<u32> = self.inner.lock().unwrap().list.iter().filter(|n| n.app == app_name).map(|n| n.id).collect();
                for id in ids {
                    Box::pin(self.dismiss(id, 2)).await;
                }
            }
            "expand" => {
                let app_name = rest.first().cloned().unwrap_or_default();
                {
                    let mut i = self.inner.lock().unwrap();
                    let e = i.expanded.entry(app_name).or_insert(false);
                    *e = !*e;
                }
                self.publish();
            }
            "dnd" => {
                {
                    let mut m = app().misc.lock().unwrap();
                    m.dnd = !m.dnd;
                }
                app().publish_status();
            }
            "dismiss" => self.dismiss(id, 2).await,
            "hide" => self.hide_popup(id).await,
            "action" => {
                // Actions are addressed by index (never by app-provided text).
                let arg = rest.get(1).cloned().unwrap_or_else(|| "default".into());
                let key = match arg.parse::<usize>() {
                    Ok(i) => self.inner.lock().unwrap().list.iter().find(|n| n.id == id)
                        .and_then(|n| n.actions.get(i).map(|a| a.0.clone())).unwrap_or_else(|| "default".into()),
                    Err(_) => "default".into(),
                };
                emit_action(id, &key).await;
                let resident = self.inner.lock().unwrap().list.iter().any(|n| n.id == id && n.resident);
                if !resident {
                    self.dismiss(id, 2).await;
                }
            }
            "test" => {
                self.internal("Welcome to Sheels", "Caelestia-style shell running on driftwm + EWW.", "waving_hand", false);
            }
            _ => bail!("notifs clear|dnd|dismiss <id>|hide <id>|action <id> <key>|expand <app>|clear-app <app>|test"),
        }
        Ok(String::new())
    }

    pub fn save(&self) {
        let i = self.inner.lock().unwrap();
        let list: Vec<&Notif> = i.list.iter().filter(|n| !n.transient).collect();
        if let Ok(s) = serde_json::to_string(&list) {
            let _ = crate::util::write_atomic(&crate::util::state_dir().join("notifs.json"), s.as_bytes());
        }
    }

    fn load(&self) {
        let path = crate::util::state_dir().join("notifs.json");
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(list) = serde_json::from_str::<Vec<Notif>>(&s) {
                let mut i = self.inner.lock().unwrap();
                i.next_id = list.iter().map(|n| n.id).max().unwrap_or(0);
                i.list = list;
            }
    }
}

fn format_clock(t: u64) -> String {
    // Local HH:MM using libc (no chrono dependency).
    let secs = (t / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

async fn emit_closed(id: u32, reason: u32) {
    if let Some(conn) = app().session_bus.get()
        && let Ok(e) = SignalEmitter::new(conn, PATH) {
            let _ = Server::notification_closed(&e, id, reason).await;
        }
}

async fn emit_action(id: u32, key: &str) {
    if let Some(conn) = app().session_bus.get()
        && let Ok(e) = SignalEmitter::new(conn, PATH) {
            let _ = Server::action_invoked(&e, id, key).await;
            // Activation token so the target app may raise itself.
            let _ = Server::activation_token(&e, id, "").await;
        }
}

struct Server;

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["actions", "body", "icon-static", "persistence", "body-hyperlinks"]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = hints.get("urgency").and_then(|v| match &**v {
            Variant::U8(u) => Some(*u),
            _ => None,
        });
        let flag = |k: &str| hints.get(k).map(|v| matches!(&**v, Variant::Bool(true))).unwrap_or(false);
        let hint_str = |k: &str| hints.get(k).and_then(|v| if let Variant::Str(s) = &**v { Some(s.as_str().to_string()) } else { None });
        let notifd = &app().notifs;
        // Reserve an id up-front so image files can be named after it.
        let provisional = {
            let i = notifd.inner.lock().unwrap();
            if replaces_id != 0 { replaces_id } else { i.next_id.wrapping_add(1).max(1) }
        };
        let mut image = String::new();
        for k in ["image-data", "image_data", "icon_data"] {
            if let Some(v) = hints.get(k)
                && let Some(p) = save_image_data(provisional, v) {
                    image = p;
                    break;
                }
        }
        let mut icon = String::new();
        if image.is_empty()
            && let Some(p) = hint_str("image-path").or_else(|| hint_str("image_path")) {
                if is_path(&p) {
                    image = p.trim_start_matches("file://").to_string();
                } else {
                    icon = p;
                }
            }
        if !app_icon.is_empty() {
            if is_path(&app_icon) {
                if image.is_empty() {
                    image = app_icon.trim_start_matches("file://").to_string();
                }
            } else if icon.is_empty() {
                icon = app_icon.clone();
            }
        }
        if icon.is_empty()
            && let Some(de) = hint_str("desktop-entry") {
                icon = app().launcher.icon_for(&de).unwrap_or_default();
            }
        let summary = strip_markup(&summary);
        let n = Notif {
            id: 0,
            app: app_name,
            mat_icon: notif_mat_icon(&summary, urgency.unwrap_or(1)),
            summary,
            body: strip_markup(&body),
            icon,
            image,
            actions: actions.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect(),
            urgency: urgency.unwrap_or(1),
            time: crate::util::now_ms(),
            transient: flag("transient"),
            resident: flag("resident"),
            toast: false,
        };
        notifd.add(n, replaces_id, expire_timeout).await
    }

    async fn close_notification(&self, id: u32) {
        app().notifs.dismiss(id, 3).await;
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("sheels".into(), "sheels".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }

    #[zbus(signal)]
    async fn notification_closed(emitter: &SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, action_key: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn activation_token(emitter: &SignalEmitter<'_>, id: u32, token: &str) -> zbus::Result<()>;
}

pub async fn run() {
    let n = &app().notifs;
    n.load();
    n.publish();
    if let Some(conn) = app().session_bus.get() {
        if let Err(e) = conn.object_server().at(PATH, Server).await {
            eprintln!("sheels: cannot export notifications: {e}");
        } else if let Err(e) = conn.request_name("org.freedesktop.Notifications").await {
            eprintln!("sheels: another notification daemon owns the bus name ({e}); popups disabled");
        }
    }
    // Debounced persistence + relative-time refresh every minute.
    let mut minute = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = n.save_kick.notified() => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                n.save();
            }
            _ = minute.tick() => {
                if !n.inner.lock().unwrap().list.is_empty() { n.publish(); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markup_is_stripped() {
        assert_eq!(strip_markup("<b>Hi</b> &amp; <a href='x'>link</a>"), "Hi & link");
        assert_eq!(strip_markup("a &lt; b"), "a < b");
    }
    #[test]
    fn mat_icons() {
        assert_eq!(notif_mat_icon("Screenshot taken", 1), "screenshot_monitor");
        assert_eq!(notif_mat_icon("hello", 2), "release_alert");
        assert_eq!(notif_mat_icon("hello", 1), "chat");
    }
}

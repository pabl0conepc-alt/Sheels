//! driftwm integration over its JSON IPC socket.
//!
//! driftwm has no workspaces: it is one infinite canvas. Its *bookmarks* are
//! exported as workspaces (ext-workspace-v1), so the bar shows bookmarks the
//! same way Caelestia shows Hyprland workspaces: shapes for each, a morphing
//! indicator on the active one, and the window icons that live near it.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::app::app;

#[derive(Deserialize, Default, Clone)]
struct WinInfo {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    app_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    position: [i32; 2],
    #[serde(default)]
    size: [i32; 2],
    #[serde(default)]
    is_focused: bool,
    #[serde(default)]
    is_widget: bool,
    #[serde(default)]
    suspended: bool,
}

#[derive(Deserialize, Default, Clone)]
struct OutInfo {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: [i32; 2],
    #[serde(default)]
    active: bool,
    #[serde(default)]
    active_bookmark: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
struct FsInfo {
    #[serde(default)]
    output: String,
    #[serde(default)]
    app_id: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize, Default, Clone)]
struct StateInfo {
    #[serde(default)]
    layout: String,
    #[serde(default)]
    layout_short: String,
    #[serde(default)]
    windows: Vec<WinInfo>,
    #[serde(default)]
    fullscreen: Vec<FsInfo>,
    #[serde(default)]
    outputs: Vec<OutInfo>,
    #[serde(default)]
    active_bookmark: Option<String>,
}

#[derive(Default)]
struct Inner {
    connected: bool,
    bookmarks: Vec<(String, f64, f64)>,
    active_bookmark: Option<String>,
    layout: String,
    layout_short: String,
    last_state: Option<StateInfo>,
    shape: usize,
}

/// Material 3 shapes for the focused workspace (eww/assets/shapes/*.svg);
/// a random one is picked on every focus change, like Caelestia.
const SHAPES: &[&str] = &[
    "slanted", "oval", "pill", "triangle", "arrow", "diamond", "pentagon", "gem", "verysunny", "sunny", "cookie4", "cookie6",
    "cookie7", "cookie9", "cookie12", "clover4", "softburst", "ghostish",
];

#[derive(Default)]
pub struct Wm {
    inner: Mutex<Inner>,
}

pub fn socket() -> PathBuf {
    if let Some(p) = std::env::var_os("DRIFTWM_SOCKET") {
        return PathBuf::from(p);
    }
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let disp = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    PathBuf::from(rt).join("driftwm").join(format!("ipc-{disp}.sock"))
}

/// One request/one reply on a fresh connection.
pub async fn request(req: &Value) -> Result<Value> {
    let mut s = tokio::time::timeout(Duration::from_secs(2), UnixStream::connect(socket())).await??;
    s.write_all(format!("{req}\n").as_bytes()).await?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), r.read_line(&mut line)).await??;
    let v: Value = serde_json::from_str(line.trim())?;
    if let Some(e) = v.get("Err") {
        bail!("driftwm: {}", e.as_str().unwrap_or("error"));
    }
    Ok(v.get("Ok").cloned().unwrap_or(Value::Null))
}

pub async fn action(a: &str) -> Result<()> {
    request(&json!({ "Action": a })).await.map(|_| ())
}

fn natural_key(s: &str) -> (u64, String) {
    (s.parse::<u64>().unwrap_or(u64::MAX), s.to_string())
}

/// Material symbol for an app, following Caelestia's category heuristics.
pub fn app_category_icon(app_id: &str) -> &'static str {
    let a = app_id.to_lowercase();
    let has = |k: &[&str]| k.iter().any(|x| a.contains(x));
    if has(&["foot", "kitty", "alacritty", "wezterm", "terminal", "konsole", "ghostty", "xterm", "ptyxis"]) {
        "terminal"
    } else if has(&["firefox", "zen", "chrom", "brave", "vivaldi", "librewolf", "browser", "qutebrowser", "epiphany"]) {
        "public"
    } else if has(&["code", "codium", "zed", "nvim", "neovide", "emacs", "jetbrains", "idea", "kate", "gedit", "sublime"]) {
        "code"
    } else if has(&["discord", "vesktop", "telegram", "signal", "whatsapp", "element", "slack", "teams", "thunderbird"]) {
        "forum"
    } else if has(&["spotify", "feishin", "music", "rhythmbox", "elisa", "tidal", "cider", "amberol"]) {
        "music_note"
    } else if has(&["mpv", "vlc", "celluloid", "video", "totem", "haruna"]) {
        "play_circle"
    } else if has(&["thunar", "nautilus", "dolphin", "files", "nemo", "pcmanfm", "yazi"]) {
        "folder"
    } else if has(&["steam", "lutris", "heroic", "game", "minecraft", "prism"]) {
        "sports_esports"
    } else if has(&["gimp", "krita", "inkscape", "blender", "photo", "darktable"]) {
        "palette"
    } else if has(&["obs"]) {
        "videocam"
    } else if has(&["pavucontrol", "pwvucontrol"]) {
        "volume_up"
    } else if has(&["btop", "htop", "monitor", "resources"]) {
        "monitor_heart"
    } else if has(&["settings", "control", "nm-connection"]) {
        "settings"
    } else if has(&["office", "writer", "calc", "impress", "onlyoffice", "obsidian", "notion", "todoist", "evince", "zathura", "okular"]) {
        "description"
    } else {
        "terminal"
    }
}

impl Wm {
    pub fn kb_json(&self) -> Value {
        let i = self.inner.lock().unwrap();
        json!({ "layout": i.layout, "short": i.layout_short })
    }

    async fn refresh_bookmarks(&self) {
        if let Ok(v) = request(&json!({ "Bookmark": {} })).await {
            if let Some(map) = v.get("Bookmarks").and_then(Value::as_object) {
                let mut list: Vec<(String, f64, f64)> = map
                    .iter()
                    .filter_map(|(k, p)| {
                        let a = p.as_array()?;
                        Some((k.clone(), a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                    })
                    .collect();
                list.sort_by(|a, b| natural_key(&a.0).cmp(&natural_key(&b.0)));
                self.inner.lock().unwrap().bookmarks = list;
            }
        }
    }

    async fn process(&self, st: StateInfo) {
        let app = app();
        // Bookmarks are not part of the state: refresh when the active one changes.
        let need_bm = {
            let i = self.inner.lock().unwrap();
            i.bookmarks.is_empty() || i.active_bookmark != st.active_bookmark
        };
        if need_bm {
            self.refresh_bookmarks().await;
        }

        let kb_changed = {
            let mut i = self.inner.lock().unwrap();
            if i.active_bookmark != st.active_bookmark {
                let seed = crate::util::now_ms() as usize ^ (i.shape.wrapping_mul(2654435761));
                let mut next = seed % SHAPES.len();
                if next == i.shape {
                    next = (next + 1) % SHAPES.len();
                }
                i.shape = next;
            }
            i.active_bookmark = st.active_bookmark.clone();
            let changed = i.layout != st.layout || i.layout_short != st.layout_short;
            i.layout = st.layout.clone();
            i.layout_short = st.layout_short.clone();
            i.last_state = Some(st.clone());
            changed
        };
        if kb_changed {
            app.publish_status();
        }

        // Outputs → UI (bar per monitor), fullscreen on the active output.
        let outs: Vec<(String, i32, i32, bool)> =
            st.outputs.iter().map(|o| (o.name.clone(), o.size[0], o.size[1], o.active)).collect();
        let active_out = st.outputs.iter().find(|o| o.active).or(st.outputs.first());
        let active_name = active_out.map(|o| o.name.clone()).unwrap_or_default();
        let fs = st.fullscreen.iter().find(|f| f.output == active_name || active_name.is_empty());
        app.ui.set_outputs(outs).await;
        app.ui.set_fullscreen(fs.is_some()).await;

        self.publish(&st, active_out.cloned());
    }

    fn publish(&self, st: &StateInfo, out: Option<OutInfo>) {
        let i = self.inner.lock().unwrap();
        let shown = crate::app::cfg().bar.workspaces_shown.max(1);
        let (vw, vh) = out.as_ref().map(|o| (o.size[0] as f64, o.size[1] as f64)).unwrap_or((1920.0, 1080.0));
        let active = out.as_ref().and_then(|o| o.active_bookmark.clone()).or_else(|| st.active_bookmark.clone());

        let live: Vec<&WinInfo> = st.windows.iter().filter(|w| !w.is_widget && !w.suspended).collect();
        let mut wss = vec![];
        for (name, bx, by) in i.bookmarks.iter().take(shown) {
            // A bookmark is "occupied" when a window overlaps the viewport-sized
            // area centred on it.
            let icons: Vec<&'static str> = live
                .iter()
                .filter(|w| {
                    let (wx, wy) = (w.position[0] as f64, w.position[1] as f64);
                    let (hw, hh) = (w.size[0] as f64 / 2.0, w.size[1] as f64 / 2.0);
                    (wx - bx).abs() < vw / 2.0 + hw && (wy - by).abs() < vh / 2.0 + hh
                })
                .take(5)
                .map(|w| app().launcher.category_icon(&w.app_id).unwrap_or_else(|| app_category_icon(&w.app_id)))
                .collect();
            let is_active = active.as_deref() == Some(name.as_str());
            wss.push(json!({
                "name": name,
                "active": is_active,
                "occupied": !icons.is_empty(),
                "icons": icons,
            }));
        }
        let active_idx = wss.iter().position(|w| w["active"] == true).map(|x| x as i64).unwrap_or(-1);
        // Indicator geometry (keep in sync with scss/_bar.scss): each slot is
        // a 32px shape area plus 20px per window icon (+4px padding), 4px apart.
        let slot_h = |w: &Value| -> i64 {
            let n = w["icons"].as_array().map(|a| a.len()).unwrap_or(0) as i64;
            32 + if n > 0 { n * 20 + 4 } else { 0 }
        };
        let (ind_top, ind_h) = if active_idx >= 0 {
            let top: i64 = wss.iter().take(active_idx as usize).map(|w| slot_h(w) + 4).sum();
            (top, slot_h(&wss[active_idx as usize]))
        } else {
            (0, 32)
        };
        let focused = st.windows.iter().find(|w| w.is_focused);
        let fs = st.fullscreen.first();
        let (title, app_id) = match (focused, fs) {
            (Some(w), _) => (w.title.clone(), w.app_id.clone()),
            (None, Some(f)) => (f.title.clone(), f.app_id.clone()),
            _ => (String::new(), String::new()),
        };
        let v = json!({
            "connected": i.connected,
            "workspaces": wss,
            "activeIdx": active_idx,
            "indTop": ind_top,
            "listH": wss.iter().map(|w| slot_h(w)).sum::<i64>() + 4 * (wss.len() as i64 - 1).max(0),
            "indH": ind_h,
            "activeName": active.unwrap_or_default(),
            "shape": SHAPES[i.shape % SHAPES.len()],
            "window": {
                "title": if title.is_empty() { "Desktop".to_string() } else { title },
                "app": app_id,
                "icon": if app_id.is_empty() { "desktop_windows" } else { app().launcher.category_icon(&app_id).unwrap_or_else(|| app_category_icon(&app_id)) },
                "id": focused.map(|w| w.id).unwrap_or(0),
            },
            "windowCount": live.len(),
        });
        drop(i);
        app().hub.publish("wm", &v);
    }

    pub async fn goto(&self, name: &str) -> Result<String> {
        if name.is_empty() {
            bail!("ws <bookmark>");
        }
        action(&format!("go-to-bookmark {name}")).await?;
        Ok(String::new())
    }

    pub async fn scroll(&self, dir: &str) -> Result<String> {
        let (list, cur) = {
            let i = self.inner.lock().unwrap();
            (i.bookmarks.clone(), i.active_bookmark.clone())
        };
        if list.is_empty() {
            return Ok(String::new());
        }
        let idx = cur.and_then(|c| list.iter().position(|b| b.0 == c));
        let n = list.len().min(crate::app::cfg().bar.workspaces_shown.max(1));
        let next = match (idx, dir) {
            (Some(i), "down" | "next") => (i + 1).min(n - 1),
            (Some(i), _) => i.saturating_sub(1),
            (None, _) => 0,
        };
        action(&format!("go-to-bookmark {}", list[next].0)).await?;
        Ok(String::new())
    }

    pub async fn window_cmd(&self, sub: &str, arg: &str) -> Result<String> {
        let sel: Value = match arg.parse::<u64>() {
            Ok(id) => json!(id),
            Err(_) if arg.is_empty() => Value::Null,
            Err(_) => json!(arg),
        };
        match sub {
            "close" => request(&json!({ "Close": sel })).await?,
            "focus" => request(&json!({ "Focus": sel })).await?,
            "fullscreen" => {
                action("toggle-fullscreen").await?;
                Value::Null
            }
            "center" => {
                action("center-window").await?;
                Value::Null
            }
            "pin" => {
                action("toggle-pin-to-screen").await?;
                Value::Null
            }
            "preview" => {
                // Window screenshot used by the active-window popout.
                let path = crate::util::cache_dir().join("preview.png");
                let target = if arg.is_empty() { json!({ "Window": {} }) } else { json!({ "Window": { "window": sel } }) };
                request(&json!({ "Screenshot": { "target": target, "scale": 0.35, "path": path.to_string_lossy() } }))
                    .await?;
                return Ok(path.to_string_lossy().into_owned());
            }
            _ => bail!("window close|focus|fullscreen|center|pin|preview [id]"),
        };
        Ok(String::new())
    }
}

/// Subscribe loop with reconnection. Events arrive once per frame while the
/// camera animates, so they are coalesced to at most ~20 Hz of processing.
pub async fn run() {
    let wm = &app().wm;
    let mut backoff = 250u64;
    loop {
        match UnixStream::connect(socket()).await {
            Ok(mut s) => {
                backoff = 250;
                if s.write_all(b"\"Subscribe\"\n").await.is_err() {
                    continue;
                }
                wm.inner.lock().unwrap().connected = true;
                wm.refresh_bookmarks().await;
                let mut lines = BufReader::new(s).lines();
                let mut pending: Option<String> = None;
                let mut last = Instant::now() - Duration::from_secs(1);
                loop {
                    let wait = Duration::from_millis(50).saturating_sub(last.elapsed());
                    tokio::select! {
                        line = lines.next_line() => {
                            match line {
                                Ok(Some(l)) => { pending = Some(l); }
                                _ => break,
                            }
                        }
                        _ = tokio::time::sleep(wait), if pending.is_some() => {}
                    }
                    if pending.is_some() && last.elapsed() >= Duration::from_millis(50) {
                        let l = pending.take().unwrap();
                        last = Instant::now();
                        if let Ok(v) = serde_json::from_str::<Value>(&l) {
                            let st = v.get("State").or_else(|| v.get("Ok").and_then(|o| o.get("State")));
                            if let Some(st) = st.cloned() {
                                if let Ok(st) = serde_json::from_value::<StateInfo>(st) {
                                    wm.process(st).await;
                                }
                            }
                        }
                    }
                }
                wm.inner.lock().unwrap().connected = false;
            }
            Err(_) => {
                if backoff >= 4000 {
                    // Not under driftwm (or not yet): still publish something sane.
                    let st = StateInfo::default();
                    wm.publish(&st, None);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(4000);
    }
}

#[allow(dead_code)]
pub fn err(e: &str) -> anyhow::Error {
    anyhow!("{e}")
}

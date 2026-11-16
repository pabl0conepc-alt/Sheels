//! App launcher backend: desktop-entry index, fuzzy search with frecency,
//! Caelestia's `>` actions (calc, scheme, variant, wallpaper, random, light,
//! dark, power actions…).
//!
//! Keyboard: EWW has no key events, so the launcher's GtkEntry uses a GTK CSS
//! binding-set that inserts private-use marker characters for Esc/Up/Down/Tab
//! (see eww/_launcher.scss). They arrive here through `query` and are turned
//! into navigation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::app::app;

const KEY_ESC: char = '\u{e000}';
const KEY_UP: char = '\u{e001}';
const KEY_DOWN: char = '\u{e002}';
const KEY_TAB: char = '\u{e003}';

#[derive(Clone, Default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub generic: String,
    pub comment: String,
    pub keywords: String,
    pub exec: String,
    pub icon: String,
    pub terminal: bool,
    pub categories: Vec<String>,
    pub wm_class: String,
}

#[derive(Default)]
struct Inner {
    apps: Vec<Entry>,
    loaded: bool,
    dir_mtimes: Vec<(PathBuf, std::time::SystemTime)>,
    query: String,
    selected: usize,
    count: usize,
    frecency: HashMap<String, u32>,
    items: Vec<Value>,
    mode: String,
    wallpapers: Vec<Value>,
}

#[derive(Default)]
pub struct Launcher {
    inner: Mutex<Inner>,
}

fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    let data_home = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::util::home().join(".local/share"));
    dirs.push(data_home.join("applications"));
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    // NixOS profiles (in case XDG_DATA_DIRS is minimal, e.g. started by a DM).
    let user = std::env::var("USER").unwrap_or_default();
    for d in [
        "/run/current-system/sw/share/applications".to_string(),
        format!("/etc/profiles/per-user/{user}/share/applications"),
        crate::util::home().join(".nix-profile/share/applications").to_string_lossy().into_owned(),
        "/var/lib/flatpak/exports/share/applications".into(),
        crate::util::home().join(".local/share/flatpak/exports/share/applications").to_string_lossy().into_owned(),
    ] {
        let p = PathBuf::from(d);
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs
}

fn parse_desktop(path: &std::path::Path, id: String) -> Option<Entry> {
    let s = std::fs::read_to_string(path).ok()?;
    let mut in_main = false;
    let mut e = Entry { id, ..Default::default() };
    let mut hidden = false;
    let mut ty = String::new();
    for l in s.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_main = l == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => e.name = v.to_string(),
            "GenericName" => e.generic = v.to_string(),
            "Comment" => e.comment = v.to_string(),
            "Keywords" => e.keywords = v.replace(';', " "),
            "Exec" => e.exec = v.to_string(),
            "Icon" => e.icon = v.to_string(),
            "Terminal" => e.terminal = v == "true",
            "Categories" => e.categories = v.split(';').filter(|c| !c.is_empty()).map(str::to_string).collect(),
            "StartupWMClass" => e.wm_class = v.to_lowercase(),
            "NoDisplay" | "Hidden" if v == "true" => hidden = true,
            "Type" => ty = v.to_string(),
            _ => {}
        }
    }
    if hidden || ty != "Application" || e.name.is_empty() || e.exec.is_empty() {
        return None;
    }
    Some(e)
}

/// Strip desktop-entry field codes from Exec.
fn clean_exec(exec: &str) -> String {
    let mut out = String::new();
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some('%') = chars.next() { out.push('%') }
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// fzy-flavoured score; None = no match.
/// `score` honouring `launcher.useFuzzy.*`: without fuzzy matching only
/// substring matches count (Caelestia falls back to plain search).
fn sc(q: &str, text: &str, fuzzy: bool) -> Option<i64> {
    if !fuzzy && !q.is_empty() && !text.to_lowercase().contains(q) {
        return None;
    }
    score(q, text)
}

pub fn score(q: &str, text: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    let t = text.to_lowercase();
    if t == q {
        return Some(2000);
    }
    if t.starts_with(q) {
        return Some(1500 - t.len() as i64);
    }
    if t.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) {
        return Some(1200 - t.len() as i64);
    }
    if let Some(pos) = t.find(q) {
        return Some(900 - pos as i64);
    }
    // Subsequence with gap penalty and word-boundary bonus.
    let tb: Vec<char> = t.chars().collect();
    let mut ti = 0;
    let mut sc: i64 = 600;
    let mut last: Option<usize> = None;
    for qc in q.chars() {
        let mut found = false;
        while ti < tb.len() {
            if tb[ti] == qc {
                if let Some(l) = last {
                    sc -= ((ti - l - 1) as i64).min(10) * 8;
                }
                if ti == 0 || !tb[ti - 1].is_alphanumeric() {
                    sc += 15;
                }
                last = Some(ti);
                ti += 1;
                found = true;
                break;
            }
            ti += 1;
        }
        if !found {
            return None;
        }
    }
    Some(sc)
}

struct Action {
    name: &'static str,
    icon: &'static str,
    desc: &'static str,
    cmd: &'static str,
    dangerous: bool,
}

const ACTIONS: &[Action] = &[
    Action { name: "Calculator", icon: "calculate", desc: "Do simple maths equations (powered by Qalc)", cmd: "autocomplete calc", dangerous: false },
    Action { name: "Scheme", icon: "palette", desc: "Change the current colour scheme", cmd: "autocomplete scheme", dangerous: false },
    Action { name: "Wallpaper", icon: "image", desc: "Change the current wallpaper", cmd: "autocomplete wallpaper", dangerous: false },
    Action { name: "Variant", icon: "colors", desc: "Change the current scheme variant", cmd: "autocomplete variant", dangerous: false },
    Action { name: "Random", icon: "casino", desc: "Switch to a random wallpaper", cmd: "wallpaper random", dangerous: false },
    Action { name: "Light", icon: "light_mode", desc: "Change the scheme to light mode", cmd: "scheme mode light", dangerous: false },
    Action { name: "Dark", icon: "dark_mode", desc: "Change the scheme to dark mode", cmd: "scheme mode dark", dangerous: false },
    Action { name: "Shutdown", icon: "power_settings_new", desc: "Shutdown the system", cmd: "session shutdown", dangerous: true },
    Action { name: "Reboot", icon: "cached", desc: "Reboot the system", cmd: "session reboot", dangerous: true },
    Action { name: "Logout", icon: "exit_to_app", desc: "Log out of the current session", cmd: "session logout", dangerous: true },
    Action { name: "Lock", icon: "lock", desc: "Lock the current session", cmd: "lock", dangerous: false },
    Action { name: "Sleep", icon: "bedtime", desc: "Suspend then hibernate", cmd: "session suspend", dangerous: false },
    Action { name: "Settings", icon: "settings", desc: "Configure the shell", cmd: "nexus open", dangerous: false },
];

impl Launcher {
    pub fn reset(&self) {
        {
            let mut i = self.inner.lock().unwrap();
            i.query.clear();
            i.selected = 0;
        }
        app().hub.publish("lset", &json!({ "text": "", "n": 0 }));
        self.maybe_reload();
        self.update();
    }

    pub async fn preload(&self) {
        self.load_frecency();
        self.reload();
        self.update();
    }

    fn load_frecency(&self) {
        if let Ok(s) = std::fs::read_to_string(crate::util::state_dir().join("launcher.json"))
            && let Ok(m) = serde_json::from_str::<HashMap<String, u32>>(&s) {
                self.inner.lock().unwrap().frecency = m;
            }
    }

    fn maybe_reload(&self) {
        let stale = {
            let i = self.inner.lock().unwrap();
            !i.loaded
                || i.dir_mtimes.iter().any(|(p, t)| std::fs::metadata(p).and_then(|m| m.modified()).map(|m| &m != t).unwrap_or(true))
        };
        if stale {
            self.reload();
        }
    }

    fn reload(&self) {
        let mut seen = std::collections::HashSet::new();
        let mut apps = vec![];
        let mut mtimes = vec![];
        for d in app_dirs() {
            let Ok(meta) = std::fs::metadata(&d) else { continue };
            if let Ok(m) = meta.modified() {
                mtimes.push((d.clone(), m));
            }
            let mut stack = vec![d.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                        continue;
                    }
                    if p.extension().map(|x| x != "desktop").unwrap_or(true) {
                        continue;
                    }
                    let rel = p.strip_prefix(&d).unwrap_or(&p).to_string_lossy().replace('/', "-");
                    let id = rel.trim_end_matches(".desktop").to_string();
                    // First directory wins (user overrides system).
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    if let Some(entry) = parse_desktop(&p, id) {
                        apps.push(entry);
                    }
                }
            }
        }
        // Hidden apps stay indexed (the Nexus lists them); `update` skips them.
        apps.sort_by_key(|a| a.name.to_lowercase());
        let mut i = self.inner.lock().unwrap();
        i.apps = apps;
        i.loaded = true;
        i.dir_mtimes = mtimes;
    }

    /// Re-run the current query after a config change.
    pub fn refresh(&self) {
        self.update();
    }

    /// Every indexed app for the Nexus "All apps" page.
    pub fn apps_brief(&self) -> Vec<Value> {
        self.maybe_reload();
        let c = crate::app::cfg();
        let i = self.inner.lock().unwrap();
        i.apps
            .iter()
            .enumerate()
            .map(|(k, a)| {
                json!({"i": k, "id": a.id, "name": a.name, "icon": a.icon,
                       "fav": c.launcher.favourite_apps.contains(&a.id), "hidden": c.launcher.hidden_apps.contains(&a.id)})
            })
            .collect()
    }

    /// Icon-theme name for a desktop id / app id (used by notifications).
    pub fn icon_for(&self, id: &str) -> Option<String> {
        let i = self.inner.lock().unwrap();
        let low = id.to_lowercase();
        i.apps
            .iter()
            .find(|a| a.id.to_lowercase() == low || a.wm_class == low || a.id.to_lowercase().ends_with(&format!(".{low}")))
            .map(|a| a.icon.clone())
    }

    /// Caelestia `Icons.getAppCategoryIcon`: Material symbol from categories.
    pub fn category_icon(&self, app_id: &str) -> Option<&'static str> {
        const MAP: &[(&str, &str)] = &[
            ("WebBrowser", "web"),
            ("Printing", "print"),
            ("Security", "security"),
            ("Network", "chat"),
            ("Archiving", "archive"),
            ("Compression", "archive"),
            ("Development", "code"),
            ("IDE", "code"),
            ("TextEditor", "edit_note"),
            ("Audio", "music_note"),
            ("Music", "music_note"),
            ("Player", "music_note"),
            ("Recorder", "mic"),
            ("Game", "sports_esports"),
            ("FileTools", "files"),
            ("FileManager", "files"),
            ("Filesystem", "files"),
            ("FileTransfer", "files"),
            ("Settings", "settings"),
            ("DesktopSettings", "settings"),
            ("HardwareSettings", "settings"),
            ("TerminalEmulator", "terminal"),
            ("ConsoleOnly", "terminal"),
            ("Utility", "build"),
            ("Monitor", "monitor_heart"),
            ("Midi", "graphic_eq"),
            ("Mixer", "graphic_eq"),
            ("AudioVideoEditing", "video_settings"),
            ("AudioVideo", "music_video"),
            ("Video", "videocam"),
            ("Building", "construction"),
            ("Graphics", "photo_library"),
            ("2DGraphics", "photo_library"),
            ("RasterGraphics", "photo_library"),
            ("TV", "tv"),
            ("System", "host"),
            ("Office", "content_paste"),
        ];
        let i = self.inner.lock().unwrap();
        let low = app_id.to_lowercase();
        let e = i.apps.iter().find(|a| {
            let id = a.id.to_lowercase();
            id == low || a.wm_class == low || id.ends_with(&format!(".{low}")) || id.rsplit('.').next() == Some(low.as_str())
        })?;
        MAP.iter().find(|(c, _)| e.categories.iter().any(|x| x == c)).map(|(_, i)| *i)
    }

    fn update(&self) {
        let (query, prefix, max) = {
            let c = crate::app::cfg();
            (self.inner.lock().unwrap().query.clone(), c.launcher.action_prefix.clone(), c.launcher.max_shown)
        };
        let dangerous_ok = crate::app::cfg().launcher.enable_dangerous_actions;
        let fz = crate::app::cfg().launcher.use_fuzzy.clone();
        let mut items: Vec<Value> = vec![];
        let mut mode = "apps".to_string();
        let mut wallpapers: Vec<Value> = vec![];

        if let Some(rest) = query.strip_prefix(&prefix) {
            let (word, arg) = rest.split_once(' ').map(|(a, b)| (a.to_lowercase(), b.to_string())).unwrap_or((rest.to_lowercase(), String::new()));
            let has_arg = rest.contains(' ');
            match (word.as_str(), has_arg) {
                ("calc", true) => {
                    mode = "calc".into();
                    items.push(json!({"kind": "calc", "name": arg.trim(), "desc": "Calculating…", "matIcon": "calculate", "id": arg.trim()}));
                    let a = arg.clone();
                    tokio::spawn(async move { app().launcher.finish_calc(a).await });
                }
                ("scheme", true) => {
                    mode = "schemes".into();
                    let q = arg.to_lowercase();
                    let cur = app().theme.current_scheme_id();
                    let mut v: Vec<(i64, Value)> = crate::theme::list_schemes()
                        .into_iter()
                        .filter_map(|(name, flavour)| {
                            let label = format!("{name} {flavour}");
                            let s = sc(&q, &label, fz.schemes)?;
                            let id = format!("{name} {flavour}");
                            let colours = crate::theme::scheme_preview(&name, &flavour);
                            Some((s, json!({"kind": "scheme", "name": name, "desc": flavour, "id": id, "matIcon": "palette",
                                             "current": id == cur, "colours": colours})))
                        })
                        .collect();
                    v.sort_by_key(|x| std::cmp::Reverse(x.0));
                    items = v.into_iter().map(|x| x.1).collect();
                }
                ("variant", true) => {
                    mode = "variants".into();
                    let q = arg.to_lowercase();
                    let cur = app().theme.current_variant();
                    for (id, name, icon, desc) in crate::theme::VARIANTS {
                        if sc(&q, name, fz.variants).is_some() {
                            items.push(json!({"kind": "variant", "name": name, "desc": desc, "id": id, "matIcon": icon, "current": *id == cur}));
                        }
                    }
                }
                ("wallpaper", true) => {
                    mode = "wallpapers".into();
                    let q = arg.to_lowercase();
                    let max_w = crate::app::cfg().launcher.max_wallpapers;
                    let cur = app().theme.current_wallpaper();
                    wallpapers = crate::theme::list_wallpapers()
                        .into_iter()
                        .filter(|p| sc(&q, &p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), fz.wallpapers).is_some())
                        .take(max_w.max(1) * 4)
                        .map(|p| {
                            let ps = p.to_string_lossy().into_owned();
                            json!({"kind": "wallpaper", "id": ps, "name": p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                                   "thumb": crate::theme::thumbnail_path(&p).to_string_lossy(), "current": ps == cur})
                        })
                        .collect();
                    crate::theme::spawn_thumbnails();
                }
                _ => {
                    mode = "actions".into();
                    let q = rest.to_lowercase();
                    let mut v: Vec<(i64, Value)> = ACTIONS
                        .iter()
                        .filter(|a| dangerous_ok || !a.dangerous)
                        .filter_map(|a| {
                            let s = sc(&q, a.name, fz.actions)?;
                            Some((s, json!({"kind": "action", "name": a.name, "desc": a.desc, "matIcon": a.icon, "id": a.cmd})))
                        })
                        .collect();
                    if !q.is_empty() {
                        v.sort_by_key(|x| std::cmp::Reverse(x.0));
                    }
                    items = v.into_iter().map(|x| x.1).collect();
                }
            }
        } else {
            let q = query.trim().to_lowercase();
            let i = self.inner.lock().unwrap();
            let favs = crate::app::cfg().launcher.favourite_apps.clone();
            let hidden = crate::app::cfg().launcher.hidden_apps.clone();
            let mut v: Vec<(i64, &Entry)> = i
                .apps
                .iter()
                .filter(|a| !hidden.contains(&a.id))
                .filter_map(|a| {
                    let s = if q.is_empty() {
                        0
                    } else {
                        let f = fz.apps;
                        let s1 = sc(&q, &a.name, f).map(|s| s + 300);
                        let s2 = sc(&q, &a.generic, f).map(|s| s / 2);
                        let s3 = sc(&q, &a.keywords, f).map(|s| s / 3);
                        let s4 = sc(&q, &a.id, f).map(|s| s / 3);
                        s1.max(s2).max(s3).max(s4)?
                    };
                    let fav = if favs.contains(&a.id) { 400 } else { 0 };
                    let fr = *i.frecency.get(&a.id).unwrap_or(&0) as i64;
                    Some((s + fav + fr.min(40) * 6, a))
                })
                .collect();
            v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase())));
            items = v
                .into_iter()
                .take(64)
                .map(|(_, a)| {
                    json!({"kind": "app", "name": a.name, "desc": if a.comment.is_empty() { &a.generic } else { &a.comment },
                           "icon": a.icon, "id": a.id, "matIcon": "apps"})
                })
                .collect();
        }
        let count = if mode == "wallpapers" { wallpapers.len() } else { items.len() };
        let mut i = self.inner.lock().unwrap();
        i.count = count;
        if i.selected >= count {
            i.selected = count.saturating_sub(1);
        }
        let sel = i.selected;
        for (k, it) in items.iter_mut().enumerate() {
            if it.get("current").is_none() {
                it["current"] = Value::Bool(false);
            }
            if it.get("colours").is_none() {
                it["colours"] = json!([]);
            }
            it["sel"] = Value::Bool(k == sel);
            it["idx"] = Value::from(k);
        }
        for (k, it) in wallpapers.iter_mut().enumerate() {
            it["sel"] = Value::Bool(k == sel);
            it["idx"] = Value::from(k);
        }
        i.items = items.clone();
        i.mode = mode.clone();
        i.wallpapers = wallpapers.clone();
        // Visible window of items (scroll follows the selection).
        let start = if sel >= max { sel + 1 - max } else { 0 };
        let shown: Vec<Value> = items.iter().skip(start).take(max).cloned().collect();
        let wmax = crate::app::cfg().launcher.max_wallpapers.max(1);
        let wstart = sel.saturating_sub(wmax / 2).min(wallpapers.len().saturating_sub(wmax));
        let wshown: Vec<Value> = wallpapers.iter().skip(wstart).take(wmax).cloned().collect();
        let v = json!({
            "query": i.query,
            "mode": mode,
            "items": shown,
            "count": count,
            "selected": sel,
            "wallpapers": wshown,
            "empty": count == 0,
            "prefix": prefix,
        });
        drop(i);
        app().hub.publish("launcher", &v);
    }

    async fn finish_calc(&self, expr: String) {
        let r = crate::calc::eval(&expr).await;
        let cur = self.inner.lock().unwrap().query.clone();
        if !cur.ends_with(&expr) {
            return; // outdated
        }
        let mut v: Value = app().hub.get("launcher").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        if let Some(items) = v["items"].as_array_mut()
            && let Some(it) = items.first_mut() {
                match &r {
                    Some(res) => {
                        it["name"] = Value::from(res.clone());
                        it["desc"] = Value::from(format!("{} = {}", expr.trim(), res));
                        it["id"] = Value::from(res.clone());
                    }
                    None => {
                        it["desc"] = Value::from("Invalid expression");
                    }
                }
            }
        if let Some(items) = self.inner.lock().unwrap().items.first_mut()
            && let Some(res) = &r {
                items["id"] = Value::from(res.clone());
            }
        app().hub.publish("launcher", &v);
    }

    async fn activate(&self, idx: usize) -> Result<()> {
        let (mode, item, prefix) = {
            let i = self.inner.lock().unwrap();
            let item = if i.mode == "wallpapers" { i.wallpapers.get(idx).cloned() } else { i.items.get(idx).cloned() };
            (i.mode.clone(), item, crate::app::cfg().launcher.action_prefix.clone())
        };
        let Some(item) = item else { return Ok(()) };
        let id = item["id"].as_str().unwrap_or("").to_string();
        match item["kind"].as_str().unwrap_or("") {
            "app" => {
                let entry = self.inner.lock().unwrap().apps.iter().find(|a| a.id == id).cloned();
                if let Some(e) = entry {
                    {
                        let mut i = self.inner.lock().unwrap();
                        *i.frecency.entry(e.id.clone()).or_default() += 1;
                        if let Ok(s) = serde_json::to_string(&i.frecency) {
                            let _ = crate::util::write_atomic(&crate::util::state_dir().join("launcher.json"), s.as_bytes());
                        }
                    }
                    let cmd = clean_exec(&e.exec);
                    let cmd = if e.terminal {
                        format!("{} -e {cmd}", crate::app::cfg().general.terminal)
                    } else {
                        cmd
                    };
                    app().ui.close("launcher").await?;
                    crate::util::spawn_sh(&cmd);
                }
            }
            "action" => {
                if let Some(word) = id.strip_prefix("autocomplete ") {
                    self.set_query(format!("{prefix}{word} "));
                    return Ok(());
                }
                app().ui.close("launcher").await?;
                let args: Vec<String> = id.split_whitespace().map(str::to_string).collect();
                Box::pin(crate::server::dispatch(args)).await?;
            }
            "calc" => {
                // Copy the result to the clipboard, like Caelestia.
                crate::util::spawn_argv(&["wl-copy", &id]);
                app().ui.close("launcher").await?;
            }
            "scheme" => {
                let mut a = vec!["scheme".to_string(), "set".to_string()];
                a.extend(id.split_whitespace().map(str::to_string));
                Box::pin(crate::server::dispatch(a)).await?;
                self.update();
            }
            "variant" => {
                Box::pin(crate::server::dispatch(vec!["scheme".into(), "variant".into(), id])).await?;
                self.update();
            }
            "wallpaper" => {
                app().ui.close("launcher").await?;
                app().theme.set_wallpaper(&id).await?;
            }
            _ => {}
        }
        let _ = mode;
        Ok(())
    }

    /// Programmatic change of the search text (Tab completion, clear). This is
    /// the only time the entry's text is pushed to EWW: binding the entry to
    /// the live query would fight the user's typing.
    fn set_query(&self, q: String) {
        {
            let mut i = self.inner.lock().unwrap();
            i.query = q.clone();
            i.selected = 0;
        }
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        app().hub.publish("lset", &json!({ "text": q, "n": n }));
        self.update();
    }

    pub async fn command(&self, sub: &str, rest: &[String]) -> Result<String> {
        let arg = rest.first().cloned().unwrap_or_default();
        match sub {
            "query" => {
                let mut nav: i64 = 0;
                let mut esc = false;
                let mut tab = false;
                let clean: String = arg
                    .chars()
                    .filter(|c| match *c {
                        KEY_ESC => {
                            esc = true;
                            false
                        }
                        KEY_UP => {
                            nav -= 1;
                            false
                        }
                        KEY_DOWN => {
                            nav += 1;
                            false
                        }
                        KEY_TAB => {
                            tab = true;
                            false
                        }
                        _ => true,
                    })
                    .collect();
                if esc {
                    app().ui.close("launcher").await?;
                    return Ok(String::new());
                }
                let changed = {
                    let mut i = self.inner.lock().unwrap();
                    let changed = i.query != clean;
                    i.query = clean.clone();
                    if changed {
                        i.selected = 0;
                    }
                    let count = i.count.max(1) as i64;
                    if nav != 0 {
                        i.selected = ((i.selected as i64 + nav).rem_euclid(count)) as usize;
                    }
                    changed || nav != 0
                };
                if tab {
                    // Tab autocompletes the selected action, as in Caelestia.
                    let item = {
                        let i = self.inner.lock().unwrap();
                        i.items.get(i.selected).cloned()
                    };
                    if let Some(it) = item
                        && let Some(word) = it["id"].as_str().and_then(|s| s.strip_prefix("autocomplete ")) {
                            let p = crate::app::cfg().launcher.action_prefix.clone();
                            self.set_query(format!("{p}{word} "));
                            return Ok(String::new());
                        }
                }
                if changed || clean != arg {
                    self.update();
                }
            }
            "key" => {
                // From the patched EWW input (`:onkeypress`).
                let max = crate::app::cfg().launcher.max_shown.max(1) as i64;
                let step = match arg.as_str() {
                    "Escape" => {
                        app().ui.close("launcher").await?;
                        return Ok(String::new());
                    }
                    "Up" => -1,
                    "Down" => 1,
                    // Ctrl+hjkl (launcher.vimKeybinds), reported by the patched EWW input.
                    "C-k" | "C-h" if crate::app::cfg().launcher.vim_keybinds => -1,
                    "C-j" | "C-l" if crate::app::cfg().launcher.vim_keybinds => 1,
                    "Page_Up" => -max,
                    "Page_Down" => max,
                    "Tab" | "ISO_Left_Tab" => {
                        let marker = KEY_TAB.to_string();
                        let q = self.inner.lock().unwrap().query.clone();
                        return Box::pin(self.command("query", &[format!("{q}{marker}")])).await;
                    }
                    _ => 0,
                };
                {
                    let mut i = self.inner.lock().unwrap();
                    let count = i.count.max(1) as i64;
                    i.selected = (i.selected as i64 + step).clamp(0, count - 1) as usize;
                }
                self.update();
            }
            "accept" => {
                let sel = self.inner.lock().unwrap().selected;
                self.activate(sel).await?;
            }
            "activate" => {
                let idx: usize = arg.parse().unwrap_or(0);
                self.activate(idx).await?;
            }
            "select" => {
                self.inner.lock().unwrap().selected = arg.parse().unwrap_or(0);
                self.update();
            }
            "move" => {
                {
                    let mut i = self.inner.lock().unwrap();
                    let count = i.count.max(1) as i64;
                    let d = if arg == "up" || arg == "left" { -1 } else { 1 };
                    i.selected = ((i.selected as i64 + d).rem_euclid(count)) as usize;
                }
                self.update();
            }
            "set" => self.set_query(rest.join(" ")),
            "reload" => {
                self.reload();
                self.update();
            }
            _ => bail!("launcher query|accept|activate <i>|select <i>|move up|down|set <text>|reload"),
        }
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_ranking() {
        let exact = score("firefox", "Firefox").unwrap();
        let prefix = score("fire", "Firefox").unwrap();
        let word = score("fox", "Fire Fox").unwrap();
        let sub = score("ffx", "Firefox").unwrap();
        assert!(exact > prefix && prefix > word && word > sub);
        assert!(score("zzz", "Firefox").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }
    #[test]
    fn exec_field_codes() {
        assert_eq!(clean_exec("firefox %u"), "firefox");
        assert_eq!(clean_exec("app --flag %F --x"), "app --flag --x");
        assert_eq!(clean_exec("echo 100%%"), "echo 100%");
    }
}

//! Panel state machine. Windows are only mapped while a panel is visible
//! (closed windows cost no memory and never steal input). Opening = map the
//! window with its revealer collapsed, then flip the reveal flag so GTK
//! animates it out of the screen border; closing = collapse, wait for the
//! animation, unmap.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::app::app;

/// Panels that grab the screen (close on outside click, mutually exclusive).
const MODAL: &[&str] = &["launcher", "session", "sidebar", "utilities"];
const ALL: &[&str] = &["launcher", "dashboard", "session", "sidebar", "utilities", "popout", "osd"];
/// Revealer transition length used in the SCSS/yuck (keep in sync).
const ANIM_MS: u64 = 400;

#[derive(Default, Clone)]
struct Panel {
    shown: bool,
    mapped: bool,
    generation: u64,
    via_hover: bool,
}

#[derive(Default)]
struct State {
    panels: HashMap<&'static str, Panel>,
    catcher_mapped: bool,
    dash_tab: u8,
    popout: String,
    popout_geom: (i32, bool), // (wrapper height, anchored to bottom)
    outputs: Vec<(String, i32, i32, bool)>, // name, width, height, active
    fullscreen: bool,
    static_open: Vec<String>,
    misc: serde_json::Map<String, Value>,
    hover_gen: HashMap<String, u64>,
    osd_gen: u64,
}

#[derive(Default)]
pub struct Ui {
    st: Mutex<State>,
    quit: Notify,
}

fn key(name: &str) -> Option<&'static str> {
    ALL.iter().copied().find(|p| *p == name)
}

impl Ui {
    pub fn publish(&self) {
        let st = self.st.lock().unwrap();
        let mut v = json!({
            "dashTab": st.dash_tab,
            "popoutName": st.popout,
            "popoutH": st.popout_geom.0,
            "popoutBottom": st.popout_geom.1,
            "fullscreen": st.fullscreen,
            "modal": MODAL.iter().any(|p| st.panels.get(p).map(|x| x.shown).unwrap_or(false)),
        });
        for p in ALL {
            v[*p] = Value::Bool(st.panels.get(p).map(|x| x.shown).unwrap_or(false));
        }
        for (k, val) in &st.misc {
            v[k] = val.clone();
        }
        drop(st);
        app().hub.publish("ui", &v);
    }

    pub fn set_misc(&self, k: &str, v: Value) {
        self.st.lock().unwrap().misc.insert(k.to_string(), v);
        self.publish();
    }

    pub fn request_quit(&self) {
        self.quit.notify_one();
    }
    pub async fn quit_requested(&self) {
        self.quit.notified().await
    }

    pub fn is_shown(&self, name: &str) -> bool {
        self.st.lock().unwrap().panels.get(name).map(|p| p.shown).unwrap_or(false)
    }

    pub fn active_output(&self) -> Option<String> {
        let st = self.st.lock().unwrap();
        st.outputs.iter().find(|o| o.3).or(st.outputs.first()).map(|o| o.0.clone())
    }

    fn active_height(&self) -> i32 {
        self.active_size().1
    }

    fn active_size(&self) -> (i32, i32) {
        let st = self.st.lock().unwrap();
        st.outputs
            .iter()
            .find(|o| o.3)
            .or(st.outputs.first())
            .map(|o| (o.1, o.2))
            .filter(|s| s.0 > 0 && s.1 > 0)
            .unwrap_or((1920, 1080))
    }

    /// Usable area (screen minus bar and border), in logical px.
    fn usable(&self) -> (i32, i32) {
        let (w, h) = self.active_size();
        let t = crate::app::cfg().border.thickness as i32;
        let bar = 40 + 2 * t.max(8);
        (w - bar - t, h - 2 * t)
    }

    pub fn set_dash_tab(&self, t: u8) {
        self.st.lock().unwrap().dash_tab = t.min(3);
        self.publish();
        self.update_demand();
    }

    fn update_demand(&self) {
        let (dash, tab, lock_like) = {
            let st = self.st.lock().unwrap();
            let dash = st.panels.get("dashboard").map(|p| p.shown).unwrap_or(false);
            let util = st.panels.get("utilities").map(|p| p.shown).unwrap_or(false);
            (dash, st.dash_tab, util)
        };
        let hub = &app().hub;
        hub.set_demand("sys", dash);
        hub.set_demand("media-pos", dash && (tab == 0 || tab == 1));
        hub.set_demand("info", dash);
        let _ = lock_like;
        app().sys.wake();
        app().info.wake();
    }

    // ---- static windows (bar + border frame), one set per output ----

    /// Called by the WM service whenever the output list changes.
    pub async fn set_outputs(&self, outs: Vec<(String, i32, i32, bool)>) {
        let changed = {
            let mut st = self.st.lock().unwrap();
            let names: Vec<_> = outs.iter().map(|o| (&o.0, o.1, o.2)).collect();
            let old: Vec<_> = st.outputs.iter().map(|o| (&o.0, o.1, o.2)).collect();
            let changed = names != old;
            st.outputs = outs;
            changed
        };
        if changed {
            self.open_static().await;
        }
    }

    pub async fn set_fullscreen(&self, fs: bool) {
        {
            let mut st = self.st.lock().unwrap();
            if st.fullscreen == fs {
                return;
            }
            st.fullscreen = fs;
        }
        self.publish();
        if fs {
            self.close_all().await;
        }
        self.open_static().await;
    }

    /// (Re)open the always-present windows: the bar and the border frame.
    pub async fn open_static(&self) {
        let eww = &app().eww;
        let (outs, fs, old) = {
            let st = self.st.lock().unwrap();
            (st.outputs.clone(), st.fullscreen, st.static_open.clone())
        };
        let old_refs: Vec<&str> = old.iter().map(String::as_str).collect();
        eww.close(&old_refs).await;
        let mut opened = vec![];
        if !fs {
            let outs = if outs.is_empty() { vec![(String::new(), 0, 0, true)] } else { outs };
            for (i, o) in outs.iter().enumerate() {
                let screen = if o.0.is_empty() { None } else { Some(o.0.as_str()) };
                for w in ["border-top", "border-right", "border-bottom", "bar", "corner-tl", "corner-tr", "corner-bl", "corner-br"]
                {
                    let id = format!("{w}-{i}");
                    let c = crate::app::cfg();
                    let args = [
                        ("monitor", o.0.clone()),
                        ("thickness", c.border.thickness.to_string()),
                        ("rounding", c.border.rounding.to_string()),
                    ];
                    eww.open(w, &id, screen, &args).await;
                    opened.push(id);
                }
            }
        }
        self.st.lock().unwrap().static_open = opened;
        // Popup windows are positioned per active output; remap if needed.
        let notif_mapped = app().notifs.has_popups();
        if notif_mapped {
            app().notifs.remap().await;
        }
    }

    // ---- panel show/hide ----

    pub async fn toggle(&self, name: &str) -> Result<String> {
        let Some(k) = key(name) else { bail!("unknown panel {name:?}") };
        if self.is_shown(k) { self.hide(k).await } else { self.show(k, false).await }
        Ok(String::new())
    }

    pub async fn open(&self, name: &str) -> Result<String> {
        let Some(k) = key(name) else { bail!("unknown panel {name:?}") };
        self.show(k, false).await;
        Ok(String::new())
    }

    pub async fn close(&self, name: &str) -> Result<String> {
        let Some(k) = key(name) else { bail!("unknown panel {name:?}") };
        self.hide(k).await;
        Ok(String::new())
    }

    pub async fn close_all(&self) {
        for p in ALL {
            if *p != "osd" && self.is_shown(p) {
                self.hide(p).await;
            }
        }
    }

    fn window_for(name: &str) -> &'static str {
        match name {
            "launcher" => "launcher",
            "dashboard" => "dashboard",
            "session" => "session",
            "sidebar" => "sidebar",
            "utilities" => "utilities",
            "popout" => "popout",
            "osd" => "osd",
            _ => "launcher",
        }
    }

    pub async fn show(&self, k: &'static str, via_hover: bool) {
        if self.st.lock().unwrap().fullscreen && k != "osd" {
            return;
        }
        if MODAL.contains(&k) {
            for other in MODAL {
                if *other != k && self.is_shown(other) {
                    self.hide(other).await;
                }
            }
            if self.is_shown("popout") {
                self.hide("popout").await;
            }
        }
        let need_map = {
            let mut st = self.st.lock().unwrap();
            let p = st.panels.entry(k).or_default();
            p.generation += 1;
            p.via_hover = via_hover;
            if p.shown && p.mapped {
                return;
            }
            !p.mapped
        };
        if k == "launcher" {
            app().launcher.reset();
        }
        if need_map {
            let screen = self.active_output();
            let (uw, uh) = self.usable();
            let mut args: Vec<(&str, String)> = vec![];
            match k {
                "popout" | "sidebar" => args.push(("h", uh.to_string())),
                "launcher" => args.push(("maxh", (uh * 7 / 10).to_string())),
                _ => {}
            }
            let _ = uw;
            app().eww.open(Self::window_for(k), Self::window_for(k), screen.as_deref(), &args).await;
            self.st.lock().unwrap().panels.entry(k).or_default().mapped = true;
            // Let GTK map the window with the revealer collapsed first.
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.st.lock().unwrap().panels.entry(k).or_default().shown = true;
        self.sync_catcher().await;
        self.publish();
        self.update_demand();
    }

    pub async fn hide(&self, k: &'static str) {
        let generation = {
            let mut st = self.st.lock().unwrap();
            let p = st.panels.entry(k).or_default();
            if !p.shown && !p.mapped {
                return;
            }
            p.shown = false;
            p.generation += 1;
            p.generation
        };
        self.publish();
        self.update_demand();
        self.sync_catcher().await;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ANIM_MS + 50)).await;
            let ui = &app().ui;
            let unmap = {
                let mut st = ui.st.lock().unwrap();
                let p = st.panels.entry(k).or_default();
                if p.generation == generation && !p.shown && p.mapped {
                    p.mapped = false;
                    true
                } else {
                    false
                }
            };
            if unmap {
                app().eww.close(&[Ui::window_for(k)]).await;
            }
        });
    }

    /// The catcher is a transparent full-screen window (respecting exclusive
    /// zones, so it never covers the bar) that closes modal panels when you
    /// click outside them — the equivalent of Caelestia's focus grab.
    async fn sync_catcher(&self) {
        let (want, mapped, dim) = {
            let st = self.st.lock().unwrap();
            let shown = |p: &str| st.panels.get(p).map(|x| x.shown).unwrap_or(false);
            let dash_click = st.panels.get("dashboard").map(|x| x.shown && !x.via_hover).unwrap_or(false);
            (MODAL.iter().any(|p| shown(p)) || dash_click, st.catcher_mapped, shown("session"))
        };
        self.set_misc_silent("scrim", Value::Bool(dim));
        if want && !mapped {
            let screen = self.active_output();
            let (uw, uh) = self.usable();
            app().eww.open("catcher", "catcher", screen.as_deref(), &[("w", uw.to_string()), ("h", uh.to_string())]).await;
            self.st.lock().unwrap().catcher_mapped = true;
        } else if !want && mapped {
            self.st.lock().unwrap().catcher_mapped = false;
            app().eww.close(&["catcher"]).await;
        }
    }

    fn set_misc_silent(&self, k: &str, v: Value) {
        self.st.lock().unwrap().misc.insert(k.to_string(), v);
    }

    // ---- hover driven panels (dashboard on top edge, bar popouts) ----

    pub async fn hover(&self, what: &str, extra: &str) {
        let generation = {
            let mut st = self.st.lock().unwrap();
            let g = st.hover_gen.entry(hover_key(what)).or_default();
            *g += 1;
            *g
        };
        let _ = generation;
        match what {
            "dashboard" => {
                if crate::app::cfg().dashboard.show_on_hover && !self.is_shown("dashboard") {
                    self.show("dashboard", true).await;
                }
            }
            "popout" => {
                // keep current popout open (pointer moved onto it)
            }
            item if item.starts_with("popout:") => {
                let name = item.trim_start_matches("popout:").to_string();
                let _ = extra;
                let geom = self.popout_geometry(&name);
                {
                    let mut st = self.st.lock().unwrap();
                    st.popout = name;
                    st.popout_geom = geom;
                }
                if name_needs_data(&self.st.lock().unwrap().popout) {
                    app().net.set_list_demand(self.st.lock().unwrap().popout == "network");
                    if self.st.lock().unwrap().popout == "battery" {
                        tokio::spawn(async { app().bat.refresh_profile().await });
                    }
                }
                if self.is_shown("popout") {
                    self.publish();
                } else {
                    self.show("popout", true).await;
                }
            }
            _ => {}
        }
    }

    pub async fn unhover(&self, what: &str) {
        let k = hover_key(what);
        let generation = {
            let mut st = self.st.lock().unwrap();
            let g = st.hover_gen.entry(k.clone()).or_default();
            *g += 1;
            *g
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(180)).await;
            let ui = &app().ui;
            if ui.st.lock().unwrap().hover_gen.get(&k).copied() != Some(generation) {
                return; // re-hovered meanwhile
            }
            match k.as_str() {
                "dashboard" => {
                    let via_hover = ui.st.lock().unwrap().panels.get("dashboard").map(|p| p.via_hover).unwrap_or(false);
                    if via_hover {
                        ui.hide("dashboard").await;
                    }
                }
                "popout" => {
                    app().net.set_list_demand(false);
                    ui.hide("popout").await;
                }
                _ => {}
            }
        });
    }

    /// Compute where a bar popout should sit so that it is centred on the
    /// hovered bar item. The bar layout uses fixed metrics (see _bar.scss).
    fn popout_geometry(&self, name: &str) -> (i32, bool) {
        let h = self.active_height();
        let t = crate::app::cfg().border.thickness as i32;
        let usable = h - 2 * t;
        // Bar metrics (keep in sync with _bar.scss):
        const PAD: i32 = 16; // vertical padding of the bar
        const POWER: i32 = 32; // power button
        const GAP: i32 = 12; // spacing between groups
        const PILL_PAD: i32 = 12; // status pill padding
        const SLOT: i32 = 30; // status icon slot
        let center_from_top = match name {
            "activewindow" => usable / 2,
            _ => {
                let icons = app().status_icon_order();
                let idx = icons.iter().position(|i| i == name).unwrap_or(0) as i32;
                let below = icons.len() as i32 - 1 - idx;
                let from_bottom = PAD + POWER + GAP + PILL_PAD + below * SLOT + SLOT / 2;
                // Bar spans the whole screen height; the popout window starts
                // below the top border strip.
                h - from_bottom - t
            }
        };
        if center_from_top * 2 <= usable {
            (center_from_top * 2, false)
        } else {
            ((usable - center_from_top) * 2, true)
        }
    }

    // ---- OSD ----

    pub async fn osd(&self) {
        if !crate::app::cfg().osd.enabled || self.is_shown("session") || self.is_shown("sidebar") {
            return;
        }
        let generation = {
            let mut st = self.st.lock().unwrap();
            st.osd_gen += 1;
            st.osd_gen
        };
        if !self.is_shown("osd") {
            self.show("osd", false).await;
        }
        let delay = crate::app::cfg().osd.hide_delay;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let ui = &app().ui;
            if ui.st.lock().unwrap().osd_gen == generation {
                ui.hide("osd").await;
            }
        });
    }
}

fn hover_key(what: &str) -> String {
    if what.starts_with("popout") { "popout".into() } else { what.to_string() }
}

fn name_needs_data(_n: &str) -> bool {
    true
}

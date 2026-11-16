//! Nexus — the settings panel (Caelestia `modules/nexus`).
//!
//! The daemon owns the navigation state and generates each page as a small
//! yuck snippet rendered by an EWW `literal`. Pages bind their values to the
//! `cfg`/`status`/`nexus` variables, so flipping a switch only updates the
//! bound property: the snippet (and its widgets) is rebuilt only when the
//! page changes. No user- or app-provided text is ever placed inside the
//! snippet; dynamic lists are addressed by index.

use std::sync::Mutex;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::app::app;

pub struct PageDef {
    pub label: &'static str,
    pub icon: &'static str,
    pub desc: &'static str,
    pub no_fill: bool,
    /// First page of a nav-list category group.
    pub first: bool,
    /// Last page of a nav-list category group.
    pub last: bool,
}

const fn pd(label: &'static str, icon: &'static str, desc: &'static str, no_fill: bool, first: bool, last: bool) -> PageDef {
    PageDef { label, icon, desc, no_fill, first, last }
}

/// Same order/categories as Caelestia's PageRegistry.
pub const PAGES: &[PageDef] = &[
    pd("Wallpaper & style", "palette", "Wallpaper, fonts, colours", false, true, true),
    pd("Network", "wifi", "Wi-Fi, ethernet, VPN", false, true, false),
    pd("Connected devices", "devices_other", "Bluetooth, pairing", true, false, false),
    pd("Audio", "volume_up", "App volumes, sound devices", false, false, true),
    pd("Updates", "update", "System updates", false, true, false),
    pd("Plugins", "extension", "Manage plugins", false, false, true),
    pd("Panels", "dock_to_bottom", "Dashboard, taskbar, launcher, sidebar", false, true, false),
    pd("Apps", "apps", "Default apps, favourites, hidden apps", false, false, false),
    pd("Services", "build", "Poll intervals, default player", false, false, false),
    pd("Language & region", "globe", "Weather location, display units", false, false, true),
    pd("About", "info", "System information, credits", false, true, true),
];

#[derive(Default)]
struct State {
    page: usize,
    stack: Vec<usize>,
    query: String,
    saved: Vec<Value>,
    vpns: Vec<Value>,
    app_vols: Vec<Value>,
    apps: Vec<Value>,
    walls: Vec<Value>,
    about: Value,
    form_ssid: String,
    form_pw: String,
    form_hidden: bool,
    form_status: String,
}

#[derive(Default)]
pub struct Nexus {
    st: Mutex<State>,
}

// ------------------------------------------------------------------ yuck builders

/// Quote a static string for yuck (labels are compile-time constants).
fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace("${", "$\u{200b}{"))
}

fn head(t: &str) -> String {
    format!("(nx-head :text {})", q(t))
}

fn card(rows: &[String]) -> String {
    format!("(box :class \"nx-card\" :orientation \"v\" :space-evenly false {})", rows.join(" "))
}

fn toggle(key: &str, label: &str, desc: &str) -> String {
    format!("(nx-toggle :label {} :desc {} :on {{cfg.{key} ?: false}} :cmd \"sheels config set {key} toggle\")", q(label), q(desc))
}

fn toggle_x(label: &str, desc: &str, on: &str, cmd: &str) -> String {
    format!("(nx-toggle :label {} :desc {} :on {{{on}}} :cmd {})", q(label), q(desc), q(cmd))
}

/// Integer stepper; `scale` converts the stored value to the shown one
/// (e.g. 100 for a 0..1 fraction shown as %, 0.001 for ms shown as s).
fn step(key: &str, label: &str, desc: &str, min: f64, max: f64, stp: f64, scale: f64) -> String {
    // min/max/step are in stored units.
    let shown = if scale == 1.0 {
        format!("{{cfg.{key}}}")
    } else {
        format!("{{round(cfg.{key} * {scale}, {})}}", if scale >= 1.0 { 0 } else { 1 })
    };
    let (s, lo, hi) = (stp, min, max);
    format!(
        "(nx-stepper :label {} :desc {} :value {shown} :dec \"sheels config step {key} -{s} {lo} {hi}\" :inc \"sheels config step {key} {s} {lo} {hi}\")",
        q(label),
        q(desc)
    )
}

/// Select (split button) cycling through `opts` = (value, text, icon).
fn select(key: &str, label: &str, desc: &str, opts: &[(&str, &str, &str)]) -> String {
    let mut text = q(opts[0].1).replace('"', "'");
    let mut icon = q(opts[0].2).replace('"', "'");
    for (v, t, i) in opts.iter().rev() {
        text = format!("cfg.{key} == '{v}' ? '{t}' : {text}");
        icon = format!("cfg.{key} == '{v}' ? '{i}' : {icon}");
    }
    let values: Vec<&str> = opts.iter().map(|o| o.0).collect();
    format!(
        "(nx-select :label {} :desc {} :text {{{text}}} :icon {{{icon}}} :cmd \"sheels config cycle {key} {}\")",
        q(label),
        q(desc),
        values.join(",")
    )
}

fn entry(key: &str, label: &str, desc: &str, placeholder: &str) -> String {
    format!(
        "(nx-entry :label {} :desc {} :value {{cfg.{key}}} :hint {} :cmd \"sheels config set {key} - <<'__SHEELS_EOF__'\n{{}}\n__SHEELS_EOF__\")",
        q(label),
        q(desc),
        q(placeholder)
    )
}

fn nav(icon: &str, label: &str, desc: &str, sub: usize) -> String {
    format!("(nx-nav :icon {} :label {} :desc {} :cmd \"sheels nexus sub {sub}\")", q(icon), q(label), q(desc))
}

/// Nav row whose description is a yuck expression.
fn nav_x(icon: &str, label: &str, desc_expr: &str, sub: usize) -> String {
    format!("(nx-nav :icon {} :label {} :desc {{{desc_expr}}} :cmd \"sheels nexus sub {sub}\")", q(icon), q(label))
}

fn info_row(label: &str, expr: &str) -> String {
    format!("(nx-info :label {} :value {{{expr}}})", q(label))
}

fn enabled_desc(key: &str) -> String {
    format!("cfg.{key} ? 'Enabled' : 'Disabled'")
}

fn page(parts: &[String]) -> String {
    format!("(box :class \"nx-page\" :orientation \"v\" :space-evenly false {})", parts.join(" "))
}

fn placeholder() -> String {
    "(nx-placeholder)".into()
}

// ------------------------------------------------------------------ pages

fn wallpaper_page(sub: usize) -> (String, String) {
    match sub {
        1 => (
            "Wallpapers".into(),
            page(&[
                "(nx-wall-actions)".into(),
                "(nx-wall-hero)".into(),
                head("Local wallpapers"),
                "(nx-wall-grid)".into(),
            ]),
        ),
        2 => ("Colours".into(), placeholder()),
        _ => (
            "Wallpaper & style".into(),
            page(&[
                "(nx-wall-preview)".into(),
                card(&[
                    toggle("background.enabled", "Display wallpaper", ""),
                    "(nx-toggle :label \"Transparency\" :desc {'Base ' + cfg.appearance.transparencyBase + ', layers ' + cfg.appearance.transparencyLayers} :on {cfg.appearance.transparency} :cmd \"sheels config set appearance.transparency toggle\")".to_string(),
                    toggle_x("Dark theme", "", "!theme.light", "sheels scheme mode toggle"),
                ]),
            ]),
        ),
    }
}

fn network_page(sub: usize) -> (String, String) {
    match sub {
        1 => ("Saved networks".into(), page(&["(nx-saved-networks)".into()])),
        2 => ("Add network".into(), page(&["(nx-add-network)".into()])),
        _ => ("Network".into(), page(&["(nx-wifi-card)".into(), "(nx-vpn-card)".into()])),
    }
}

fn audio_page(sub: usize) -> (String, String) {
    match sub {
        1 => ("App volumes".into(), page(&["(nx-app-volumes)".into()])),
        _ => ("Audio".into(), page(&["(nx-audio-card :mic false)".into(), "(nx-audio-card :mic true)".into(), "(nx-app-volumes-row)".into()])),
    }
}

fn panels_page(sub: usize) -> (String, String) {
    let t = |s: &str| s.to_string();
    match sub {
        1 => (
            t("Dashboard"),
            page(&[
                head("General"),
                card(&[
                    toggle("dashboard.enabled", "Enabled", ""),
                    toggle("dashboard.showOnHover", "Show on hover", "Reveal when the cursor reaches the screen edge"),
                    toggle("dashboard.showClockSeconds", "Show clock seconds", "Display seconds for the clock in the main panel"),
                ]),
                head("Tabs"),
                card(&[
                    toggle("dashboard.showDashboard", "Dashboard", ""),
                    toggle("dashboard.showMedia", "Media", ""),
                    toggle("dashboard.showPerformance", "Performance", ""),
                    toggle("dashboard.showWeather", "Weather", ""),
                ]),
                head("Performance widgets"),
                card(&[
                    toggle("dashboard.performance.showBattery", "Battery", ""),
                    toggle("dashboard.performance.showGpu", "GPU", ""),
                    toggle("dashboard.performance.showCpu", "CPU", ""),
                    toggle("dashboard.performance.showMemory", "Memory", ""),
                    toggle("dashboard.performance.showStorage", "Storage", ""),
                    toggle("dashboard.performance.showNetwork", "Network", ""),
                ]),
            ]),
        ),
        2 => (
            t("Taskbar"),
            page(&[
                head("Behaviour"),
                card(&[
                    toggle("bar.persistent", "Persistent", "Keep the bar visible at all times"),
                    toggle("bar.showOnHover", "Show on hover", "Reveal the bar when the cursor reaches the screen edge"),
                ]),
                head("Components"),
                card(&[
                    nav("workspaces", "Workspaces", "Indicators, window icons", 6),
                    nav("web_asset", "Active window", "Title display, popout", 7),
                    nav("widgets", "Tray", "System tray icons", 8),
                    nav("signal_cellular_alt", "Status icons", "Visible indicators", 9),
                    nav("schedule", "Clock", "Date, icon, background", 10),
                ]),
                head("Scroll actions"),
                card(&[
                    toggle("bar.scrollActions.workspaces", "Workspaces", "Scroll over the workspace indicator to switch workspaces"),
                    toggle("bar.scrollActions.volume", "Volume", "Scroll on the top half of the bar to adjust volume"),
                    toggle("bar.scrollActions.brightness", "Brightness", "Scroll on the bottom half of the bar to adjust brightness"),
                ]),
            ]),
        ),
        3 => (
            t("Launcher"),
            page(&[
                head("General"),
                card(&[
                    toggle("launcher.enabled", "Enabled", ""),
                    entry("launcher.actionPrefix", "Action prefix", "Prefix used to run actions in the launcher", ">"),
                ]),
                head("Display"),
                card(&[
                    step("launcher.maxShown", "Max items shown", "", 1.0, 20.0, 1.0, 1.0),
                    step("launcher.maxWallpapers", "Max wallpapers", "", 1.0, 30.0, 1.0, 1.0),
                ]),
                head("Behaviour"),
                card(&[
                    toggle("launcher.vimKeybinds", "Vim keybinds", "Navigate results with Ctrl+hjkl"),
                    toggle("launcher.enableDangerousActions", "Enable dangerous actions", "Allow actions that shut down or log out"),
                ]),
                head("Fuzzy search"),
                card(&[
                    toggle("launcher.useFuzzy.apps", "Apps", ""),
                    toggle("launcher.useFuzzy.actions", "Actions", ""),
                    toggle("launcher.useFuzzy.schemes", "Schemes", ""),
                    toggle("launcher.useFuzzy.variants", "Variants", ""),
                    toggle("launcher.useFuzzy.wallpapers", "Wallpapers", ""),
                ]),
            ]),
        ),
        4 => (
            t("Sidebar"),
            page(&[head("General"), card(&[toggle("sidebar.enabled", "Enabled", "")])]),
        ),
        5 => (
            t("Utilities"),
            page(&[
                head("General"),
                card(&[toggle("utilities.enabled", "Enabled", "Show the utilities panel")]),
                head("Cards"),
                card(&[
                    toggle("utilities.keepAwake", "Keep awake", "Show the idle inhibitor card"),
                    toggle("utilities.screenRecorder", "Screen recorder", "Show the screen recorder card"),
                    toggle("utilities.quickToggles", "Quick toggles", "Show the quick toggles card"),
                ]),
                head("Quick toggles"),
                card(&[
                    toggle("utilities.toggles.wifi", "Wi-Fi", "Toggle wireless networking"),
                    toggle("utilities.toggles.bluetooth", "Bluetooth", "Toggle the Bluetooth adapter"),
                    toggle("utilities.toggles.mic", "Microphone", "Mute or unmute the default source"),
                    toggle("utilities.toggles.settings", "Settings", "Open the settings window"),
                    toggle("utilities.toggles.gameMode", "Game mode", "Toggle game mode"),
                    toggle("utilities.toggles.dnd", "Do not disturb", "Silence notifications"),
                    toggle("utilities.toggles.vpn", "VPN", "Connect or disconnect the VPN"),
                ]),
            ]),
        ),
        6 => (
            t("Workspaces"),
            page(&[card(&[
                step("bar.workspaces.shown", "Shown", "Number of workspaces displayed", 1.0, 20.0, 1.0, 1.0),
                toggle("bar.workspaces.activeIndicator", "Active indicator", ""),
                toggle("bar.workspaces.occupiedBg", "Occupied background", ""),
                toggle("bar.workspaces.showUnoccupied", "Show unoccupied", "Show workspaces that are inactive and empty"),
                toggle("bar.workspaces.showWindows", "Show windows", "Show icons of open windows on each workspace"),
                step("bar.workspaces.maxWindowIcons", "Max window icons", "", 1.0, 10.0, 1.0, 1.0),
            ])]),
        ),
        7 => (
            t("Active window"),
            page(&[card(&[
                toggle("bar.activeWindow.compact", "Compact", ""),
                toggle("bar.activeWindow.inverted", "Inverted", ""),
                toggle("bar.activeWindow.showOnHover", "Show on hover", "Only show the active window title while hovering"),
                toggle("bar.activeWindow.popoutOnHover", "Popout on hover", "Show a window details popout when hovering"),
            ])]),
        ),
        8 => (
            t("Tray"),
            page(&[card(&[
                toggle("bar.tray.background", "Background", ""),
                toggle("bar.tray.compact", "Compact", ""),
            ])]),
        ),
        9 => (
            t("Status icons"),
            page(&[
                head("Visible icons"),
                "(nx-status-icons)".into(),
                head("Behaviour"),
                card(&[toggle("bar.status.popoutOnHover", "Popout on hover", "Show a details popout when hovering the status icons")]),
            ]),
        ),
        10 => (
            t("Clock"),
            page(&[card(&[
                toggle("bar.clock.background", "Background", ""),
                toggle("bar.clock.showDate", "Show date", ""),
                toggle("bar.clock.showIcon", "Show icon", ""),
                toggle("bar.clock.showSeconds", "Show seconds", ""),
            ])]),
        ),
        _ => (
            t("Panels"),
            page(&[card(&[
                nav_x("space_dashboard", "Dashboard", &enabled_desc("dashboard.enabled"), 1),
                nav_x("bottom_navigation", "Taskbar", "cfg.bar.persistent ? 'Always visible' : 'Shown on hover'", 2),
                nav_x("apps", "Launcher", &enabled_desc("launcher.enabled"), 3),
                nav_x("side_navigation", "Sidebar", &enabled_desc("sidebar.enabled"), 4),
                nav_x("handyman", "Utilities", &enabled_desc("utilities.enabled"), 5),
            ])]),
        ),
    }
}

fn apps_page(sub: usize) -> (String, String) {
    match sub {
        1 => ("All apps".into(), page(&["(nx-all-apps)".into()])),
        _ => (
            "Apps".into(),
            page(&[
                head("Default applications"),
                card(&[
                    "(nx-default-app :icon \"terminal\" :label \"Terminal\" :key \"general.terminal\" :value {cfg.general.terminal})".into(),
                    "(nx-default-app :icon \"volume_up\" :label \"Audio\" :key \"general.audio\" :value {cfg.general.audio})".into(),
                    "(nx-default-app :icon \"play_circle\" :label \"Media playback\" :key \"general.playback\" :value {cfg.general.playback})".into(),
                    "(nx-default-app :icon \"folder\" :label \"File manager\" :key \"general.explorer\" :value {cfg.general.explorer})".into(),
                ]),
                head("Library"),
                card(&[nav("apps", "All apps", "Browse installed apps, set favourites and hidden", 1)]),
            ]),
        ),
    }
}

fn services_page(sub: usize) -> (String, String) {
    match sub {
        1 => (
            "Notifications".into(),
            page(&[
                head("Notifications"),
                card(&[
                    select(
                        "notifs.fullscreen",
                        "Show in fullscreen",
                        "Whether notifications appear over fullscreen apps",
                        &[("on", "On", "notifications_active"), ("critical", "Critical", "priority_high"), ("off", "Off", "notifications_off")],
                    ),
                    toggle("notifs.expire", "Expire automatically", "Dismiss notifications after their timeout"),
                    toggle("notifs.openExpanded", "Open expanded", "Show notifications expanded by default"),
                    step("notifs.defaultExpireTimeout", "Default timeout", "Time before a notification dismisses (ms)", 1000.0, 60000.0, 500.0, 1.0),
                    step("notifs.groupPreviewNum", "Group preview count", "Notifications shown per group before collapsing", 1.0, 10.0, 1.0, 1.0),
                ]),
                head("Toasts"),
                card(&[
                    select(
                        "utilities.toastsFullscreen",
                        "Show in fullscreen",
                        "Whether toasts appear over fullscreen apps",
                        &[("true", "On", "notifications_active"), ("false", "Off", "notifications_off")],
                    )
                    .replace("sheels config cycle utilities.toastsFullscreen true,false", "sheels config set utilities.toastsFullscreen toggle")
                    .replace("cfg.utilities.toastsFullscreen == 'true'", "cfg.utilities.toastsFullscreen")
                    .replace("cfg.utilities.toastsFullscreen == 'false'", "!cfg.utilities.toastsFullscreen"),
                    step("utilities.maxToasts", "Visible toasts", "Maximum number of toasts shown at once", 1.0, 10.0, 1.0, 1.0),
                ]),
                head("Toast events"),
                card(&[
                    toggle("utilities.toasts.chargingChanged", "Charging changes", ""),
                    toggle("utilities.toasts.gameModeChanged", "Game mode changes", ""),
                    toggle("utilities.toasts.dndChanged", "Do not disturb changes", ""),
                    toggle("utilities.toasts.audioOutputChanged", "Audio output changes", ""),
                    toggle("utilities.toasts.audioInputChanged", "Audio input changes", ""),
                    toggle("utilities.toasts.capsLockChanged", "Caps lock changes", ""),
                    toggle("utilities.toasts.numLockChanged", "Num lock changes", ""),
                    toggle("utilities.toasts.kbLayoutChanged", "Keyboard layout changes", ""),
                    toggle("utilities.toasts.vpnChanged", "VPN changes", ""),
                    toggle("utilities.toasts.nowPlaying", "Now playing", ""),
                ]),
            ]),
        ),
        _ => (
            "Services".into(),
            page(&[
                head("Notifications"),
                card(&[nav("notifications", "Notifications", "Notifications, toasts, timeouts", 1)]),
                head("Polling"),
                card(&[
                    step(
                        "dashboard.resourceUpdateInterval",
                        "System stats refresh",
                        "CPU, memory and GPU update interval (seconds)",
                        500.0,
                        10000.0,
                        500.0,
                        0.001,
                    ),
                    step("services.wifiRescanInterval", "Wi-Fi rescan", "How often available networks are rescanned (seconds)", 5.0, 120.0, 5.0, 1.0),
                ]),
                head("Media"),
                card(&[select(
                    "services.defaultPlayer",
                    "Default player",
                    "Preferred media player when several are open",
                    &[("Spotify", "Spotify", "music_note"), ("Firefox", "Firefox", "public"), ("mpv", "mpv", "movie"), ("", "Auto", "auto_awesome")],
                )]),
                head("Input increments"),
                card(&[
                    step("services.audioIncrement", "Volume step", "Amount the volume changes per scroll (%)", 0.01, 0.5, 0.01, 100.0),
                    step("services.brightnessIncrement", "Brightness step", "Amount the brightness changes per scroll (%)", 0.01, 0.5, 0.01, 100.0),
                    step("services.maxVolume", "Max volume", "Upper limit for output volume (%)", 0.5, 2.0, 0.05, 100.0),
                ]),
                head("Service tuning"),
                card(&[
                    toggle("services.smartScheme", "Smart colour scheme", "Derive theme mode and variant from the wallpaper"),
                    select(
                        "services.gpuType",
                        "GPU",
                        "GPU vendor used for usage and temperature",
                        &[("auto", "Auto", "auto_awesome"), ("nvidia", "NVIDIA", "memory"), ("amd", "AMD", "memory"), ("intel", "Intel", "memory"), ("none", "None", "block")],
                    ),
                ]),
            ]),
        ),
    }
}

fn region_page() -> (String, String) {
    (
        "Language & region".into(),
        page(&[
            head("Weather"),
            card(&[entry("services.weatherLocation", "Location", "City name or \"lat,lon\" (empty = detect from IP)", "Auto")]),
            head("Units"),
            card(&[
                select(
                    "services.weatherUnits",
                    "Temperature",
                    "Units for weather temperatures",
                    &[("auto", "Auto", "auto_awesome"), ("celsius", "°C", "thermostat"), ("fahrenheit", "°F", "thermostat")],
                ),
                select(
                    "services.sensorUnits",
                    "System temperatures",
                    "Units for CPU and GPU temperatures",
                    &[("celsius", "°C", "thermostat"), ("fahrenheit", "°F", "thermostat")],
                ),
                select(
                    "services.dataUnits",
                    "Data sizes",
                    "Units for data sizes and network speeds",
                    &[("binary", "Binary (KiB, MiB)", "storage"), ("decimal", "Decimal (KB, MB)", "storage")],
                ),
            ]),
            head("Time & date"),
            card(&[select(
                "services.clockFormat",
                "Clock format",
                "How times are shown across the shell",
                &[("auto", "Auto", "auto_awesome"), ("24h", "24-hour", "schedule"), ("12h", "12-hour", "schedule")],
            )]),
        ]),
    )
}

fn about_page() -> (String, String) {
    (
        "About".into(),
        page(&[
            "(nx-about-hero)".into(),
            head("System"),
            card(&[
                info_row("Hostname", "nexus.about.host"),
                info_row("Device", "nexus.about.device"),
                info_row("Distro", "nexus.about.distro"),
                info_row("Kernel", "nexus.about.kernel"),
                info_row("Firmware", "nexus.about.firmware"),
            ]),
            head("Software"),
            card(&[
                info_row("Shell", "nexus.about.shell"),
                info_row("EWW", "nexus.about.eww"),
                info_row("driftwm", "nexus.about.driftwm"),
            ]),
            head("Credits"),
            card(&[
                info_row("Design", "'Caelestia Shell (soramanew & contributors)'"),
                info_row("Compositor", "'driftwm'"),
                info_row("Widgets", "'EWW (elkowar)'"),
                info_row("Licence", "'GPL-3.0'"),
            ]),
        ]),
    )
}

fn build(page_i: usize, sub: usize) -> (String, String) {
    match page_i {
        0 => wallpaper_page(sub),
        1 => network_page(sub),
        2 => ("Connected devices".into(), page(&["(nx-bt-card)".into(), "(nx-bt-adapter)".into()])),
        3 => audio_page(sub),
        4 => ("Updates".into(), placeholder()),
        5 => ("Plugins".into(), placeholder()),
        6 => panels_page(sub),
        7 => apps_page(sub),
        8 => services_page(sub),
        9 => region_page(),
        _ => about_page(),
    }
}

/// Lower-case text searched by the nav filter: page label, description and
/// every row label/description of all its sub-pages.
fn search_text(i: usize) -> &'static str {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    &CACHE.get_or_init(|| (0..PAGES.len()).map(build_search_text).collect())[i]
}

fn build_search_text(i: usize) -> String {
    let p = &PAGES[i];
    let mut s = format!("{} {}", p.label, p.desc);
    for sub in 0..12 {
        let (title, body) = build(i, sub);
        s.push(' ');
        s.push_str(&title);
        for part in body.split(":label \"").skip(1).chain(body.split(":desc \"").skip(1)) {
            if let Some(end) = part.find('"') {
                s.push(' ');
                s.push_str(&part[..end]);
            }
        }
    }
    s.to_lowercase()
}

// ------------------------------------------------------------------ data

/// Split one `nmcli -t` line on unescaped ':'.
fn nm_fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

async fn nm_connections() -> (Vec<Value>, Vec<Value>) {
    let out = crate::util::run(&["nmcli", "-t", "-f", "NAME,UUID,TYPE,ACTIVE", "connection", "show"], 3000).await.unwrap_or_default();
    let (mut saved, mut vpns) = (vec![], vec![]);
    for l in out.lines() {
        let f = nm_fields(l);
        if f.len() < 4 {
            continue;
        }
        let item = |i: usize| json!({"i": i, "name": f[0], "uuid": f[1], "active": f[3] == "yes"});
        match f[2].as_str() {
            "802-11-wireless" => saved.push(item(saved.len())),
            "vpn" | "wireguard" => vpns.push(item(vpns.len())),
            _ => {}
        }
    }
    (saved, vpns)
}

async fn app_volumes() -> Vec<Value> {
    let out = crate::util::run(&["pactl", "-f", "json", "list", "sink-inputs"], 2000).await.unwrap_or_default();
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    let mut list = vec![];
    for (i, s) in v.as_array().into_iter().flatten().enumerate() {
        let p = &s["properties"];
        let name = p["application.name"].as_str().or(p["media.name"].as_str()).unwrap_or("Unknown").to_string();
        let vol = s["volume"]
            .as_object()
            .and_then(|o| o.values().next())
            .and_then(|c| c["value_percent"].as_str())
            .and_then(|p| p.trim_end_matches('%').trim().parse::<i64>().ok())
            .unwrap_or(0);
        list.push(json!({
            "i": i,
            "index": s["index"],
            "name": name,
            "media": p["media.name"].as_str().unwrap_or(""),
            "vol": vol,
            "muted": s["mute"].as_bool().unwrap_or(false),
        }));
    }
    list
}

async fn about() -> Value {
    let rd = |p: &str| crate::util::read_trim(p).unwrap_or_default();
    let distro = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        })
        .unwrap_or_default();
    let ver = |out: Option<String>| out.map(|s| s.lines().next().unwrap_or("").trim().to_string()).unwrap_or_else(|| "—".into());
    let (eww, drift) = tokio::join!(
        crate::util::run(&["eww", "--version"], 2000),
        crate::util::run(&["driftwm", "--version"], 2000)
    );
    let vendor = rd("/sys/class/dmi/id/sys_vendor");
    let product = rd("/sys/class/dmi/id/product_name");
    json!({
        "host": rd("/proc/sys/kernel/hostname"),
        "device": format!("{vendor} {product}").trim().to_string(),
        "distro": distro,
        "kernel": rd("/proc/sys/kernel/osrelease"),
        "firmware": rd("/sys/class/dmi/id/bios_version"),
        "shell": env!("CARGO_PKG_VERSION"),
        "eww": ver(eww).trim_start_matches("eww ").to_string(),
        "driftwm": ver(drift).trim_start_matches("driftwm ").to_string(),
    })
}

// ------------------------------------------------------------------ state

impl Nexus {
    pub fn json(&self) -> Value {
        let st = self.st.lock().unwrap();
        let sub = st.stack.last().copied().unwrap_or(0);
        let (title, _) = build(st.page, sub);
        let q = st.query.trim().to_lowercase();
        let matches: Vec<bool> = (0..PAGES.len()).map(|i| q.is_empty() || search_text(i).contains(q.as_str())).collect();
        json!({
            "page": st.page,
            "sub": sub,
            "isSub": sub != 0,
            "title": title,
            "match": matches,
            "query": st.query,
            "vpnActive": st.vpns.iter().any(|v| v["active"] == true),
            "wallRows": st.walls.chunks(3).map(|c| Value::from(c.to_vec())).collect::<Vec<_>>(),
            "nav": PAGES.iter().enumerate().map(|(i, p)| json!({
                "i": i, "label": p.label, "desc": p.desc, "icon": p.icon, "fill": !p.no_fill, "first": p.first, "last": p.last,
            })).collect::<Vec<_>>(),
            "saved": st.saved,
            "vpns": st.vpns,
            "appVols": st.app_vols,
            "apps": st.apps,
            "walls": st.walls,
            "about": if st.about.is_null() {
                json!({"host": "", "device": "", "distro": "", "kernel": "", "firmware": "", "shell": env!("CARGO_PKG_VERSION"), "eww": "", "driftwm": ""})
            } else {
                st.about.clone()
            },
            "form": {"ssid": st.form_ssid, "hidden": st.form_hidden, "status": st.form_status},
        })
    }

    pub fn current(&self) -> (usize, usize) {
        let st = self.st.lock().unwrap();
        (st.page, st.stack.last().copied().unwrap_or(0))
    }

    /// Load whatever the visible page needs, then publish.
    pub async fn refresh(&self) {
        let (p, sub) = self.current();
        let visible = app().ui.is_shown("nexus");
        // Rescan Wi-Fi while the network page is open (like the popout).
        app().net.set_list_demand(visible && p == 1);
        if visible {
            match (p, sub) {
                (0, 1) => {
                    let cur = app().theme.current_wallpaper();
                    let walls: Vec<Value> = crate::theme::list_wallpapers()
                        .iter()
                        .enumerate()
                        .map(|(i, w)| {
                            let s = w.to_string_lossy().into_owned();
                            json!({"i": i, "thumb": crate::theme::thumbnail_path(w).to_string_lossy(), "current": s == cur,
                                   "name": w.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()})
                        })
                        .collect();
                    self.st.lock().unwrap().walls = walls;
                    crate::theme::spawn_thumbnails();
                }
                (1, _) => {
                    let (saved, vpns) = nm_connections().await;
                    let mut st = self.st.lock().unwrap();
                    st.saved = saved;
                    st.vpns = vpns;
                }
                (3, _) => {
                    let v = app_volumes().await;
                    self.st.lock().unwrap().app_vols = v;
                }
                (7, 1) => {
                    let apps = app().launcher.apps_brief();
                    self.st.lock().unwrap().apps = apps;
                }
                (10, _) => {
                    let a = about().await;
                    self.st.lock().unwrap().about = a;
                }
                _ => {}
            }
        }
        publish();
    }

    /// Audio changed: refresh the app volume list if it is on screen.
    pub async fn on_audio(&self) {
        let (p, _) = self.current();
        if p == 3 && app().ui.is_shown("nexus") {
            let v = app_volumes().await;
            self.st.lock().unwrap().app_vols = v;
            publish();
        }
    }

    pub async fn command(&self, sub: &str, rest: &[String]) -> Result<String> {
        let arg = |i: usize| rest.get(i).map(String::as_str).unwrap_or("");
        let idx = |i: usize| arg(i).parse::<usize>().ok();
        match sub {
            // nexus open [page]
            "open" | "toggle" => {
                if let Some(p) = idx(0).filter(|p| *p < PAGES.len()) {
                    let mut st = self.st.lock().unwrap();
                    st.page = p;
                    st.stack.clear();
                } else if let Some(p) = PAGES.iter().position(|p| p.label.to_lowercase().starts_with(&arg(0).to_lowercase()) && !arg(0).is_empty()) {
                    let mut st = self.st.lock().unwrap();
                    st.page = p;
                    st.stack.clear();
                }
                if sub == "toggle" && app().ui.is_shown("nexus") {
                    app().ui.close("nexus").await?;
                } else {
                    app().ui.open("nexus").await?;
                }
            }
            "close" => {
                app().ui.close("nexus").await?;
            }
            "page" => {
                let Some(p) = idx(0).filter(|p| *p < PAGES.len()) else { bail!("nexus page <0-{}>", PAGES.len() - 1) };
                let mut st = self.st.lock().unwrap();
                st.page = p;
                st.stack.clear();
            }
            "sub" => {
                let Some(s) = idx(0) else { bail!("nexus sub <n>") };
                self.st.lock().unwrap().stack.push(s);
            }
            "back" => {
                self.st.lock().unwrap().stack.pop();
            }
            "search" => {
                let q = rest.join(" ");
                let mut st = self.st.lock().unwrap();
                st.query = q.chars().take(64).collect();
                // Jump to the first match so the content follows the search.
                let ql = st.query.trim().to_lowercase();
                if !ql.is_empty() && !search_text(st.page).contains(ql.as_str())
                    && let Some(first) = (0..PAGES.len()).find(|i| search_text(*i).contains(ql.as_str())) {
                        st.page = first;
                        st.stack.clear();
                    }
            }
            // Wallpapers sub-page
            "wall" => {
                let Some(i) = idx(0) else { bail!("nexus wall <i>") };
                let Some(p) = crate::theme::list_wallpapers().get(i).cloned() else { bail!("no wallpaper {i}") };
                app().theme.wallpaper_cmd("set", &p.to_string_lossy()).await?;
            }
            "browse" => {
                let dir = crate::util::expand(&crate::app::cfg().paths.wallpaper_dir);
                let _ = std::fs::create_dir_all(&dir);
                let explorer = crate::app::cfg().general.explorer.clone();
                crate::util::spawn_argv(&[&explorer, &dir.to_string_lossy()]);
            }
            // Network page
            "forget" | "vpn" | "saved-connect" => {
                let Some(i) = idx(0) else { bail!("nexus {sub} <i>") };
                let item = {
                    let st = self.st.lock().unwrap();
                    if sub == "vpn" { st.vpns.get(i).cloned() } else { st.saved.get(i).cloned() }
                };
                let Some(item) = item else { bail!("no connection {i}") };
                let uuid = item["uuid"].as_str().unwrap_or("").to_string();
                let active = item["active"].as_bool().unwrap_or(false);
                let action = match sub {
                    "forget" => "delete",
                    _ if active => "down",
                    _ => "up",
                };
                crate::util::run_ok(&["nmcli", "connection", action, "uuid", &uuid], 30000).await;
            }
            // Quick toggle: disconnect active VPNs, or bring the first one up.
            "vpn-toggle" => {
                let (saved, vpns) = nm_connections().await;
                {
                    let mut st = self.st.lock().unwrap();
                    st.saved = saved;
                    st.vpns = vpns.clone();
                }
                let active: Vec<&Value> = vpns.iter().filter(|v| v["active"] == true).collect();
                if active.is_empty() {
                    if let Some(u) = vpns.first().and_then(|v| v["uuid"].as_str()) {
                        crate::util::run_ok(&["nmcli", "connection", "up", "uuid", u], 30000).await;
                    } else {
                        crate::util::spawn_argv(&["nm-connection-editor"]);
                    }
                } else {
                    for v in active {
                        if let Some(u) = v["uuid"].as_str() {
                            crate::util::run_ok(&["nmcli", "connection", "down", "uuid", u], 30000).await;
                        }
                    }
                }
            }
            "vpn-off" => {
                let active: Vec<String> = {
                    let st = self.st.lock().unwrap();
                    st.vpns.iter().filter(|v| v["active"] == true).filter_map(|v| v["uuid"].as_str().map(str::to_string)).collect()
                };
                for u in active {
                    crate::util::run_ok(&["nmcli", "connection", "down", "uuid", &u], 30000).await;
                }
            }
            "vpn-settings" | "net-settings" => {
                crate::util::spawn_argv(&["nm-connection-editor"]);
            }
            "form-ssid" | "form-pw" => {
                let v = rest.join(" ");
                let mut st = self.st.lock().unwrap();
                if sub == "form-ssid" { st.form_ssid = v } else { st.form_pw = v }
                st.form_status.clear();
                return Ok(String::new());
            }
            "form-hidden" => {
                let mut st = self.st.lock().unwrap();
                st.form_hidden = !st.form_hidden;
            }
            "form-connect" => {
                let (ssid, pw, hidden) = {
                    let st = self.st.lock().unwrap();
                    (st.form_ssid.clone(), st.form_pw.clone(), st.form_hidden)
                };
                if ssid.trim().is_empty() {
                    self.st.lock().unwrap().form_status = "Enter a network name".into();
                } else {
                    self.st.lock().unwrap().form_status = "Connecting…".into();
                    publish();
                    let mut argv = vec!["nmcli", "device", "wifi", "connect", ssid.as_str()];
                    if !pw.is_empty() {
                        argv.extend(["password", pw.as_str()]);
                    }
                    if hidden {
                        argv.extend(["hidden", "yes"]);
                    }
                    let ok = crate::util::run_ok(&argv, 45000).await;
                    let mut st = self.st.lock().unwrap();
                    if ok {
                        st.form_status = "Connected".into();
                        st.form_ssid.clear();
                        st.form_pw.clear();
                        st.stack.pop();
                    } else {
                        st.form_status = "Could not connect".into();
                    }
                }
            }
            // Audio page: per-app volume
            "appvol" | "appmute" => {
                let Some(i) = idx(0) else { bail!("nexus {sub} <i> [pct]") };
                let index = self.st.lock().unwrap().app_vols.get(i).and_then(|a| a["index"].as_u64());
                let Some(index) = index else { bail!("no stream {i}") };
                let index = index.to_string();
                if sub == "appmute" {
                    crate::util::run_ok(&["pactl", "set-sink-input-mute", &index, "toggle"], 2000).await;
                } else {
                    let max = (crate::app::cfg().services.max_volume * 100.0).round() as i64;
                    let pct: f64 = arg(1).parse().unwrap_or(0.0);
                    let pct = (pct.round() as i64).clamp(0, max);
                    crate::util::run_ok(&["pactl", "set-sink-input-volume", &index, &format!("{pct}%")], 2000).await;
                }
                self.on_audio().await;
                return Ok(String::new());
            }
            // Apps page
            "app-fav" | "app-hide" => {
                let Some(i) = idx(0) else { bail!("nexus {sub} <i>") };
                let id = self.st.lock().unwrap().apps.get(i).and_then(|a| a["id"].as_str().map(str::to_string));
                let Some(id) = id else { bail!("no app {i}") };
                let key = if sub == "app-fav" { "launcher.favouriteApps" } else { "launcher.hiddenApps" };
                crate::settings::command("list-toggle", key, &[id.as_str()]).await?;
                let apps = app().launcher.apps_brief();
                self.st.lock().unwrap().apps = apps;
            }
            // Status icons list editor
            "status-toggle" | "status-up" | "status-down" | "status-remove" | "status-add" => {
                let c = crate::app::cfg();
                let mut icons = c.bar.status.icons.clone();
                let mut disabled = c.bar.status.disabled.clone();
                drop(c);
                let all = ["lockStatus", "audio", "microphone", "kbLayout", "network", "bluetooth", "battery"];
                let mut order: Vec<String> = icons.iter().chain(disabled.iter()).cloned().collect();
                match sub {
                    "status-add" => {
                        if let Some(missing) = all.iter().find(|a| !order.iter().any(|o| o == *a)) {
                            icons.push(missing.to_string());
                        }
                    }
                    _ => {
                        let Some(i) = idx(0).filter(|i| *i < order.len()) else { bail!("nexus {sub} <i>") };
                        let name = order[i].clone();
                        match sub {
                            "status-toggle" => {
                                if let Some(p) = icons.iter().position(|x| *x == name) {
                                    icons.remove(p);
                                    disabled.push(name);
                                } else {
                                    disabled.retain(|x| *x != name);
                                    icons.push(name);
                                }
                                // Keep the editor order stable.
                                icons.sort_by_key(|x| order.iter().position(|o| o == x));
                                disabled.sort_by_key(|x| order.iter().position(|o| o == x));
                            }
                            "status-remove" => {
                                icons.retain(|x| *x != name);
                                disabled.retain(|x| *x != name);
                            }
                            _ => {
                                let j = if sub == "status-up" { i.checked_sub(1) } else { (i + 1 < order.len()).then_some(i + 1) };
                                if let Some(j) = j {
                                    order.swap(i, j);
                                    icons.sort_by_key(|x| order.iter().position(|o| o == x));
                                    disabled.sort_by_key(|x| order.iter().position(|o| o == x));
                                }
                            }
                        }
                    }
                }
                crate::settings::write("bar.status.disabled", serde_json::to_value(&disabled)?).await?;
                crate::settings::write("bar.status.icons", serde_json::to_value(&icons)?).await?;
            }
            _ => bail!("nexus open|toggle|close|page|sub|back|search|wall|browse|forget|vpn|appvol|appmute|app-fav|app-hide|status-*"),
        }
        self.refresh().await;
        Ok(String::new())
    }
}

/// Status icons in editor order (enabled first as listed, then disabled).
pub fn status_editor() -> Vec<Value> {
    let c = crate::app::cfg();
    let label = |n: &str| match n {
        "lockStatus" => "Lock keys",
        "audio" => "Speakers",
        "microphone" => "Microphone",
        "kbLayout" => "Keyboard layout",
        "network" => "Network",
        "bluetooth" => "Bluetooth",
        "battery" => "Battery",
        _ => "Unknown",
    };
    c.bar
        .status
        .icons
        .iter()
        .map(|n| (n, true))
        .chain(c.bar.status.disabled.iter().map(|n| (n, false)))
        .enumerate()
        .map(|(i, (n, on))| json!({"i": i, "label": label(n), "on": on}))
        .collect()
}

pub fn snapshot() -> Value {
    let mut v = app().nexus.json();
    v["statusIcons"] = Value::from(status_editor());
    v
}

/// The page snippet lives in its own topic so the `literal` is rebuilt in a
/// separate update from the data its widgets bind to.
pub fn page_snapshot() -> Value {
    let (p, sub) = app().nexus.current();
    json!({ "yuck": build(p, sub).1 })
}

pub fn publish() {
    let hub = &app().hub;
    hub.publish("npage", &page_snapshot());
    hub.publish("nexus", &snapshot());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nm_split() {
        assert_eq!(nm_fields(r"My\:Net:uuid-1:802-11-wireless:yes"), vec!["My:Net", "uuid-1", "802-11-wireless", "yes"]);
    }

    #[test]
    fn pages_build_balanced() {
        for p in 0..PAGES.len() {
            for s in 0..11 {
                let (_, y) = build(p, s);
                let mut depth = 0i32;
                let mut in_str = false;
                let mut esc = false;
                for c in y.chars() {
                    if in_str {
                        if esc {
                            esc = false;
                        } else if c == '\\' {
                            esc = true;
                        } else if c == '"' {
                            in_str = false;
                        }
                        continue;
                    }
                    match c {
                        '"' => in_str = true,
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    assert!(depth >= 0, "page {p}/{s}");
                }
                assert_eq!(depth, 0, "page {p}/{s}: {y}");
            }
        }
    }

    #[test]
    fn search_finds_rows() {
        assert!(search_text(6).contains("vim keybinds"));
        assert!(search_text(8).contains("default timeout"));
    }
}

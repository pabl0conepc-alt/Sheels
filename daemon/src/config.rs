//! User configuration: `~/.config/sheels/shell.json`. Every key is optional;
//! only what you want to change needs to be present (same philosophy as
//! Caelestia's `shell.json`).

use serde::{Deserialize, Serialize};

/// Declares a `#[serde(default)]` camelCase struct together with its Default.
macro_rules! section {
    ($(#[$m:meta])* $name:ident { $($(#[$fm:meta])* $f:ident : $t:ty = $d:expr),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        pub struct $name { $($(#[$fm])* pub $f: $t),* }
        impl Default for $name {
            fn default() -> Self { $name { $($f: $d),* } }
        }
    };
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub general: General,
    pub background: Background,
    pub bar: Bar,
    pub border: Border,
    pub launcher: Launcher,
    pub notifs: Notifs,
    pub osd: Osd,
    pub dashboard: Dashboard,
    pub sidebar: Sidebar,
    pub utilities: Utilities,
    pub services: Services,
    pub session: Session,
    pub paths: Paths,
    pub driftwm: Driftwm,
    pub lock: Lock,
    pub appearance: Appearance,
}

section!(General {
    terminal: String = "foot".into(),
    audio: String = "pwvucontrol".into(),
    playback: String = "mpv".into(),
    explorer: String = "thunar".into(),
    logo: String = String::new(),
    battery_warn_levels: Vec<u8> = vec![20, 10, 5],
    battery_critical: u8 = 3,
});

section!(Background {
    enabled: bool = true,
});

section!(Bar {
    persistent: bool = true,
    show_on_hover: bool = true,
    scroll_actions: ScrollActions = ScrollActions::default(),
    workspaces: BarWorkspaces = BarWorkspaces::default(),
    active_window: BarActiveWindow = BarActiveWindow::default(),
    tray: BarTray = BarTray::default(),
    status: BarStatus = BarStatus::default(),
    clock: BarClock = BarClock::default(),
});
section!(ScrollActions {
    workspaces: bool = true,
    volume: bool = true,
    brightness: bool = true,
});
section!(BarWorkspaces {
    /// Number of bookmark "workspaces" shown in the bar.
    shown: usize = 5,
    active_indicator: bool = true,
    active_trail: bool = true,
    occupied_bg: bool = false,
    show_unoccupied: bool = true,
    show_windows: bool = true,
    max_window_icons: usize = 5,
});
section!(BarActiveWindow {
    compact: bool = false,
    inverted: bool = false,
    show_on_hover: bool = true,
    popout_on_hover: bool = true,
});
section!(BarTray {
    background: bool = false,
    recolour: bool = false,
    compact: bool = false,
    popout_on_hover: bool = true,
});
section!(BarStatus {
    /// Top to bottom. Known: lockStatus, audio, microphone, kbLayout, network, bluetooth, battery.
    icons: Vec<String> = strs(&["lockStatus", "network", "bluetooth", "battery"]),
    /// Entries present in the list but switched off in the Nexus.
    disabled: Vec<String> = strs(&["audio", "microphone", "kbLayout"]),
    popout_on_hover: bool = true,
});
section!(BarClock {
    background: bool = false,
    show_date: bool = false,
    show_icon: bool = true,
    show_seconds: bool = false,
});

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Border {
    pub thickness: u32,
    pub rounding: u32,
}
impl Default for Border {
    fn default() -> Self {
        Border { thickness: 10, rounding: 25 }
    }
}

section!(Launcher {
    enabled: bool = true,
    show_on_hover: bool = false,
    max_shown: usize = 7,
    max_wallpapers: usize = 9,
    action_prefix: String = ">".into(),
    vim_keybinds: bool = false,
    enable_dangerous_actions: bool = false,
    use_fuzzy: UseFuzzy = UseFuzzy::default(),
    favourite_apps: Vec<String> = vec![],
    hidden_apps: Vec<String> = vec![],
});
section!(UseFuzzy {
    apps: bool = true,
    actions: bool = true,
    schemes: bool = true,
    variants: bool = true,
    wallpapers: bool = true,
});
section!(Notifs {
    expire: bool = true,
    default_expire_timeout: u64 = 5000,
    open_expanded: bool = false,
    group_preview_num: usize = 3,
    max_popups: usize = 5,
    /// Show popups while a window is fullscreen: "on" | "off" | "critical".
    fullscreen: String = "on".into(),
    action_on_click: bool = false,
});
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Osd {
    pub enabled: bool,
    pub hide_delay: u64,
    pub enable_brightness: bool,
    pub enable_microphone: bool,
}
impl Default for Osd {
    fn default() -> Self {
        Osd { enabled: true, hide_delay: 2000, enable_brightness: true, enable_microphone: false }
    }
}

section!(Dashboard {
    enabled: bool = true,
    show_on_hover: bool = true,
    show_clock_seconds: bool = false,
    show_dashboard: bool = true,
    show_media: bool = true,
    show_performance: bool = true,
    show_weather: bool = true,
    performance: Performance = Performance::default(),
    /// Milliseconds.
    resource_update_interval: u64 = 1000,
    /// Milliseconds.
    media_update_interval: u64 = 500,
});
section!(Performance {
    show_battery: bool = true,
    show_gpu: bool = true,
    show_cpu: bool = true,
    show_memory: bool = true,
    show_storage: bool = true,
    show_network: bool = true,
});
section!(Sidebar {
    enabled: bool = true,
    show_on_hover: bool = false,
});
section!(Utilities {
    enabled: bool = true,
    keep_awake: bool = true,
    screen_recorder: bool = true,
    quick_toggles: bool = true,
    toggles: QuickToggles = QuickToggles::default(),
    max_toasts: usize = 4,
    toasts_fullscreen: bool = false,
    toasts: Toasts = Toasts::default(),
});
section!(QuickToggles {
    wifi: bool = true,
    bluetooth: bool = true,
    mic: bool = true,
    settings: bool = true,
    game_mode: bool = true,
    dnd: bool = true,
    vpn: bool = false,
});
section!(Toasts {
    charging_changed: bool = true,
    game_mode_changed: bool = true,
    dnd_changed: bool = true,
    audio_output_changed: bool = true,
    audio_input_changed: bool = true,
    caps_lock_changed: bool = true,
    num_lock_changed: bool = true,
    kb_layout_changed: bool = true,
    vpn_changed: bool = true,
    now_playing: bool = false,
});
section!(Services {
    weather_location: String = String::new(),
    /// "auto" | "celsius" | "fahrenheit"
    weather_units: String = "auto".into(),
    /// "celsius" | "fahrenheit"
    sensor_units: String = "celsius".into(),
    /// "binary" | "decimal"
    data_units: String = "binary".into(),
    audio_increment: f64 = 0.1,
    brightness_increment: f64 = 0.1,
    max_volume: f64 = 1.0,
    default_player: String = "Spotify".into(),
    /// "auto" | "nvidia" | "amd" | "intel" | "none"
    gpu_type: String = "auto".into(),
    smart_scheme: bool = true,
    /// "auto" | "12h" | "24h"
    clock_format: String = "auto".into(),
    /// Seconds between Wi-Fi rescans while the network page/popout is open.
    wifi_rescan_interval: u64 = 15,
});
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    pub logout: Vec<String>,
    pub shutdown: Vec<String>,
    pub hibernate: Vec<String>,
    pub reboot: Vec<String>,
}
impl Default for Session {
    fn default() -> Self {
        Session {
            logout: vec!["driftwm".into(), "msg".into(), "action".into(), "quit".into()],
            shutdown: vec!["systemctl".into(), "poweroff".into()],
            hibernate: vec!["systemctl".into(), "hibernate".into()],
            reboot: vec!["systemctl".into(), "reboot".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Paths {
    pub wallpaper_dir: String,
    pub face: String,
}
impl Default for Paths {
    fn default() -> Self {
        Paths { wallpaper_dir: "~/Pictures/Wallpapers".into(), face: "~/.face".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Driftwm {
    /// Rewrite the `# >>> sheels` block of the driftwm config on theme changes.
    pub manage_config: bool,
    pub config_path: String,
}
impl Default for Driftwm {
    fn default() -> Self {
        Driftwm { manage_config: true, config_path: "~/.config/driftwm/config.toml".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Lock {
    pub command: Vec<String>,
}
impl Default for Lock {
    fn default() -> Self {
        Lock {
            command: vec![
                "hyprlock".into(),
                "-c".into(),
                crate::util::state_dir().join("theme/hyprlock.conf").to_string_lossy().into_owned(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    pub transparency: bool,
    pub transparency_base: f64,
    pub transparency_layers: f64,
    pub font_scale: f64,
    pub anim_scale: f64,
}
impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            transparency: false,
            transparency_base: 0.85,
            transparency_layers: 0.4,
            font_scale: 1.0,
            anim_scale: 1.0,
        }
    }
}

/// Recursively merge `over` into `base` (objects merge, everything else replaces).
pub fn merge(base: &mut serde_json::Value, over: &serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

impl Config {
    /// `~/.config/sheels/shell.json`, plus the Nexus overlay
    /// (`~/.local/state/sheels/settings.json`) used when shell.json is managed
    /// by Home Manager (a read-only store symlink).
    pub fn load() -> Config {
        let path = crate::util::config_dir().join("shell.json");
        let read = |p: &std::path::Path| -> Option<serde_json::Value> {
            let s = std::fs::read_to_string(p).ok()?;
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("sheels: invalid {}: {e}; ignoring it", p.display());
                    None
                }
            }
        };
        let mut v = read(&path).unwrap_or_else(|| serde_json::json!({}));
        if let Some(o) = read(&crate::util::state_dir().join("settings.json")) {
            merge(&mut v, &o);
        }
        serde_json::from_value(v).unwrap_or_else(|e| {
            eprintln!("sheels: invalid configuration: {e}; using defaults");
            Config::default()
        })
    }
}

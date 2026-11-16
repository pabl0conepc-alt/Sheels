//! User configuration: `~/.config/sheels/shell.json`. Every key is optional;
//! only what you want to change needs to be present (same philosophy as
//! Caelestia's `shell.json`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub general: General,
    pub bar: Bar,
    pub border: Border,
    pub launcher: Launcher,
    pub notifs: Notifs,
    pub osd: Osd,
    pub dashboard: Dashboard,
    pub services: Services,
    pub session: Session,
    pub paths: Paths,
    pub driftwm: Driftwm,
    pub lock: Lock,
    pub appearance: Appearance,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: General::default(),
            bar: Bar::default(),
            border: Border::default(),
            launcher: Launcher::default(),
            notifs: Notifs::default(),
            osd: Osd::default(),
            dashboard: Dashboard::default(),
            services: Services::default(),
            session: Session::default(),
            paths: Paths::default(),
            driftwm: Driftwm::default(),
            lock: Lock::default(),
            appearance: Appearance::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct General {
    pub terminal: String,
    pub audio: String,
    pub explorer: String,
    pub logo: String,
    pub battery_warn_levels: Vec<u8>,
    pub battery_critical: u8,
}
impl Default for General {
    fn default() -> Self {
        General {
            terminal: "foot".into(),
            audio: "pwvucontrol".into(),
            explorer: "thunar".into(),
            logo: String::new(),
            battery_warn_levels: vec![20, 10, 5],
            battery_critical: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bar {
    /// Number of bookmark "workspaces" shown in the bar.
    pub workspaces_shown: usize,
    pub clock_show_date: bool,
    pub clock_show_seconds: bool,
    pub clock_show_icon: bool,
    pub status_icons: Vec<String>,
    pub scroll_volume: bool,
    pub scroll_brightness: bool,
    pub persistent: bool,
}
impl Default for Bar {
    fn default() -> Self {
        Bar {
            workspaces_shown: 5,
            clock_show_date: false,
            clock_show_seconds: false,
            clock_show_icon: true,
            status_icons: ["lockStatus", "network", "bluetooth", "battery"].iter().map(|s| s.to_string()).collect(),
            scroll_volume: true,
            scroll_brightness: true,
            persistent: true,
        }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Launcher {
    pub max_shown: usize,
    pub max_wallpapers: usize,
    pub action_prefix: String,
    pub enable_dangerous_actions: bool,
    pub favourite_apps: Vec<String>,
    pub hidden_apps: Vec<String>,
}
impl Default for Launcher {
    fn default() -> Self {
        Launcher {
            max_shown: 7,
            max_wallpapers: 9,
            action_prefix: ">".into(),
            enable_dangerous_actions: false,
            favourite_apps: vec![],
            hidden_apps: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Notifs {
    pub expire: bool,
    pub default_expire_timeout: u64,
    pub max_popups: usize,
    pub action_on_click: bool,
}
impl Default for Notifs {
    fn default() -> Self {
        Notifs { expire: true, default_expire_timeout: 5000, max_popups: 5, action_on_click: false }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Dashboard {
    pub enabled: bool,
    pub show_on_hover: bool,
    pub resource_update_interval: u64,
    pub media_update_interval: u64,
}
impl Default for Dashboard {
    fn default() -> Self {
        Dashboard { enabled: true, show_on_hover: true, resource_update_interval: 1000, media_update_interval: 500 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Services {
    pub weather_location: String,
    /// "auto" | "celsius" | "fahrenheit"
    pub weather_units: String,
    pub sensor_units: String,
    pub audio_increment: f64,
    pub brightness_increment: f64,
    pub max_volume: f64,
    pub default_player: String,
    /// "auto" | "nvidia" | "amd" | "intel" | "none"
    pub gpu_type: String,
    pub smart_scheme: bool,
    /// "auto" | "12h" | "24h"
    pub clock_format: String,
}
impl Default for Services {
    fn default() -> Self {
        Services {
            weather_location: String::new(),
            weather_units: "auto".into(),
            sensor_units: "celsius".into(),
            audio_increment: 0.1,
            brightness_increment: 0.1,
            max_volume: 1.0,
            default_player: "Spotify".into(),
            gpu_type: "auto".into(),
            smart_scheme: true,
            clock_format: "auto".into(),
        }
    }
}

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
        Lock { command: vec!["hyprlock".into(), "--immediate-render".into()] }
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

impl Config {
    pub fn load() -> Config {
        let path = crate::util::config_dir().join("shell.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Config>(&s) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("sheels: invalid {}: {e}; using defaults", path.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }
}

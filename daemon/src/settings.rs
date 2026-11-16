//! Live configuration editing (backs the Nexus settings panel).
//!
//! Only keys the user changed are written to `~/.config/sheels/shell.json`
//! (the file stays sparse, like Caelestia's `shell.json`). After each write
//! the config is reloaded, the `cfg` topic is republished and the services
//! affected by that section are refreshed.

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use crate::app::app;
use crate::config::Config;

fn shell_json() -> std::path::PathBuf {
    crate::util::config_dir().join("shell.json")
}

/// Where Nexus edits go: shell.json itself, unless it is a symlink (Home
/// Manager) or not writable — then a state-dir overlay merged on top of it.
fn path() -> std::path::PathBuf {
    let p = shell_json();
    let managed = std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink() || m.permissions().readonly()).unwrap_or(false);
    if managed { crate::util::state_dir().join("settings.json") } else { p }
}

fn read_user() -> Value {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn lookup<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(v, |v, k| v.get(k))
}

/// Effective value of `key` (user file merged over defaults).
pub fn effective(key: &str) -> Option<Value> {
    let full = serde_json::to_value(&*crate::app::cfg()).ok()?;
    lookup(&full, key).cloned()
}

/// Value the key has without the file being written: the defaults, plus
/// shell.json when the Nexus writes to the overlay.
fn baseline(key: &str) -> Option<Value> {
    let mut full = serde_json::to_value(Config::default()).ok()?;
    if path() != shell_json()
        && let Some(base) = std::fs::read_to_string(shell_json()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            crate::config::merge(&mut full, &base);
        }
    lookup(&full, key).cloned()
}

fn default_of(key: &str) -> Option<Value> {
    let full = serde_json::to_value(Config::default()).ok()?;
    lookup(&full, key).cloned()
}

fn set_in(root: &mut Value, key: &str, val: Value) {
    let mut cur = root;
    let parts: Vec<&str> = key.split('.').collect();
    for (i, k) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let obj = cur.as_object_mut().unwrap();
        if i == parts.len() - 1 {
            obj.insert(k.to_string(), val);
            return;
        }
        cur = obj.entry(k.to_string()).or_insert_with(|| Value::Object(Map::new()));
    }
}

/// Coerce a textual value to the JSON type the key has by default.
fn coerce(key: &str, raw: &str) -> Result<Value> {
    let Some(def) = default_of(key) else { bail!("unknown setting {key}") };
    let cur = effective(key).unwrap_or(def.clone());
    Ok(match def {
        Value::Bool(_) => match raw {
            "toggle" => Value::Bool(!cur.as_bool().unwrap_or(false)),
            "true" | "on" | "1" => Value::Bool(true),
            "false" | "off" | "0" => Value::Bool(false),
            _ => bail!("{key} expects true|false|toggle"),
        },
        Value::Number(n) => {
            let x: f64 = raw.trim().parse().map_err(|_| anyhow::anyhow!("{key} expects a number"))?;
            if n.is_f64() {
                serde_json::json!(x)
            } else {
                serde_json::json!(x.round().max(0.0) as u64)
            }
        }
        Value::String(_) => Value::String(raw.to_string()),
        Value::Array(_) => {
            serde_json::from_str::<Value>(raw).ok().filter(Value::is_array).ok_or_else(|| anyhow::anyhow!("{key} expects a JSON array"))?
        }
        _ => bail!("{key} cannot be set directly"),
    })
}

/// Validate, store and apply one change.
pub async fn write(key: &str, val: Value) -> Result<()> {
    let mut user = read_user();
    let mut probe = serde_json::to_value(&*crate::app::cfg())?;
    set_in(&mut probe, key, val.clone());
    // Make sure the result still deserialises (types, enums…).
    let cfg: Config = serde_json::from_value(probe)?;
    if baseline(key).as_ref() == Some(&val) {
        // Back to the value underneath: drop the key so the file stays minimal.
        remove_in(&mut user, key);
    } else {
        set_in(&mut user, key, val);
    }
    let text = serde_json::to_string_pretty(&user)? + "\n";
    crate::util::write_atomic(&path(), text.as_bytes())?;
    *app().cfg.write().unwrap() = std::sync::Arc::new(cfg);
    publish();
    apply(key).await;
    Ok(())
}

fn remove_in(root: &mut Value, key: &str) {
    let parts: Vec<&str> = key.split('.').collect();
    fn rec(v: &mut Value, parts: &[&str]) {
        let Some(obj) = v.as_object_mut() else { return };
        if parts.len() == 1 {
            obj.remove(parts[0]);
            return;
        }
        if let Some(child) = obj.get_mut(parts[0]) {
            rec(child, &parts[1..]);
            if child.as_object().is_some_and(Map::is_empty) {
                obj.remove(parts[0]);
            }
        }
    }
    rec(root, &parts);
}

/// Publish the effective configuration for the UI (`cfg` topic).
pub fn publish() {
    let c = crate::app::cfg();
    if let Ok(mut v) = serde_json::to_value(&*c) {
        // Quick toggles laid out like Caelestia: one row, or two rows
        // (ceil(n/2) + rest) when there are more than six.
        let t = &c.utilities.toggles;
        let ids: Vec<&str> = [
            (t.wifi, "wifi"),
            (t.bluetooth, "bluetooth"),
            (t.mic, "mic"),
            (t.settings, "settings"),
            (t.game_mode, "gameMode"),
            (t.dnd, "dnd"),
            (t.vpn, "vpn"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, id)| *id)
        .collect();
        let rows: Vec<Vec<&str>> = if ids.len() > 6 {
            let split = ids.len().div_ceil(2);
            vec![ids[..split].to_vec(), ids[split..].to_vec()]
        } else {
            vec![ids]
        };
        v["toggleRows"] = serde_json::json!(rows);
        app().hub.publish("cfg", &v);
    }
}

/// Refresh whatever depends on the changed section.
async fn apply(key: &str) {
    let a = app();
    let top = key.split('.').next().unwrap_or("");
    match top {
        "appearance" | "border" | "background" | "driftwm" | "paths" => a.theme.apply_current().await,
        "services" if key == "services.smartScheme" => a.theme.apply_current().await,
        "services" if key.starts_with("services.weather") || key == "services.clockFormat" => a.info.refresh_weather(),
        "bar" => {
            a.wm.republish();
            a.publish_status();
        }
        "launcher" => a.launcher.refresh(),
        "notifs" => a.notifs.publish(),
        "dashboard" => {
            let t = a.hub.get("ui").and_then(|u| serde_json::from_str::<Value>(&u).ok()).and_then(|u| u["dashTab"].as_u64()).unwrap_or(0);
            a.ui.set_dash_tab(t as u8);
        }
        _ => {}
    }
    // The bar width depends on the border thickness / persistence: reopen static windows.
    if top == "border" || key == "bar.persistent" {
        a.ui.open_static().await;
    }
    crate::nexus::publish();
}

/// `sheels config <sub> <key> [value…]`
pub async fn command(sub: &str, key: &str, rest: &[&str]) -> Result<String> {
    match sub {
        "get" => Ok(effective(key).map(|v| v.to_string()).unwrap_or_default()),
        "set" => {
            let raw = rest.join(" ");
            let v = coerce(key, &raw)?;
            write(key, v).await?;
            Ok(String::new())
        }
        // config step <key> <delta> <min> <max>
        "step" => {
            let delta: f64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let lo: f64 = rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(f64::MIN);
            let hi: f64 = rest.get(2).and_then(|s| s.parse().ok()).unwrap_or(f64::MAX);
            let cur = effective(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let nv = ((cur + delta) * 1000.0).round() / 1000.0;
            let v = coerce(key, &crate::util::clamp(nv, lo, hi).to_string())?;
            write(key, v).await?;
            Ok(String::new())
        }
        // config cycle <key> a,b,c
        "cycle" => {
            let opts: Vec<&str> = rest.first().map(|s| s.split(',').collect()).unwrap_or_default();
            if opts.is_empty() {
                bail!("config cycle <key> a,b,c");
            }
            let cur = effective(key).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let i = opts.iter().position(|o| *o == cur).map(|i| (i + 1) % opts.len()).unwrap_or(0);
            write(key, Value::String(opts[i].to_string())).await?;
            Ok(String::new())
        }
        // String lists: config list-toggle <key> <item> | list-add <key> <item>
        // | list-remove <key> <idx> | list-move <key> <idx> up|down
        "list-toggle" | "list-add" | "list-remove" | "list-move" => {
            let mut list: Vec<String> = effective(key)
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| anyhow::anyhow!("{key} is not a list"))?;
            let arg = rest.first().copied().unwrap_or("");
            match sub {
                "list-toggle" => {
                    if let Some(i) = list.iter().position(|x| x == arg) {
                        list.remove(i);
                    } else if !arg.is_empty() {
                        list.push(arg.to_string());
                    }
                }
                "list-add" => {
                    if !arg.is_empty() && !list.iter().any(|x| x == arg) {
                        list.push(arg.to_string());
                    }
                }
                "list-remove" => {
                    if let Ok(i) = arg.parse::<usize>()
                        && i < list.len() {
                            list.remove(i);
                        }
                }
                _ => {
                    let i: usize = arg.parse().unwrap_or(usize::MAX);
                    let up = rest.get(1).copied() == Some("up");
                    if i < list.len() {
                        let j = if up { i.checked_sub(1) } else { (i + 1 < list.len()).then_some(i + 1) };
                        if let Some(j) = j {
                            list.swap(i, j);
                        }
                    }
                }
            }
            write(key, serde_json::to_value(list)?).await?;
            Ok(String::new())
        }
        "path" => Ok(path().to_string_lossy().into_owned()),
        _ => bail!("config get|set|step|cycle|list-toggle|list-add|list-remove|list-move|path <key> [value]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_remove_nested() {
        let mut v = serde_json::json!({});
        set_in(&mut v, "bar.clock.showDate", Value::Bool(true));
        assert_eq!(v, serde_json::json!({"bar": {"clock": {"showDate": true}}}));
        remove_in(&mut v, "bar.clock.showDate");
        assert_eq!(v, serde_json::json!({}));
    }

    #[test]
    fn defaults_have_camel_keys() {
        assert_eq!(default_of("bar.workspaces.shown"), Some(serde_json::json!(5)));
        assert_eq!(default_of("dashboard.performance.showGpu"), Some(Value::Bool(true)));
        assert_eq!(default_of("services.audioIncrement"), Some(serde_json::json!(0.1)));
        assert!(default_of("nope.nothing").is_none());
    }
}

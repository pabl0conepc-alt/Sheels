//! `sheels setup`: one-shot per-user setup + health check (no daemon needed).
//!
//! - installs the driftwm config with Caelestia's keybinds and the shell in
//!   its autostart (the previous file is kept as config.toml.bak);
//! - creates the wallpaper directory;
//! - checks that everything the shell relies on is reachable.

use std::path::Path;
use std::process::Command;

use crate::util::{expand, home, share_dir, which};

fn ok(msg: &str) {
    println!("  \x1b[32m✓\x1b[0m {msg}");
}
fn warn(msg: &str) {
    println!("  \x1b[33m!\x1b[0m {msg}");
}

fn has_font(family: &str) -> bool {
    Command::new("fc-list")
        .arg(family)
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
        || std::env::var_os("SHEELS_FONT_DIRS")
            .map(|d| {
                std::env::split_paths(&d).any(|p| {
                    walk_has(&p, &family.to_lowercase().replace(' ', ""), 3)
                })
            })
            .unwrap_or(false)
}

fn walk_has(dir: &Path, needle: &str, depth: u32) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase().replace([' ', '-', '_'], "")).unwrap_or_default();
        if name.contains(needle) {
            return true;
        }
        if depth > 0 && p.is_dir() && walk_has(&p, needle, depth - 1) {
            return true;
        }
    }
    false
}

pub fn run(args: &[String]) -> i32 {
    let force = args.iter().any(|a| a == "--force");
    let check_only = args.iter().any(|a| a == "--check");
    println!("Sheels setup\n");

    // 1. driftwm config
    let cfg_dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    let target = cfg_dir.join("driftwm/config.toml");
    let src = share_dir().join("config/driftwm/config.toml");
    let current = std::fs::read_to_string(&target).unwrap_or_default();
    if current.contains("sheels daemon") && !force {
        ok(&format!("driftwm config already starts the shell ({})", target.display()));
    } else if check_only {
        warn("driftwm config does not start the shell yet (run `sheels setup`)");
    } else {
        match std::fs::read_to_string(&src) {
            Ok(new) => {
                let _ = std::fs::create_dir_all(target.parent().unwrap());
                if target.exists() {
                    let bak = target.with_extension("toml.bak");
                    let _ = std::fs::copy(&target, &bak);
                    ok(&format!("previous driftwm config saved as {}", bak.display()));
                }
                // Write a regular writable file: the theme keeps its colour
                // block in sync, so a read-only store link would not work.
                let _ = std::fs::remove_file(&target);
                match std::fs::write(&target, new) {
                    Ok(()) => ok(&format!("installed driftwm config with Caelestia keybinds ({})", target.display())),
                    Err(e) => warn(&format!("cannot write {}: {e}", target.display())),
                }
            }
            Err(e) => warn(&format!("cannot read {}: {e}", src.display())),
        }
    }

    // 2. wallpapers
    let walls = expand(&crate::config::Config::load().paths.wallpaper_dir);
    if !check_only {
        let _ = std::fs::create_dir_all(&walls);
    }
    let n = std::fs::read_dir(&walls).map(|r| r.count()).unwrap_or(0);
    if n == 0 {
        warn(&format!("{} is empty: put wallpapers there (the colours come from them)", walls.display()));
    } else {
        ok(&format!("{n} wallpaper(s) in {}", walls.display()));
    }

    // 3. health check
    println!();
    for (bin, what) in [
        ("driftwm", "compositor"),
        ("eww", "widgets (bundled)"),
        ("pactl", "audio"),
        ("nmcli", "network"),
        ("hyprlock", "lock screen"),
        ("swayidle", "auto-lock on idle"),
        ("cliphist", "clipboard history"),
        ("foot", "terminal (Super+T)"),
    ] {
        if which(bin) {
            ok(&format!("{bin} ({what})"));
        } else {
            warn(&format!("{bin} not found ({what})"));
        }
    }
    for fam in ["Material Symbols Rounded", "Google Sans Flex", "Rubik"] {
        if has_font(fam) {
            ok(&format!("font {fam}"));
        } else {
            warn(&format!("font {fam} missing: icons/text will look wrong"));
        }
    }
    let running = std::os::unix::net::UnixStream::connect(crate::util::socket_path()).is_ok();
    if running {
        ok("shell is running");
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        warn("shell not running in this session: start it with `sheels daemon` (or log out and back in)");
    } else {
        println!("\nDone. Log out, pick \"driftwm\" on the login screen and the shell starts by itself.");
    }
    0
}

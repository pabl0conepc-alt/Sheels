//! Small helpers shared by every service: paths, process spawning, sysfs reads.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("sheels")
}
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("sheels")
}
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("sheels")
}
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("sheels"),
        _ => PathBuf::from(format!("/tmp/sheels-{}", unsafe { libc::getuid() })),
    }
}
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SHEELS_SOCKET") {
        return PathBuf::from(p);
    }
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    runtime_dir().join(format!("sheels-{display}.sock"))
}

/// Directory holding the shipped EWW config and assets.
pub fn share_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SHEELS_SHARE") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|p| p.parent()) {
            let cand = prefix.join("share/sheels");
            if cand.join("eww/eww.yuck").exists() {
                return cand;
            }
            // Running from the source tree: daemon/target/<profile>/sheels
            for anc in exe.ancestors() {
                if anc.join("eww/eww.yuck").exists() {
                    return anc.to_path_buf();
                }
            }
        }
    }
    PathBuf::from("/usr/share/sheels")
}

pub fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else if p == "~" {
        home()
    } else {
        PathBuf::from(p)
    }
}

pub fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub fn read_num<T: std::str::FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read_trim(path)?.parse().ok()
}

pub fn which(bin: &str) -> bool {
    if bin.contains('/') {
        return Path::new(bin).exists();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// Fire-and-forget a shell command, fully detached from the daemon.
pub fn spawn_sh(cmd: &str) {
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("setsid -f sh -c {} >/dev/null 2>&1", shell_quote(cmd)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| std::thread::spawn(move || c.wait()));
}

/// Spawn a program (no shell) detached; argv[0] is the program.
pub fn spawn_argv(argv: &[&str]) {
    if argv.is_empty() {
        return;
    }
    let cmd = argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    spawn_sh(&cmd);
}

/// Run a command and capture stdout (async, with timeout).
pub async fn run(argv: &[&str], timeout_ms: u64) -> Option<String> {
    let mut cmd = tokio::process::Command::new(argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let fut = cmd.output();
    match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok(out)) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => None,
    }
}

/// Like `run` but returns true when the command exited successfully.
pub async fn run_ok(argv: &[&str], timeout_ms: u64) -> bool {
    let mut cmd = tokio::process::Command::new(argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    matches!(tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.status()).await, Ok(Ok(s)) if s.success())
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Atomically write a file (write to tmp, rename) creating parent dirs.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Write only when content differs (avoids waking file watchers).
pub fn write_if_changed(path: &Path, data: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).map(|s| s == data).unwrap_or(false) {
        return Ok(false);
    }
    write_atomic(path, data.as_bytes())?;
    Ok(true)
}

pub fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn human_bytes(b: f64, binary: bool) -> String {
    let (k, units): (f64, [&str; 5]) =
        if binary { (1024.0, ["B", "KiB", "MiB", "GiB", "TiB"]) } else { (1000.0, ["B", "KB", "MB", "GB", "TB"]) };
    let mut v = b;
    let mut i = 0;
    while v >= k && i < units.len() - 1 {
        v /= k;
        i += 1;
    }
    if i == 0 { format!("{v:.0} {}", units[i]) } else { format!("{v:.1} {}", units[i]) }
}

pub fn clamp<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

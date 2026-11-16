//! Owns the EWW instance: builds a runtime config dir (symlinks to the shipped
//! yuck/scss + generated theme), starts `eww daemon`, opens/closes windows.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

pub struct Eww {
    pub dir: PathBuf,
    bin: String,
    child: Mutex<Option<tokio::process::Child>>,
}

impl Eww {
    pub fn new() -> Self {
        let dir = crate::util::runtime_dir().join("eww");
        let bin = std::env::var("SHEELS_EWW").unwrap_or_else(|_| "eww".into());
        Eww { dir, bin, child: Mutex::new(None) }
    }

    /// Prepare the runtime config directory. Shipped files are symlinked so a
    /// package update is picked up on the next reload; only `_theme.scss` is
    /// generated here.
    pub fn prepare_dir(&self) -> std::io::Result<()> {
        let src = crate::util::share_dir().join("eww");
        std::fs::create_dir_all(&self.dir)?;
        for entry in std::fs::read_dir(&src)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("_theme") {
                continue;
            }
            let dst = self.dir.join(&name);
            let target = entry.path();
            if std::fs::read_link(&dst).map(|t| t == target).unwrap_or(false) {
                continue;
            }
            let _ = std::fs::remove_file(&dst);
            std::os::unix::fs::symlink(&target, &dst)?;
        }
        Ok(())
    }

    fn cmd(&self) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(&self.bin);
        c.arg("-c").arg(&self.dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        c
    }

    pub async fn start(&self) {
        if let Err(e) = self.prepare_dir() {
            eprintln!("sheels: cannot prepare eww dir: {e}");
        }
        // Make sure a stale instance for this config dir is gone.
        let _ = self.cmd().arg("kill").status().await;
        let mut c = self.cmd();
        c.arg("daemon").arg("--no-daemonize");
        if std::env::var_os("SHEELS_EWW_LOG").is_some() {
            c.stderr(Stdio::inherit()).stdout(Stdio::inherit());
        }
        match c.spawn() {
            Ok(child) => *self.child.lock().unwrap() = Some(child),
            Err(e) => {
                eprintln!("sheels: cannot start eww ({}): {e}", self.bin);
                return;
            }
        }
        for _ in 0..100 {
            if matches!(self.cmd().arg("ping").status().await, Ok(s) if s.success()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        eprintln!("sheels: eww did not come up");
    }

    pub async fn open(&self, window: &str, id: &str, screen: Option<&str>, args: &[(&str, String)]) {
        let mut c = self.cmd();
        c.arg("open").arg(window).arg("--id").arg(id);
        if let Some(s) = screen {
            c.arg("--screen").arg(s);
        }
        for (k, v) in args {
            c.arg("--arg").arg(format!("{k}={v}"));
        }
        let _ = c.status().await;
    }

    pub async fn close(&self, ids: &[&str]) {
        if ids.is_empty() {
            return;
        }
        let _ = self.cmd().arg("close").args(ids).status().await;
    }

    pub async fn reload(&self) {
        let _ = self.prepare_dir();
        let _ = self.cmd().arg("reload").status().await;
    }

    pub async fn kill(&self) {
        let _ = self.cmd().arg("kill").status().await;
        let child = self.child.lock().unwrap().take();
        if let Some(mut c) = child {
            let _ = c.kill().await;
        }
    }
}

//! Screen recorder (Caelestia `caelestia record`): wf-recorder, optional
//! region (slurp) and sound. Recordings live in ~/Videos/Recordings.

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::app::app;

#[derive(Default)]
struct Inner {
    child: Option<std::process::Child>,
    started: u64,
    paused: bool,
    file: Option<PathBuf>,
}

#[derive(Default)]
pub struct Recorder {
    inner: Mutex<Inner>,
}

fn dir() -> PathBuf {
    crate::util::home().join("Videos/Recordings")
}

impl Recorder {
    pub fn json(&self) -> Value {
        let mut i = self.inner.lock().unwrap();
        // Reap a recorder that exited on its own.
        if let Some(c) = i.child.as_mut()
            && let Ok(Some(_)) = c.try_wait() {
                i.child = None;
            }
        json!({ "running": i.child.is_some(), "paused": i.paused, "started": i.started / 1000 })
    }

    pub fn list(&self) -> Value {
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir())
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().map(|x| x == "mp4" || x == "mkv" || x == "webm").unwrap_or(false))
                    .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by_key(|x| std::cmp::Reverse(x.0));
        Value::Array(
            files
                .iter()
                .take(8)
                .enumerate()
                .map(|(i, (_, p))| json!({"i": i, "name": p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()}))
                .collect(),
        )
    }

    fn nth(&self, idx: &str) -> Option<PathBuf> {
        let k: usize = idx.parse().ok()?;
        let list = self.list();
        let name = list.get(k)?["name"].as_str()?.to_string();
        Some(dir().join(name))
    }

    pub async fn command(&self, sub: &str, arg: &str) -> Result<String> {
        match sub {
            "" | "toggle" | "fullscreen" | "region" | "sound" | "region-sound" => {
                if self.inner.lock().unwrap().child.is_some() {
                    return self.stop().await;
                }
                let sound = sub.contains("sound") || arg == "--sound";
                let region = sub.starts_with("region") || arg == "--region";
                let _ = std::fs::create_dir_all(dir());
                let file = dir().join(format!("recording_{}.mp4", crate::util::now_ms() / 1000));
                let mut argv: Vec<String> = vec!["wf-recorder".into(), "-f".into(), file.to_string_lossy().into()];
                if region {
                    app().ui.close_all().await;
                    let g = crate::util::run(&["slurp"], 60000).await.unwrap_or_default();
                    if g.trim().is_empty() {
                        return Ok(String::new());
                    }
                    argv.push("-g".into());
                    argv.push(g.trim().into());
                }
                if sound {
                    argv.push("--audio".into());
                }
                let child = std::process::Command::new(&argv[0])
                    .args(&argv[1..])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                match child {
                    Ok(c) => {
                        let mut i = self.inner.lock().unwrap();
                        i.child = Some(c);
                        i.started = crate::util::now_ms();
                        i.paused = false;
                        i.file = Some(file);
                    }
                    Err(e) => bail!("wf-recorder: {e}"),
                }
            }
            "stop" => return self.stop().await,
            "pause" => {
                let mut i = self.inner.lock().unwrap();
                if let Some(c) = i.child.as_ref() {
                    let sig = if i.paused { libc::SIGCONT } else { libc::SIGSTOP };
                    unsafe { libc::kill(c.id() as i32, sig) };
                    i.paused = !i.paused;
                }
            }
            "play" => {
                if let Some(p) = self.nth(arg) {
                    crate::util::spawn_argv(&["xdg-open", &p.to_string_lossy()]);
                }
            }
            "folder" => crate::util::spawn_argv(&["xdg-open", &dir().to_string_lossy()]),
            "delete" => {
                if let Some(p) = self.nth(arg) {
                    let _ = std::fs::remove_file(p);
                }
            }
            _ => bail!("record [fullscreen|region|sound|region-sound|stop|pause|play <i>|delete <i>|folder]"),
        }
        app().publish_status();
        Ok(String::new())
    }

    async fn stop(&self) -> Result<String> {
        let (child, file) = {
            let mut i = self.inner.lock().unwrap();
            (i.child.take(), i.file.take())
        };
        if let Some(c) = child {
            unsafe {
                libc::kill(c.id() as i32, libc::SIGCONT);
                libc::kill(c.id() as i32, libc::SIGINT);
            }
            let mut c = c;
            let _ = tokio::task::spawn_blocking(move || c.wait()).await;
            if let Some(f) = file {
                app().notifs.internal("Recording saved", &format!("Saved to {}", f.display()), "screen_record", false);
            }
        }
        app().publish_status();
        Ok(String::new())
    }
}

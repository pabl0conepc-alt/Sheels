//! Audio via PipeWire's PulseAudio interface. A single long-lived
//! `pactl subscribe` child tells us when anything changes; we then re-query
//! once (debounced) with JSON output. Zero polling.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::app::app;

#[derive(Default, Clone, PartialEq)]
struct Dev {
    name: String,
    desc: String,
    vol: f64,
    muted: bool,
}

#[derive(Default)]
struct Inner {
    available: bool,
    sinks: Vec<Dev>,
    sources: Vec<Dev>,
    default_sink: String,
    default_source: String,
    initialised: bool,
}

#[derive(Default)]
pub struct Audio {
    inner: Mutex<Inner>,
}

fn icon_for(vol: f64, muted: bool) -> &'static str {
    if muted {
        "no_sound"
    } else if vol >= 0.5 {
        "volume_up"
    } else if vol > 0.0 {
        "volume_down"
    } else {
        "volume_mute"
    }
}

fn mic_icon(vol: f64, muted: bool) -> &'static str {
    if muted || vol <= 0.0 { "mic_off" } else { "mic" }
}

fn parse_devs(v: &Value) -> Vec<Dev> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter(|d| !d["name"].as_str().unwrap_or("").ends_with(".monitor"))
                .map(|d| {
                    let vols: Vec<f64> = d["volume"]
                        .as_object()
                        .map(|m| {
                            m.values()
                                .filter_map(|c| {
                                    c["value_percent"].as_str().and_then(|p| p.trim_end_matches('%').trim().parse::<f64>().ok())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let vol = if vols.is_empty() { 0.0 } else { vols.iter().sum::<f64>() / vols.len() as f64 / 100.0 };
                    Dev {
                        name: d["name"].as_str().unwrap_or("").to_string(),
                        desc: d["description"].as_str().unwrap_or("").to_string(),
                        vol,
                        muted: d["mute"].as_bool().unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Audio {
    pub fn json(&self) -> Value {
        let i = self.inner.lock().unwrap();
        let sink = i.sinks.iter().find(|d| d.name == i.default_sink).cloned().unwrap_or_default();
        let src = i.sources.iter().find(|d| d.name == i.default_source).cloned().unwrap_or_default();
        let list = |v: &Vec<Dev>, def: &str| -> Value {
            v.iter().enumerate().map(|(i, d)| json!({"i": i, "name": d.name, "desc": d.desc, "active": d.name == def})).collect()
        };
        json!({
            "available": i.available,
            "vol": (sink.vol * 100.0).round(),
            "muted": sink.muted,
            "icon": icon_for(sink.vol, sink.muted),
            "sink": sink.desc,
            "micVol": (src.vol * 100.0).round(),
            "micMuted": src.muted,
            "micIcon": mic_icon(src.vol, src.muted),
            "source": src.desc,
            "sinks": list(&i.sinks, &i.default_sink),
            "sources": list(&i.sources, &i.default_source),
        })
    }

    async fn refresh(&self) {
        let (sinks, sources, info) = tokio::join!(
            crate::util::run(&["pactl", "-f", "json", "list", "sinks"], 2000),
            crate::util::run(&["pactl", "-f", "json", "list", "sources"], 2000),
            crate::util::run(&["pactl", "-f", "json", "info"], 2000),
        );
        let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
        let (sinks, sources, info) = (parse_devs(&parse(sinks)), parse_devs(&parse(sources)), parse(info));
        let (changed_vol, first) = {
            let mut i = self.inner.lock().unwrap();
            let old_sink = i.sinks.iter().find(|d| d.name == i.default_sink).cloned();
            let old_src = i.sources.iter().find(|d| d.name == i.default_source).cloned();
            i.available = info.is_object();
            i.default_sink = info["default_sink_name"].as_str().unwrap_or("").to_string();
            i.default_source = info["default_source_name"].as_str().unwrap_or("").to_string();
            i.sinks = sinks;
            i.sources = sources;
            let new_sink = i.sinks.iter().find(|d| d.name == i.default_sink).cloned();
            let new_src = i.sources.iter().find(|d| d.name == i.default_source).cloned();
            let vol_changed = old_sink.as_ref().map(|d| (d.vol, d.muted)) != new_sink.as_ref().map(|d| (d.vol, d.muted))
                && old_sink.as_ref().map(|d| &d.name) == new_sink.as_ref().map(|d| &d.name);
            let mic_changed = crate::app::cfg().osd.enable_microphone
                && old_src.as_ref().map(|d| (d.vol, d.muted)) != new_src.as_ref().map(|d| (d.vol, d.muted));
            let first = !i.initialised;
            i.initialised = true;
            (vol_changed || mic_changed, first)
        };
        app().publish_status();
        if changed_vol && !first {
            app().ui.osd().await;
        }
    }

    pub async fn command(&self, mic: bool, sub: &str, arg: &str) -> Result<String> {
        let target = if mic { "@DEFAULT_SOURCE@" } else { "@DEFAULT_SINK@" };
        let (vcmd, mcmd) = if mic { ("set-source-volume", "set-source-mute") } else { ("set-sink-volume", "set-sink-mute") };
        let cfg = crate::app::cfg();
        let inc = (cfg.services.audio_increment * 100.0).round() as i64;
        let max = (cfg.services.max_volume * 100.0).round() as i64;
        drop(cfg);
        let cur = {
            let i = self.inner.lock().unwrap();
            let list = if mic { &i.sources } else { &i.sinks };
            let def = if mic { &i.default_source } else { &i.default_sink };
            list.iter().find(|d| &d.name == def).map(|d| (d.vol * 100.0).round() as i64).unwrap_or(0)
        };
        match sub {
            "up" | "down" => {
                let n = if sub == "up" { cur + inc } else { cur - inc };
                let n = n.clamp(0, max);
                crate::util::run_ok(&["pactl", vcmd, target, &format!("{n}%")], 2000).await;
                if sub == "up" {
                    crate::util::run_ok(&["pactl", mcmd, target, "0"], 2000).await;
                }
            }
            "mute" | "toggle" => {
                crate::util::run_ok(&["pactl", mcmd, target, "toggle"], 2000).await;
            }
            "set" => {
                let n: f64 = arg.trim_end_matches('%').parse().unwrap_or(cur as f64);
                let n = (n.round() as i64).clamp(0, max);
                crate::util::run_ok(&["pactl", vcmd, target, &format!("{n}%")], 2000).await;
            }
            _ => bail!("volume|mic up|down|mute|set <pct>"),
        }
        Ok(String::new())
    }

    pub async fn set_default(&self, source: bool, name: &str) -> Result<String> {
        let cmd = if source { "set-default-source" } else { "set-default-sink" };
        // Accept an index (from the UI) or a device name.
        let resolved = name.parse::<usize>().ok().and_then(|k| {
            let i = self.inner.lock().unwrap();
            let list = if source { &i.sources } else { &i.sinks };
            list.get(k).map(|d| d.name.clone())
        });
        let name = resolved.as_deref().unwrap_or(name);
        if !crate::util::run_ok(&["pactl", cmd, name], 2000).await {
            bail!("cannot set default device");
        }
        Ok(String::new())
    }
}

pub async fn run() {
    let audio = &app().audio;
    if !crate::util::which("pactl") {
        eprintln!("sheels: pactl not found, audio disabled");
        return;
    }
    loop {
        audio.refresh().await;
        let child = tokio::process::Command::new("pactl")
            .arg("subscribe")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = child else {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                l = lines.next_line() => match l {
                    Ok(Some(l)) => {
                        if l.contains("on sink #") || l.contains("on source #") || l.contains("on server") || l.contains("on card #") {
                            deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(25));
                        }
                    }
                    _ => break,
                },
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                    deadline = None;
                    audio.refresh().await;
                }
            }
        }
        let _ = child.kill().await;
        // Server restarted (e.g. pipewire-pulse); retry shortly.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

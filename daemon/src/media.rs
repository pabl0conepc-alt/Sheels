//! MPRIS media players. Signal driven; the playback position is published as
//! (position, timestamp) so EWW interpolates it with EWW_TIME — the daemon
//! never ticks for progress bars.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};
use zbus::MatchRule;

use crate::app::app;
use crate::dbus::{get_all, s, signals_rule};

const PFX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";

#[derive(Default, Clone)]
struct Player {
    bus: String,
    owner: String,
    identity: String,
    status: String,
    title: String,
    artist: String,
    album: String,
    art_url: String,
    trackid: String,
    len_s: f64,
    pos_s: f64,
    pos_ts: f64,
    can_next: bool,
    can_prev: bool,
    can_play: bool,
    can_seek: bool,
}

#[derive(Default)]
struct Inner {
    players: Vec<Player>,
    active: Option<String>,
    art_cache: HashMap<String, String>,
}

#[derive(Default)]
pub struct Media {
    inner: Mutex<Inner>,
    kick: tokio::sync::Notify,
}

fn now_s() -> f64 {
    crate::util::now_ms() as f64 / 1000.0
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Media {
    fn pick_active(i: &mut Inner) {
        let valid = i.active.as_ref().map(|a| i.players.iter().any(|p| &p.bus == a)).unwrap_or(false);
        if valid {
            return;
        }
        let def = crate::app::cfg().services.default_player.to_lowercase();
        i.active = i
            .players
            .iter()
            .find(|p| p.status == "Playing")
            .or_else(|| i.players.iter().find(|p| p.identity.to_lowercase() == def))
            .or(i.players.first())
            .map(|p| p.bus.clone());
    }

    fn publish(&self) {
        let i = self.inner.lock().unwrap();
        let p = i.active.as_ref().and_then(|a| i.players.iter().find(|p| &p.bus == a)).cloned();
        let players: Vec<Value> = i.players.iter().map(|p| json!({"bus": p.bus, "name": p.identity})).collect();
        let v = match p {
            Some(p) => {
                let art = if p.art_url.starts_with("file://") {
                    url_decode(p.art_url.trim_start_matches("file://"))
                } else {
                    i.art_cache.get(&p.art_url).cloned().unwrap_or_default()
                };
                json!({
                    "available": true,
                    "bus": p.bus,
                    "identity": p.identity,
                    "title": if p.title.is_empty() { "Unknown title".to_string() } else { p.title.clone() },
                    "artist": if p.artist.is_empty() { "Unknown artist".to_string() } else { p.artist.clone() },
                    "album": p.album,
                    "art": art,
                    "playing": p.status == "Playing",
                    "status": p.status,
                    "canNext": p.can_next,
                    "canPrev": p.can_prev,
                    "canPlay": p.can_play,
                    "canSeek": p.can_seek,
                    "len": p.len_s.round(),
                    "pos": p.pos_s.round(),
                    "ts": p.pos_ts.round(),
                    "players": players,
                })
            }
            None => json!({"available": false, "title": "No media", "artist": "", "playing": false, "players": players,
                           "len": 0, "pos": 0, "ts": 0, "art": ""}),
        };
        drop(i);
        app().hub.publish("media", &v);
    }

    async fn read_player(conn: &zbus::Connection, bus: &str) -> Option<Player> {
        let pl = get_all(conn, bus, PATH, "org.mpris.MediaPlayer2.Player").await.ok()?;
        let root = get_all(conn, bus, PATH, "org.mpris.MediaPlayer2").await.unwrap_or(Value::Null);
        let md = &pl["Metadata"];
        let artist = match &md["xesam:artist"] {
            Value::Array(a) => a.iter().map(s).collect::<Vec<_>>().join(", "),
            other => s(other),
        };
        let owner = conn
            .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetNameOwner", &(bus,))
            .await
            .ok()
            .and_then(|m| m.body().deserialize::<String>().ok())
            .unwrap_or_default();
        let mut identity = s(&root["Identity"]);
        if identity.is_empty() {
            identity = bus.trim_start_matches(PFX).split('.').next().unwrap_or("").to_string();
        }
        Some(Player {
            bus: bus.to_string(),
            owner,
            identity,
            status: s(&pl["PlaybackStatus"]),
            title: s(&md["xesam:title"]),
            artist,
            album: s(&md["xesam:album"]),
            art_url: s(&md["mpris:artUrl"]),
            trackid: s(&md["mpris:trackid"]),
            len_s: md["mpris:length"].as_f64().unwrap_or(0.0) / 1e6,
            pos_s: pl["Position"].as_f64().unwrap_or(0.0) / 1e6,
            pos_ts: now_s(),
            can_next: pl["CanGoNext"].as_bool().unwrap_or(false),
            can_prev: pl["CanGoPrevious"].as_bool().unwrap_or(false),
            can_play: pl["CanPlay"].as_bool().unwrap_or(true),
            can_seek: pl["CanSeek"].as_bool().unwrap_or(false),
        })
    }

    async fn refresh(&self) {
        let Some(conn) = app().session_bus.get() else { return };
        let names: Vec<String> = match conn
            .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListNames", &())
            .await
        {
            Ok(m) => m.body().deserialize::<Vec<String>>().unwrap_or_default(),
            Err(_) => return,
        };
        let mut players = vec![];
        for n in names.iter().filter(|n| n.starts_with(PFX)) {
            if let Some(p) = Self::read_player(conn, n).await {
                players.push(p);
            }
        }
        let fetch: Vec<String> = {
            let mut i = self.inner.lock().unwrap();
            // A player that just started playing becomes the active one.
            for p in &players {
                let was = i.players.iter().find(|o| o.bus == p.bus).map(|o| o.status.clone()).unwrap_or_default();
                if p.status == "Playing" && was != "Playing" {
                    i.active = Some(p.bus.clone());
                }
            }
            i.players = players;
            Self::pick_active(&mut i);
            i.players
                .iter()
                .filter(|p| p.art_url.starts_with("http") && !i.art_cache.contains_key(&p.art_url))
                .map(|p| p.art_url.clone())
                .collect()
        };
        for url in fetch {
            tokio::spawn(async move { app().media.fetch_art(url).await });
        }
        self.publish();
    }

    async fn fetch_art(&self, url: String) {
        let dir = crate::util::cache_dir().join("covers");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{:016x}", crate::util::fnv1a(url.as_bytes())));
        let p = path.to_string_lossy().into_owned();
        if !path.exists() && !crate::util::run_ok(&["curl", "-sfL", "--max-time", "10", "-o", &p, &url], 12000).await {
            return;
        }
        {
            let mut i = self.inner.lock().unwrap();
            i.art_cache.insert(url, p);
            // Keep the cache bounded.
            if i.art_cache.len() > 64 {
                let k = i.art_cache.keys().next().cloned();
                if let Some(k) = k {
                    i.art_cache.remove(&k);
                }
            }
        }
        self.publish();
    }

    fn active_player(&self) -> Option<Player> {
        let i = self.inner.lock().unwrap();
        i.active.as_ref().and_then(|a| i.players.iter().find(|p| &p.bus == a)).cloned()
    }

    pub async fn command(&self, sub: &str, arg: &str) -> Result<String> {
        let Some(conn) = app().session_bus.get() else { bail!("no session bus") };
        if sub == "player" {
            let mut i = self.inner.lock().unwrap();
            if i.players.is_empty() {
                return Ok(String::new());
            }
            let cur = i.active.as_ref().and_then(|a| i.players.iter().position(|p| &p.bus == a)).unwrap_or(0);
            let next = match arg {
                "" | "next" => (cur + 1) % i.players.len(),
                "prev" => (cur + i.players.len() - 1) % i.players.len(),
                name => i
                    .players
                    .iter()
                    .position(|p| p.identity.eq_ignore_ascii_case(name) || p.bus == name)
                    .unwrap_or(cur),
            };
            i.active = Some(i.players[next].bus.clone());
            drop(i);
            self.publish();
            return Ok(String::new());
        }
        let Some(p) = self.active_player() else { return Ok(String::new()) };
        let iface = Some("org.mpris.MediaPlayer2.Player");
        match sub {
            "play-pause" | "toggle" => {
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "PlayPause", &()).await?;
            }
            "play" => {
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "Play", &()).await?;
            }
            "pause" => {
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "Pause", &()).await?;
            }
            "next" => {
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "Next", &()).await?;
            }
            "previous" | "prev" => {
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "Previous", &()).await?;
            }
            "stop" => {
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "Stop", &()).await?;
            }
            "seek" => {
                // Relative seek in seconds.
                let off: f64 = arg.parse().unwrap_or(0.0);
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "Seek", &((off * 1e6) as i64,)).await?;
            }
            "seek-to" | "position" => {
                // Absolute position: seconds, or a percentage with '%'.
                let secs = if let Some(pc) = arg.strip_suffix('%') {
                    pc.parse::<f64>().unwrap_or(0.0) / 100.0 * p.len_s
                } else {
                    arg.parse::<f64>().unwrap_or(0.0)
                };
                let tid = zbus::ObjectPath::try_from(p.trackid.as_str())
                    .map(|o| o.into_owned())
                    .unwrap_or_else(|_| zbus::ObjectPath::from_static_str("/org/mpris/MediaPlayer2/TrackList/NoTrack").unwrap());
                conn.call_method(Some(p.bus.as_str()), PATH, iface, "SetPosition", &(tid, (secs * 1e6) as i64)).await?;
            }
            "raise" => {
                let _ = conn.call_method(Some(p.bus.as_str()), PATH, Some("org.mpris.MediaPlayer2"), "Raise", &()).await;
            }
            _ => bail!("media play-pause|play|pause|next|previous|stop|seek <s>|seek-to <s|pct%>|player [name]"),
        }
        self.kick.notify_one();
        Ok(String::new())
    }
}

pub async fn run() {
    let media = &app().media;
    let Some(conn) = app().session_bus.get() else { return };
    media.refresh().await;
    let rule_props = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)
        .interface("org.freedesktop.DBus.Properties")
        .build();
    let rule_seek = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)
        .interface("org.mpris.MediaPlayer2.Player")
        .member("Seeked")
        .build();
    let rule_owner = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .arg0ns("org.mpris.MediaPlayer2")
        .build();
    let (Ok(r1), Ok(r2), Ok(r3)) = (rule_props, rule_seek, rule_owner) else { return };
    let (Ok(mut s1), Ok(mut s2), Ok(mut s3)) =
        (signals_rule(conn, r1).await, signals_rule(conn, r2).await, signals_rule(conn, r3).await)
    else {
        return;
    };
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            m = s1.next() => { if m.is_none() { break; } deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(60)); }
            m = s2.next() => {
                let Some(Ok(m)) = m else { break };
                // Seeked: update position of that player directly.
                let sender = m.header().sender().map(|s| s.as_str().to_string()).unwrap_or_default();
                if let Ok(pos) = m.body().deserialize::<i64>() {
                    {
                        let mut i = media.inner.lock().unwrap();
                        if let Some(p) = i.players.iter_mut().find(|p| p.owner == sender) {
                            p.pos_s = pos as f64 / 1e6;
                            p.pos_ts = now_s();
                        }
                    }
                    media.publish();
                }
            }
            m = s3.next() => {
                let Some(Ok(m)) = m else { break };
                let arg0: Option<(String, String, String)> = m.body().deserialize().ok();
                if arg0.map(|(n, _, _)| n.starts_with(PFX)).unwrap_or(false) {
                    deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(60));
                }
            }
            _ = media.kick.notified() => { deadline.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(80)); }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                deadline = None;
                media.refresh().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_decoding() {
        assert_eq!(super::url_decode("/a%20b/c%C3%A9.png"), "/a b/cé.png");
        assert_eq!(super::url_decode("100%"), "100%");
    }
}

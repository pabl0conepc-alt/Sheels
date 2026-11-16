//! Topic hub: every topic holds its latest JSON value. Listeners (EWW
//! `deflisten` processes) receive the current value immediately and then every
//! change. Values are only republished when they actually differ, so EWW never
//! re-renders for nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub const TOPICS: &[&str] = &["wm", "status", "media", "notifs", "sys", "launcher", "lset", "ui", "theme", "info", "cfg", "nexus", "npage"];

#[derive(Clone)]
pub struct Hub {
    inner: Arc<HashMap<&'static str, watch::Sender<Arc<str>>>>,
    /// Number of active listeners per topic is tracked by watch itself; we
    /// additionally expose "demand" flags (e.g. perf stats only while shown).
    demand: Arc<Mutex<HashMap<String, bool>>>,
}

impl Hub {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        for t in TOPICS {
            let (tx, _rx) = watch::channel::<Arc<str>>(Arc::from("{}"));
            map.insert(*t, tx);
        }
        Hub { inner: Arc::new(map), demand: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn publish(&self, topic: &str, value: &serde_json::Value) {
        let s = value.to_string();
        self.publish_str(topic, s);
    }

    pub fn publish_str(&self, topic: &str, s: String) {
        if let Some(tx) = self.inner.get(topic) {
            tx.send_if_modified(|cur| {
                if &**cur == s.as_str() {
                    false
                } else {
                    *cur = Arc::from(s.as_str());
                    true
                }
            });
        }
    }

    pub fn get(&self, topic: &str) -> Option<Arc<str>> {
        self.inner.get(topic).map(|tx| tx.borrow().clone())
    }

    pub fn subscribe(&self, topic: &str) -> Option<watch::Receiver<Arc<str>>> {
        self.inner.get(topic).map(|tx| tx.subscribe())
    }

    pub fn set_demand(&self, key: &str, on: bool) {
        self.demand.lock().unwrap().insert(key.to_string(), on);
    }

    pub fn demand(&self, key: &str) -> bool {
        *self.demand.lock().unwrap().get(key).unwrap_or(&false)
    }
}

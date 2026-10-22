//! Query results: a TTL cache, the last good result per query (served as
//! stale when a backend fails), and an optional snapshot file so the last
//! known state survives a restart.

use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;

#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub data: Option<Value>,
    pub fetched_at: Option<String>,
    pub stale: bool,
    pub quiet: bool,
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    data: Value,
    fetched_at: String,
    #[serde(skip)]
    at: Option<Instant>,
}

pub struct Cache {
    entries: Mutex<HashMap<String, Entry>>,
    snapshot: Option<PathBuf>,
    dirty: AtomicBool,
}

impl Cache {
    pub fn new(state_dir: Option<PathBuf>) -> Result<Self> {
        let snapshot = state_dir.map(|d| d.join("snapshot.json"));
        let mut entries = HashMap::new();
        if let Some(path) = &snapshot {
            std::fs::create_dir_all(path.parent().unwrap())
                .map_err(|e| format!("state_dir {}: {e}", path.display()))?;
            match std::fs::read(path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(e) => entries = e,
                    Err(e) => {
                        tracing::warn!(msg = "snapshot unreadable, starting empty", error = %e)
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("reading {}: {e}", path.display()).into()),
            }
        }
        Ok(Self {
            entries: Mutex::new(entries),
            snapshot,
            dirty: AtomicBool::new(false),
        })
    }

    /// Fresh cached data, else `fetch`. When `fetch` fails, the last good data
    /// is returned with `stale: true` and the error.
    // ponytail: two concurrent misses on one key both fetch; add a per-key lock if a backend minds
    pub async fn get<F, Fut>(&self, key: &str, ttl: Duration, quiet: bool, fetch: F) -> Envelope
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value>>,
    {
        if let Some(e) = self.entries.lock().unwrap().get(key)
            && e.at.is_some_and(|at| at.elapsed() < ttl)
        {
            return envelope(Some(e), false, quiet, None);
        }
        let started = Instant::now();
        let result = fetch().await;
        let ms = started.elapsed().as_millis() as u64;
        let mut entries = self.entries.lock().unwrap();
        match result {
            Ok(data) => {
                tracing::debug!(msg = "query", source = key, ms, status = "ok");
                let entry = Entry {
                    data,
                    fetched_at: jiff::Timestamp::now().to_string(),
                    at: Some(Instant::now()),
                };
                let env = envelope(Some(&entry), false, quiet, None);
                entries.insert(key.to_string(), entry);
                self.dirty.store(true, Ordering::Relaxed);
                env
            }
            Err(e) => {
                let status = if quiet {
                    "quiet"
                } else if entries.contains_key(key) {
                    "stale"
                } else {
                    "error"
                };
                if quiet {
                    tracing::debug!(msg = "query", source = key, ms, status, error = %e);
                } else {
                    tracing::warn!(msg = "query", source = key, ms, status, error = %e);
                }
                envelope(entries.get(key), true, quiet, Some(e.to_string()))
            }
        }
    }

    /// Drops cached results whose key starts with `prefix`, so the next poll
    /// after an action shows its effect.
    pub fn invalidate(&self, prefix: &str) {
        for e in self
            .entries
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|(k, _)| k.starts_with(prefix))
        {
            e.1.at = None;
        }
    }

    /// Writes the snapshot file at most every 30 s, only when something changed.
    pub async fn persist_loop(&self) {
        let Some(path) = &self.snapshot else { return };
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            if !self.dirty.swap(false, Ordering::Relaxed) {
                continue;
            }
            let bytes =
                serde_json::to_vec(&*self.entries.lock().unwrap()).expect("JSON values serialize");
            let tmp = path.with_extension("json.tmp");
            let written = tokio::fs::write(&tmp, bytes)
                .await
                .and(tokio::fs::rename(&tmp, path).await);
            if let Err(e) = written {
                tracing::warn!(msg = "snapshot write failed", path = %path.display(), error = %e);
            }
        }
    }
}

fn envelope(e: Option<&Entry>, stale: bool, quiet: bool, error: Option<String>) -> Envelope {
    Envelope {
        data: e.map(|e| e.data.clone()),
        fetched_at: e.map(|e| e.fetched_at.clone()),
        stale: stale && e.is_some(),
        quiet,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn stale_fallback_and_snapshot() {
        let dir = std::env::temp_dir().join(format!("homie-cache-test-{}", std::process::id()));
        let cache = Cache::new(Some(dir.clone())).unwrap();
        let ttl = Duration::from_secs(60);

        let ok = cache
            .get("pve:lab", ttl, false, || async { Ok(json!({"n": 1})) })
            .await;
        assert_eq!(ok.data, Some(json!({"n": 1})));
        assert!(!ok.stale);

        // within the TTL the fetch is not called
        let hit = cache
            .get("pve:lab", ttl, false, || async {
                Err("must not run".into())
            })
            .await;
        assert!(hit.error.is_none());

        cache.invalidate("pve:");
        let failed = cache
            .get("pve:lab", ttl, true, || async { Err("down".into()) })
            .await;
        assert!(failed.stale && failed.quiet);
        assert_eq!(failed.data, Some(json!({"n": 1})));
        assert_eq!(failed.error.as_deref(), Some("down"));

        let never = cache
            .get("k8s:x", ttl, false, || async { Err("down".into()) })
            .await;
        assert!(never.data.is_none() && !never.stale);

        // snapshot round trip: loaded entries are always refetched, and served when that fails
        std::fs::write(
            dir.join("snapshot.json"),
            serde_json::to_vec(&*cache.entries.lock().unwrap()).unwrap(),
        )
        .unwrap();
        let reloaded = Cache::new(Some(dir.clone())).unwrap();
        let after = reloaded
            .get("pve:lab", ttl, false, || async { Err("still down".into()) })
            .await;
        assert!(after.stale);
        assert_eq!(after.data, Some(json!({"n": 1})));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

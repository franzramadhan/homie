//! Action history (RFC 0027): who did what and when. The last `history.size`
//! entries, in memory and, with a `state_dir`, in `history.jsonl` (0600):
//! appended per entry, rewritten to the cap at start and when it doubles.

use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Shared;
use crate::{
    cache::Envelope,
    config::Config,
    integrations::{Caller, argocd, flux, proxmox},
};

/// Longest kept text: a failed login's user name is whatever was typed.
const MAX_TEXT: usize = 256;
/// A failing write is logged once, then at most this often.
const ERROR_EVERY: Duration = Duration::from_secs(3600);
/// What a role reads instead of someone else's name.
const ANOTHER: &str = "another user";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub time: String,
    /// email, user name, `demo` or `local`; none when an OIDC login failed
    /// before naming anyone
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// a digest of the session id (never the id): how a role finds its own
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub action: String,
    /// `pve/<name>/<node>/<vmid>`, `flux/<cluster>/<kind>/<ns>/<name>`,
    /// `argocd/<cluster>/<ns>/<name>`, the login method
    pub target: String,
    /// `ok`, `error` or `denied`
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_TEXT).collect()
}

/// The first 8 bytes of SHA-256 of the session id, in hex.
pub fn session(sid: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, sid.as_bytes()).as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Entry {
    pub fn new(
        user: Option<&str>,
        sid: Option<&str>,
        action: &str,
        target: &str,
        result: &str,
        error: Option<String>,
    ) -> Self {
        Self {
            time: jiff::Timestamp::now().to_string(),
            user: user.map(clip),
            session: sid.map(session),
            action: clip(action),
            target: clip(target),
            result: result.into(),
            error: error.as_deref().map(clip),
        }
    }

    /// An entry for what `caller` did.
    pub fn by(
        caller: &Caller,
        action: &str,
        target: &str,
        result: &str,
        error: Option<String>,
    ) -> Self {
        let user = caller.email.as_deref().unwrap_or("local");
        Self::new(
            Some(user),
            caller.sid.as_deref(),
            action,
            target,
            result,
            error,
        )
    }
}

struct Inner {
    entries: VecDeque<Entry>,
    /// lines in the file, to rewrite it when it reaches twice the cap
    lines: usize,
    failed: Option<Instant>,
}

pub struct History {
    enabled: bool,
    size: usize,
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
}

/// Writes `entries` to `path` through a temporary file, mode 0600.
fn rewrite<'a>(path: &Path, entries: impl Iterator<Item = &'a Entry>) -> std::io::Result<()> {
    let mut text = String::new();
    for e in entries {
        text.push_str(&serde_json::to_string(e).expect("entries serialize"));
        text.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

fn append(path: &Path, e: &Entry) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let line = format!("{}\n", serde_json::to_string(e).expect("entries serialize"));
    o.open(path)?.write_all(line.as_bytes())
}

impl History {
    /// Loads `state_dir/history.jsonl`: lines that do not parse (a write cut
    /// short) are skipped, and the file is rewritten to the last `size`.
    pub fn new(cfg: &Config) -> crate::Result<Self> {
        let h = &cfg.history;
        let file = cfg
            .state_dir
            .as_ref()
            .filter(|_| h.enabled)
            .map(|d| d.join("history.jsonl"));
        let mut entries = VecDeque::new();
        if let Some(path) = &file {
            std::fs::create_dir_all(path.parent().unwrap())
                .map_err(|e| format!("state_dir {}: {e}", path.display()))?;
            match std::fs::read_to_string(path) {
                Ok(text) => {
                    let mut skipped = 0;
                    for line in text.lines() {
                        match serde_json::from_str::<Entry>(line) {
                            Ok(e) => entries.push_back(e),
                            Err(_) => skipped += 1,
                        }
                    }
                    if skipped > 0 {
                        tracing::warn!(msg = "history", skipped, note = "unreadable lines dropped");
                    }
                    while entries.len() > h.size {
                        entries.pop_front();
                    }
                    rewrite(path, entries.iter())
                        .map_err(|e| format!("writing {}: {e}", path.display()))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("reading {}: {e}", path.display()).into()),
            }
        }
        Ok(Self {
            enabled: h.enabled,
            size: h.size,
            file,
            inner: Mutex::new(Inner {
                lines: entries.len(),
                entries,
                failed: None,
            }),
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether entries survive a restart.
    pub fn kept(&self) -> bool {
        self.file.is_some()
    }

    pub fn push(&self, e: Entry) {
        if !self.enabled {
            return;
        }
        let mut i = self.inner.lock().unwrap();
        i.entries.push_back(e);
        while i.entries.len() > self.size {
            i.entries.pop_front();
        }
        let Some(path) = &self.file else { return };
        let written = if i.lines + 1 >= 2 * self.size {
            rewrite(path, i.entries.iter()).map(|()| i.entries.len())
        } else {
            append(path, i.entries.back().unwrap()).map(|()| i.lines + 1)
        };
        match written {
            Ok(lines) => i.lines = lines,
            Err(e) => {
                if i.failed.is_none_or(|f| f.elapsed() >= ERROR_EVERY) {
                    tracing::warn!(msg = "history", path = %path.display(), error = %e, result = "error");
                    i.failed = Some(Instant::now());
                }
            }
        }
    }

    /// Newest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.inner
            .lock()
            .unwrap()
            .entries
            .iter()
            .rev()
            .cloned()
            .collect()
    }
}

/// The Proxmox or cluster a target names, as the query that lists it.
fn source(target: &str) -> Option<(&'static str, &str)> {
    let (kind, rest) = target.split_once('/')?;
    match kind {
        "pve" => Some(("pve", rest.rsplitn(3, '/').nth(2)?)),
        "flux" => Some(("flux", rest.split('/').next()?)),
        _ => None,
    }
}

/// Whether `target` is in a role's own view of its source (`data`, as the
/// role's query returned it): a guest with the role's tags, a Flux object
/// deploying only into its namespaces. Anything else is not.
fn visible(target: &str, data: Option<&Value>) -> bool {
    let Some(data) = data else { return false };
    let rows = |v: &Value| v.as_array().cloned().unwrap_or_default();
    if let Some(rest) = target.strip_prefix("pve/") {
        let mut p = rest.rsplitn(3, '/');
        let (Some(vmid), Some(node)) = (p.next(), p.next()) else {
            return false;
        };
        return rows(&data["guests"]).iter().any(|g| {
            g["node"] == node && g["vmid"].as_u64().is_some_and(|v| v.to_string() == vmid)
        });
    }
    if let Some(rest) = target.strip_prefix("flux/") {
        let p: Vec<&str> = rest.splitn(4, '/').collect();
        let [_, kind, ns, name] = p[..] else {
            return false;
        };
        return rows(data)
            .iter()
            .any(|i| i["kind"] == kind && i["namespace"] == ns && i["name"] == name);
    }
    false
}

/// The history as `caller` may see it. A role sees its own session's
/// entries and those whose target is in its view; anyone else's name reads
/// "another user", and addresses in errors are redacted.
async fn view(state: &Shared, caller: &Caller) -> Vec<Value> {
    let entries = state.history.entries();
    let row = |e: Entry, user: Option<String>, error: Option<String>| {
        json!({ "time": e.time, "user": user, "action": e.action, "target": e.target,
                "result": e.result, "error": error })
    };
    if caller.role.is_none() {
        return entries
            .into_iter()
            .map(|e| {
                let (user, error) = (e.user.clone(), e.error.clone());
                row(e, user, error)
            })
            .collect();
    }
    let own = caller.sid.as_deref().map(session);
    let mut seen: HashMap<(&str, String), Option<Value>> = HashMap::new();
    for e in &entries {
        if let Some((kind, name)) = source(&e.target)
            && !seen.contains_key(&(kind, name.to_string()))
        {
            let env = state.lab.query(&state.cache, kind, name, caller).await;
            seen.insert((kind, name.to_string()), env.and_then(|e| e.data));
        }
    }
    entries
        .into_iter()
        .filter_map(|e| {
            let mine = own.is_some() && e.session == own;
            let data = source(&e.target).and_then(|(k, n)| seen.get(&(k, n.to_string()))?.as_ref());
            if !mine && !visible(&e.target, data) {
                return None;
            }
            let user = if mine {
                e.user.clone()
            } else {
                Some(ANOTHER.into())
            };
            let error = e.error.as_deref().map(crate::roles::redact_ips);
            Some(row(e, user, error))
        })
        .collect()
}

fn page(caller: &Caller) -> bool {
    caller
        .role
        .as_ref()
        .is_none_or(|r| r.role.pages.iter().any(|p| p == "history"))
}

/// The history as a query envelope (`/api/q/history/_` and the live stream),
/// `None` when it is off or not one of the role's pages. `fetched_at` is the
/// newest entry's time, so an unchanged history is not sent again.
pub(super) async fn envelope(state: &Shared, caller: &Caller) -> Option<Envelope> {
    if !state.history.enabled() || !page(caller) {
        return None;
    }
    let rows = view(state, caller).await;
    Some(Envelope {
        fetched_at: rows
            .first()
            .map(|r| r["time"].as_str().unwrap_or_default().to_string()),
        data: Some(Value::Array(rows)),
        stale: false,
        quiet: false,
        error: None,
        tone: None,
        reasons: Vec::new(),
    })
}

/// `GET /api/history`: the rows alone, for scripts.
pub async fn list(State(state): State<Shared>, Extension(caller): Extension<Caller>) -> Response {
    match envelope(&state, &caller).await {
        Some(env) => Json(env.data).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "the history is off or not in this view" })),
        )
            .into_response(),
    }
}

/// A role's refused action (403 in the guard), recorded as `denied` with the
/// target its body names, as the handler would have. The body is read for
/// that only, and never kept.
pub(super) async fn denied(state: &Shared, caller: &Caller, req: Request) {
    if !state.history.enabled() {
        return;
    }
    let path = req.uri().path().to_string();
    let mut p = path
        .strip_prefix("/api/act/")
        .unwrap_or_default()
        .split('/');
    let (Some(kind), Some(name), Some(op)) = (p.next(), p.next(), p.next()) else {
        return;
    };
    let body = axum::body::to_bytes(req.into_body(), 4096)
        .await
        .unwrap_or_default();
    let parsed = match (kind, op) {
        ("pve", "power") => serde_json::from_slice::<proxmox::PowerRequest>(&body)
            .ok()
            .filter(|r| matches!(r.action.as_str(), "start" | "shutdown" | "reboot" | "stop"))
            .map(|r| {
                (
                    format!("pve.guest.{}", r.action),
                    format!("{}/{}", r.node, r.vmid),
                )
            }),
        ("pve", "console") => serde_json::from_slice::<proxmox::ConsoleRequest>(&body)
            .ok()
            .map(|r| ("console.open".into(), format!("{}/{}", r.node, r.vmid))),
        ("flux", "reconcile") => serde_json::from_slice::<flux::ReconcileRequest>(&body)
            .ok()
            .map(|r| {
                (
                    "flux.reconcile".into(),
                    format!("{}/{}/{}", r.kind, r.namespace, r.name),
                )
            }),
        ("argocd", "refresh") => serde_json::from_slice::<argocd::RefreshRequest>(&body)
            .ok()
            .map(|r| {
                (
                    "argocd.refresh".into(),
                    format!("{}/{}", r.namespace, r.name),
                )
            }),
        _ => return,
    };
    let (action, target) = match parsed {
        Some((a, t)) => (a, format!("{kind}/{name}/{t}")),
        None => (format!("{kind}.{op}"), format!("{kind}/{name}")),
    };
    state.record(Entry::by(
        caller,
        &action,
        &target,
        "denied",
        Some("not available in this view".into()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(dir: Option<&Path>, size: usize) -> Config {
        Config {
            state_dir: dir.map(Path::to_path_buf),
            history: crate::config::History {
                enabled: true,
                size,
            },
            ..Config::default()
        }
    }

    fn entry(n: usize) -> Entry {
        Entry::new(
            Some("me@example.com"),
            Some("sid"),
            "flux.reconcile",
            &format!("flux/main/Kustomization/apps/a{n}"),
            "ok",
            None,
        )
    }

    #[test]
    fn capped_kept_and_private() {
        crate::server::harness::logs();
        let dir = crate::server::harness::temp_dir("history");
        let path = dir.join("history.jsonl");
        let h = History::new(&cfg(Some(&dir), 3)).unwrap();
        assert!(h.kept() && h.enabled());
        for n in 0..5 {
            h.push(entry(n));
        }
        let targets = |h: &History| {
            h.entries()
                .iter()
                .map(|e| e.target.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            targets(&h),
            ["a4", "a3", "a2"].map(|t| format!("flux/main/Kustomization/apps/{t}"))
        );
        // rewritten when the file reached twice the cap
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!(lines <= 5, "{lines} lines");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // a restart keeps the last entries; a line cut short is skipped
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"time\":\"2026-");
        std::fs::write(&path, text).unwrap();
        let again = History::new(&cfg(Some(&dir), 2)).unwrap();
        assert_eq!(targets(&again), targets(&h)[..2]);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().lines().count(),
            2,
            "trimmed at start"
        );
        again.push(entry(9));
        let last = std::fs::read_to_string(&path).unwrap();
        let last: Entry = serde_json::from_str(last.lines().last().unwrap()).unwrap();
        assert_eq!(last, again.entries()[0]);
        assert_eq!(last.session.as_deref(), Some(session("sid").as_str()));
        assert!(
            !std::fs::read_to_string(&path).unwrap().contains("\"sid\""),
            "never the session id"
        );
    }

    #[test]
    fn off_memory_only_and_failing_writes() {
        crate::server::harness::logs();
        let dir = crate::server::harness::temp_dir("history-off");
        let mut c = cfg(Some(&dir), 10);
        c.history.enabled = false;
        let off = History::new(&c).unwrap();
        off.push(entry(1));
        assert!(off.entries().is_empty() && !off.kept());
        assert!(!dir.join("history.jsonl").exists());

        let mem = History::new(&cfg(None, 10)).unwrap();
        mem.push(entry(1));
        assert_eq!(mem.entries().len(), 1);
        assert!(!mem.kept());

        // the file cannot be written: kept in memory, logged once
        let h = History::new(&cfg(Some(&dir), 10)).unwrap();
        std::fs::create_dir_all(dir.join("history.jsonl")).unwrap();
        h.push(entry(1));
        h.push(entry(2));
        assert_eq!(h.entries().len(), 2);
        assert!(h.inner.lock().unwrap().failed.is_some());
        assert!(
            History::new(&cfg(Some(&dir), 10)).is_err(),
            "an unreadable file stops the start"
        );
    }

    #[test]
    fn long_text_is_clipped() {
        let e = Entry::new(
            Some(&"x".repeat(5000)),
            None,
            "login",
            "local",
            "denied",
            Some("y".repeat(5000)),
        );
        assert_eq!(e.user.unwrap().len(), MAX_TEXT);
        assert_eq!(e.error.unwrap().len(), MAX_TEXT);
        assert_eq!(session("a").len(), 16);
    }

    #[test]
    fn targets_in_a_roles_view() {
        let pve = json!({"guests": [{"vmid": 101, "node": "pve1"}]});
        assert_eq!(source("pve/lab/pve1/101"), Some(("pve", "lab")));
        assert_eq!(source("pve/a/b/pve1/101"), Some(("pve", "a/b")));
        assert!(visible("pve/lab/pve1/101", Some(&pve)));
        assert!(!visible("pve/lab/pve2/101", Some(&pve)), "another node");
        assert!(!visible("pve/lab/pve1/102", Some(&pve)));
        assert!(
            !visible("pve/lab/pve1/101", None),
            "no view of that Proxmox"
        );
        assert!(!visible("pve/lab", Some(&pve)));
        let flux = json!([{"kind": "Kustomization", "namespace": "apps", "name": "web"}]);
        assert_eq!(
            source("flux/main/Kustomization/apps/web"),
            Some(("flux", "main"))
        );
        assert!(visible("flux/main/Kustomization/apps/web", Some(&flux)));
        assert!(!visible("flux/main/HelmRelease/apps/web", Some(&flux)));
        assert!(!visible("flux/main/Kustomization/apps", Some(&flux)));
        for t in ["argocd/main/argocd/app", "local", "", "github/session"] {
            assert_eq!(source(t), None, "{t}");
            assert!(!visible(t, Some(&flux)), "{t}");
        }
    }
}

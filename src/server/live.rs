//! Live updates (RFC 0009): one server-sent event stream per tab. The server
//! runs the tab's queries through the cache on every refresh tick, and right
//! away when an action or a Kubernetes watch invalidates one, then sends the
//! envelopes that changed.

use std::{
    collections::{BTreeSet, HashMap},
    convert::Infallible,
    hash::{DefaultHasher, Hash, Hasher},
    time::Duration,
};

use axum::{
    Extension,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::{StreamExt, future::join_all};
use kube::{
    Api,
    api::{ApiResource, DynamicObject, PartialObjectMeta},
    core::GroupVersionKind,
    runtime::{WatchStreamExt, watcher},
};
use serde::Deserialize;
use tokio::{sync::mpsc, task::JoinSet};

use super::{Shared, guard};
use crate::integrations::{Caller, argocd, flux};

/// Queries per stream: a page shows a handful, this only bounds the work.
const MAX_KEYS: usize = 64;
/// Minimum time between two rounds, however busy the watches are.
const MIN_ROUND: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
pub struct Params {
    /// `kind/name` pairs separated by commas, as in `/api/q/{kind}/{name}`
    q: String,
}

fn keys(q: &str) -> Vec<(String, String)> {
    q.split(',')
        .filter_map(|k| k.split_once('/'))
        .filter(|(k, n)| !k.is_empty() && !n.is_empty())
        .map(|(k, n)| (k.to_string(), n.to_string()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_KEYS)
        .collect()
}

pub async fn stream(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    Query(p): Query<Params>,
) -> Response {
    if !state.cfg.live_updates {
        return StatusCode::NOT_FOUND.into_response();
    }
    let sid = state
        .auth
        .as_ref()
        .and_then(|a| guard::cookie(&headers, a.cookie_name()).map(String::from));
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(run(state, caller, sid, keys(&p.q), tx));
    let events = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx))
    });
    let mut resp = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    // nginx and friends buffer responses unless told otherwise
    resp.headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    resp
}

/// Ends when the tab goes away (the channel closes) or the session ends.
/// Dropping `_watches` (on return or a new token) stops its Kubernetes watches.
async fn run(
    state: Shared,
    mut caller: Caller,
    sid: Option<String>,
    keys: Vec<(String, String)>,
    tx: mpsc::Sender<Event>,
) {
    let mut changed = state.cache.subscribe();
    let mut sent: HashMap<&(String, String), u64> = HashMap::new();
    let mut _watches = JoinSet::new();
    let mut watch_token = None;
    let refresh = Duration::from_secs(state.cfg.refresh_seconds);
    loop {
        changed.borrow_and_update();
        // the session can end, or its ID token be refreshed, while the tab stays open
        if let (Some(auth), Some(sid)) = (&state.auth, &sid) {
            match auth.user(sid).await {
                Some(u) => caller.id_token = u.id_token,
                None => return,
            }
        }
        // roles (public demos) never add apiserver watches: refresh tick only
        if caller.role.is_none() && watch_token.as_ref() != Some(&caller.id_token) {
            _watches = watch(&state, &caller, &keys);
            watch_token = Some(caller.id_token.clone());
        }
        let envelopes = join_all(
            keys.iter()
                .map(|(k, n)| state.lab.query(&state.cache, k, n, &caller)),
        )
        .await;
        for (key, env) in keys.iter().zip(envelopes) {
            let Some(env) = env else { continue };
            let data =
                serde_json::json!({ "key": format!("{}/{}", key.0, key.1), "envelope": env })
                    .to_string();
            let mut h = DefaultHasher::new();
            data.hash(&mut h);
            let h = h.finish();
            if sent.insert(key, h) == Some(h) {
                continue;
            }
            if tx.send(Event::default().data(data)).await.is_err() {
                return;
            }
        }
        tokio::select! {
            _ = tx.closed() => return,
            _ = tokio::time::sleep(MIN_ROUND) => {}
        }
        tokio::select! {
            _ = tx.closed() => return,
            _ = tokio::time::sleep(refresh.saturating_sub(MIN_ROUND)) => {}
            r = changed.changed() => if r.is_err() { return },
        }
    }
}

/// Resources found missing (`cluster/apiVersion/Kind`), so later streams
/// skip them.
// ponytail: never forgotten; installing a CRD needs a Homie restart to get its watch
fn absent() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static ABSENT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    ABSENT.get_or_init(Default::default)
}

/// Metadata-only watches for the Kubernetes queries in `keys`. Nothing is
/// stored: an event only marks the cached query stale, and the stream's next
/// round fetches it. Initial lists (and relists after a reconnect) are
/// skipped; the refresh tick covers what a reconnect missed.
fn watch(state: &Shared, caller: &Caller, keys: &[(String, String)]) -> JoinSet<()> {
    let mut set = JoinSet::new();
    let core = |kind: &str| GroupVersionKind::gvk("", "v1", kind);
    for (kind, name) in keys {
        let resources: Vec<ApiResource> = match kind.as_str() {
            "k8s" => [
                core("Pod"),
                core("Node"),
                GroupVersionKind::gvk("apps", "v1", "Deployment"),
            ]
            .iter()
            .map(ApiResource::from_gvk)
            .collect(),
            "flux" => flux::KINDS
                .iter()
                .filter_map(|k| flux::resource(k.2))
                .collect(),
            "argocd" => vec![argocd::resource()],
            _ => continue,
        };
        let Some(client) = state
            .lab
            .kubernetes
            .get(name)
            .and_then(|k| k.client(caller.id_token.as_deref()).ok())
        else {
            continue;
        };
        for ar in resources {
            let absent_key = format!("{name}/{}/{}", ar.api_version, ar.kind);
            if absent().lock().unwrap().contains(&absent_key) {
                continue;
            }
            // metadata-only requests: no object bodies cross the wire
            let api = Api::<PartialObjectMeta<DynamicObject>>::all_with(client.clone(), &ar);
            let (state, prefix) = (state.clone(), format!("{kind}:{name}"));
            set.spawn(async move {
                let mut events =
                    std::pin::pin!(watcher::watcher(api, watcher::Config::default()).default_backoff());
                while let Some(e) = events.next().await {
                    match e {
                        Ok(watcher::Event::Apply(_) | watcher::Event::Delete(_)) => {
                            state.cache.invalidate(&prefix)
                        }
                        Ok(_) => {}
                        // the CRD is not installed: nothing to watch
                        Err(
                            watcher::Error::InitialListFailed(kube::Error::Api(s))
                            | watcher::Error::WatchStartFailed(kube::Error::Api(s)),
                        ) if s.code == 404 => {
                            absent().lock().unwrap().insert(absent_key);
                            return;
                        }
                        Err(e) => {
                            tracing::debug!(msg = "watch", source = prefix, kind = ar.kind, error = %e)
                        }
                    }
                }
            });
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_keys() {
        assert_eq!(
            keys("k8s/main,pve/lab,,bad,/x,k8s/main,security/a/b"),
            vec![
                ("k8s".into(), "main".into()),
                ("pve".into(), "lab".into()),
                ("security".into(), "a/b".into()),
            ]
        );
        let many = (0..100)
            .map(|i| format!("pve/{i}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(keys(&many).len(), MAX_KEYS);
    }
}

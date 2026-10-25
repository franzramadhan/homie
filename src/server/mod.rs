//! The HTTP server: JSON API, login routes and the embedded web UI.

mod api;
mod assets;
mod console;
mod github;
mod guard;
#[cfg(test)]
pub(crate) mod harness;
mod history;
mod live;
mod login;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::{
    Router,
    middleware::from_fn_with_state,
    routing::{get, post},
};

use crate::{Result, auth::Auth, cache::Cache, config::Config, integrations::Lab};

pub struct AppState {
    pub cfg: Config,
    pub lab: Lab,
    pub cache: Cache,
    pub auth: Option<Auth>,
    /// GitHub picker helper: pending OAuth states (user tokens live in the
    /// GitHub integration, per session)
    pub github_states:
        std::sync::Mutex<std::collections::HashMap<String, (String, std::time::Instant)>>,
    /// consoles opened and not yet connected (RFC 0023), by single-use id
    pub consoles: console::Consoles,
    /// who did what and when (RFC 0027)
    pub history: history::History,
}

impl AppState {
    /// Keeps a history entry and wakes the live streams.
    fn record(&self, e: history::Entry) {
        if self.history.enabled() {
            self.history.push(e);
            self.cache.invalidate("history:");
        }
    }
}

pub type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/api/meta", get(api::meta))
        .route("/api/q/{kind}/{name}", get(api::query))
        .route("/api/stream", get(live::stream))
        .route("/api/act/pve/{name}/power", post(api::pve_power))
        .route("/api/act/pve/{name}/task", get(api::pve_task))
        .route("/api/act/pve/{name}/console", post(console::open))
        .route("/api/act/pve/{name}/console/{id}", get(console::connect))
        .route("/api/act/flux/{name}/reconcile", post(api::flux_reconcile))
        .route("/api/act/argocd/{name}/refresh", post(api::argocd_refresh))
        .route("/api/logs", post(api::search_logs))
        .route("/api/history", get(history::list))
        .route("/api/github/repos", get(github::repos))
        .route("/api/github/workflows", get(github::workflows))
        .route("/api/github/watch", post(github::save_watch))
        .route("/github/login", get(github::login))
        .route("/github/callback", get(github::callback))
        .route("/auth/logout", post(login::logout))
        .layer(from_fn_with_state(state.clone(), guard::session));
    Router::new()
        .merge(api)
        .route("/auth/options", get(login::options))
        .route("/auth/local", post(login::local))
        .route("/auth/demo", post(login::demo))
        .route("/auth/login", get(login::login))
        .route("/auth/callback", get(login::callback))
        .route("/healthz", get(|| async { "ok" }))
        .fallback(assets::serve)
        .layer(from_fn_with_state(state.clone(), guard::host))
        .with_state(state)
}

pub async fn run(cfg: Config) -> Result<()> {
    let lab = Lab::new(&cfg).await?;
    let cache = Cache::new(cfg.state_dir.clone())?;
    let auth = Auth::new(&cfg).await?;
    let history = history::History::new(&cfg)?;
    if auth.is_none() && cfg.allow_unauthenticated {
        tracing::warn!(
            msg = "insecure",
            integration = "homie",
            reason = "unauthenticated"
        );
    }
    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .map_err(|e| format!("listen {}: {e}", cfg.listen))?;
    let metrics = match &cfg.metrics {
        Some(m) => {
            let token = match &m.token {
                Some(t) => Some(t.resolve().await?),
                None => None,
            };
            let l = tokio::net::TcpListener::bind(&m.listen)
                .await
                .map_err(|e| format!("metrics.listen {}: {e}", m.listen))?;
            Some((l, token))
        }
        None => None,
    };
    let metrics_addr = cfg.metrics.as_ref().map(|m| m.listen.as_str());
    tracing::info!(msg = "listening", addr = %cfg.listen, metrics = metrics_addr, login = auth.is_some(), version = crate::VERSION);

    let state = Arc::new(AppState {
        cfg,
        lab,
        cache,
        auth,
        github_states: Default::default(),
        consoles: Default::default(),
        history,
    });
    let persist = state.clone();
    tokio::spawn(async move { persist.cache.persist_loop().await });
    if state.cfg.notify.is_some() {
        tokio::spawn(crate::notify::run(state.clone()));
    }
    if let Some((l, token)) = metrics {
        let app = metrics_router(state.clone(), token);
        tokio::spawn(async move { axum::serve(l, app).with_graceful_shutdown(shutdown()).await });
    }

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Only `GET /metrics` (RFC 0024): no session, no Host check, an optional
/// bearer token.
fn metrics_router(state: Shared, token: Option<String>) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .with_state((state, token.map(Arc::new)))
}

async fn metrics(
    axum::extract::State((state, token)): axum::extract::State<(Shared, Option<Arc<String>>)>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse};
    if let Some(t) = &token {
        let given = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        // equal digests: the comparison time says nothing about the token
        let digest = |s: &str| ring::digest::digest(&ring::digest::SHA256, s.as_bytes());
        if digest(given).as_ref() != digest(t).as_ref() {
            tracing::warn!(msg = "metrics", result = "denied");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        crate::metrics::scrape(&state).await,
    )
        .into_response()
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}

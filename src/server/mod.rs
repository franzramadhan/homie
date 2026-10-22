//! The HTTP server: JSON API, login routes and the embedded web UI.

mod api;
mod assets;
mod guard;
mod login;

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
}

pub type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/api/meta", get(api::meta))
        .route("/api/q/{kind}/{name}", get(api::query))
        .route("/api/act/pve/{name}/power", post(api::pve_power))
        .route("/api/act/pve/{name}/task", get(api::pve_task))
        .route("/api/act/flux/{name}/reconcile", post(api::flux_reconcile))
        .route("/auth/logout", post(login::logout))
        .layer(from_fn_with_state(state.clone(), guard::session));
    Router::new()
        .merge(api)
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
    let auth = match &cfg.auth {
        Some(a) => Some(Auth::new(a, &cfg.public_url).await?),
        None => None,
    };
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
    tracing::info!(msg = "listening", addr = %cfg.listen, login = auth.is_some(), version = crate::VERSION);

    let state = Arc::new(AppState {
        cfg,
        lab,
        cache,
        auth,
    });
    let persist = state.clone();
    tokio::spawn(async move { persist.cache.persist_loop().await });

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
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

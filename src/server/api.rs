//! JSON API: what is configured (`/api/meta`), named queries (`/api/q`) and
//! actions (`/api/act`). Every action is logged with the acting user.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::Shared;
use crate::integrations::{Caller, argocd, flux, logs, proxmox::PowerRequest};

fn error(status: StatusCode, msg: impl ToString) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

pub async fn meta(State(state): State<Shared>, Extension(caller): Extension<Caller>) -> Response {
    let lab = &state.lab;
    // a role (RFC 0016) sees only its pages, and no links to other UIs
    let role = caller.role.as_ref().map(|r| &r.role);
    let page = |p: &str| role.is_none_or(|r| r.pages.iter().any(|x| x == p));
    let full = role.is_none();
    let some = |on: bool| move |v: Vec<serde_json::Value>| if on { v } else { Vec::new() };
    let quiet = lab
        .quiet
        .as_ref()
        .map(|q| json!({ "window": q.window, "timezone": q.timezone, "active": q.active() }));
    Json(json!({
        "version": crate::VERSION,
        "auth": state.auth.is_some(),
        "user": caller.email,
        "role": caller.role.as_ref().map(|r| &r.name),
        "pages": role.map(|r| &r.pages),
        "refresh_seconds": state.cfg.refresh_seconds,
        "live_updates": state.cfg.live_updates,
        "quiet": quiet,
        "proxmox": some(page("infra"))(lab.proxmox.values().map(|p| json!({
            "name": p.name, "quiet": p.quiet, "ui_url": full.then_some(&p.ui_url), "insecure": p.insecure,
        })).collect()),
        "kubernetes": some(page("workloads") || page("gitops"))(lab.kubernetes.values().map(|k| json!({
            "name": k.name, "quiet": k.quiet, "headlamp_url": if full { k.headlamp_url.as_deref() } else { None },
            "forward_token": full && k.forwards_token(), "metrics": k.metrics.is_some(),
            "argocd_url": if full { k.argocd_url.as_deref() } else { None },
        })).collect()),
        "grafana": some(page("alerts"))(lab.grafana.values().map(|g| json!({
            "name": g.name, "quiet": g.quiet, "url": g.url, "insecure": g.insecure,
        })).collect()),
        "openbao": some(page("services"))(lab.openbao.values().map(|b| json!({
            "name": b.name, "quiet": b.quiet, "url": b.url, "insecure": b.insecure,
        })).collect()),
        "logs": some(page("logs"))(lab.logs.datasources.iter().map(|s| json!({
            "label": s.label, "type": s.kind, "pod_query": s.pod_query, "node_query": s.node_query,
        })).collect()),
        "security": if page("security") { lab.presets() } else { Vec::new() },
        "tailscale": page("tailscale") && lab.tailscale.is_some(),
        "github": lab.github.as_ref().filter(|_| page("workflows")).map(|g| json!({
            "watch": g.watch().len(), "helper": (full || g.session_mode()) && g.helper, "store": g.store.name(),
            "install_url": g.install_url,
        })),
        "healthchecks": lab.healthchecks.as_ref().filter(|_| page("outage")).map(|h| json!({ "url": h.url })),
        // hostnames and addresses: never for a role (RFC 0018)
        "certificates": full && lab.certificates_on(),
    }))
    .into_response()
}

pub async fn query(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((kind, name)): Path<(String, String)>,
) -> Response {
    if let Some(r) = &caller.role
        && !crate::roles::allows(&r.role, &kind)
    {
        return error(StatusCode::FORBIDDEN, "not available in this view");
    }
    match state.lab.query(&state.cache, &kind, &name, &caller).await {
        Some(envelope) => Json(envelope).into_response(),
        None => error(StatusCode::NOT_FOUND, format!("no query {kind}/{name}")),
    }
}

pub(super) fn log_action(
    action: &str,
    caller: &Caller,
    target: &str,
    result: &Result<String, crate::Error>,
) {
    let user = caller.email.as_deref().unwrap_or("local");
    match result {
        Ok(detail) => {
            tracing::info!(msg = "action", action, user, target, detail = %detail, result = "ok")
        }
        Err(e) => {
            tracing::warn!(msg = "action", action, user, target, error = %e, result = "error")
        }
    }
}

pub async fn pve_power(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
    Json(req): Json<PowerRequest>,
) -> Response {
    let Some(pve) = state.lab.proxmox.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such Proxmox");
    };
    let result = pve.power(&req).await;
    log_action(
        &format!("pve.guest.{}", req.action),
        &caller,
        &format!("{name}/{}/{}", req.node, req.vmid),
        &result,
    );
    state.cache.invalidate(&format!("pve:{name}"));
    match result {
        Ok(upid) => Json(json!({ "upid": upid })).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct TaskQuery {
    node: String,
    upid: String,
}

pub async fn pve_task(
    State(state): State<Shared>,
    Path(name): Path<String>,
    Query(q): Query<TaskQuery>,
) -> Response {
    let Some(pve) = state.lab.proxmox.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such Proxmox");
    };
    match pve.task(&q.node, &q.upid).await {
        Ok(t) => {
            state.cache.invalidate(&format!("pve:{name}"));
            Json(t).into_response()
        }
        Err(e) => error(StatusCode::BAD_GATEWAY, e),
    }
}

pub async fn flux_reconcile(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
    Json(req): Json<flux::ReconcileRequest>,
) -> Response {
    let Some(kube) = state.lab.kubernetes.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such cluster");
    };
    let result = match kube.client(caller.id_token.as_deref()) {
        Ok(client) => flux::reconcile(client, &req).await,
        Err(e) => Err(e),
    };
    log_action(
        "flux.reconcile",
        &caller,
        &format!("{name}/{}/{}/{}", req.kind, req.namespace, req.name),
        &result,
    );
    state.cache.invalidate(&format!("flux:{name}"));
    match result {
        Ok(done) => Json(json!({ "reconciled": done })).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn argocd_refresh(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
    Json(req): Json<argocd::RefreshRequest>,
) -> Response {
    let Some(kube) = state.lab.kubernetes.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such cluster");
    };
    let result = match kube.client(caller.id_token.as_deref()) {
        Ok(client) => argocd::refresh(client, &req).await,
        Err(e) => Err(e),
    };
    log_action(
        "argocd.refresh",
        &caller,
        &format!("{name}/{}/{}", req.namespace, req.name),
        &result,
    );
    state.cache.invalidate(&format!("argocd:{name}"));
    match result {
        Ok(done) => Json(json!({ "refreshed": done })).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

/// Log search. A POST for the body; it reads only, but every query is logged.
pub async fn search_logs(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<logs::SearchRequest>,
) -> Response {
    let started = std::time::Instant::now();
    let result = state.lab.search_logs(&req).await;
    let user = caller.email.as_deref().unwrap_or("local");
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(r) => {
            tracing::info!(msg = "logs", user, source = %req.source, minutes = req.minutes, query = %req.query, ms, result = "ok");
            Json(r).into_response()
        }
        Err(e) => {
            tracing::warn!(msg = "logs", user, source = %req.source, query = %req.query, ms, error = %e, result = "error");
            error(StatusCode::BAD_REQUEST, e)
        }
    }
}

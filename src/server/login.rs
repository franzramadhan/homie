//! Login routes: which methods exist, password login, OIDC redirect and
//! callback, logout.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{Shared, history::Entry};
use crate::integrations::Caller;

const SESSION_SECS: u64 = 12 * 3600;

/// Public: the login page asks which methods to offer.
pub async fn options(State(state): State<Shared>) -> Response {
    let (oidc, local, demo) = match &state.auth {
        Some(a) => (a.oidc.is_some(), a.local.is_some(), a.demo.is_some()),
        None => (false, false, false),
    };
    Json(json!({ "oidc": oidc, "local": local, "demo": demo })).into_response()
}

/// The passwordless demo session (RFC 0016). Same write checks as the API.
pub async fn demo(State(state): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(denied) = super::guard::write_denied(&state, &headers) {
        return denied;
    }
    let Some(auth) = &state.auth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match auth.demo_login() {
        Ok(sid) => {
            state.record(Entry::new(
                Some("demo"),
                Some(&sid),
                "login",
                "demo",
                "ok",
                None,
            ));
            (
                StatusCode::NO_CONTENT,
                [(
                    header::SET_COOKIE,
                    auth.cookie(&sid, crate::auth::DEMO_TTL.as_secs()),
                )],
            )
                .into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Starts the OIDC flow, or shows the login page when OIDC is not configured.
pub async fn login(State(state): State<Shared>) -> Response {
    match state.auth.as_ref().and_then(|a| a.oidc.as_ref()) {
        Some(oidc) => Redirect::to(&oidc.login_url()).into_response(),
        None => Redirect::to("/login").into_response(),
    }
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn failed(msg: &str) -> Response {
    let body = format!("Login failed: {msg}\n\nTry again: /login\n");
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

pub async fn callback(State(state): State<Shared>, Query(q): Query<Callback>) -> Response {
    let Some(auth) = &state.auth else {
        return Redirect::to("/").into_response();
    };
    if let Some(e) = q.error {
        return failed(&format!("{e} {}", q.error_description.unwrap_or_default()));
    }
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return failed("missing code or state");
    };
    match auth.oidc_callback(&code, &st).await {
        Ok(sid) => {
            let user = auth.name(&sid);
            state.record(Entry::new(
                user.as_deref(),
                Some(&sid),
                "login",
                "oidc",
                "ok",
                None,
            ));
            (
                [(header::SET_COOKIE, auth.cookie(&sid, SESSION_SECS))],
                Redirect::to("/"),
            )
                .into_response()
        }
        Err(e) => {
            // the provider named no one yet, or the error says who
            if auth.oidc.is_some() {
                state.record(Entry::new(
                    None,
                    None,
                    "login",
                    "oidc",
                    "denied",
                    Some(e.to_string()),
                ));
            }
            failed(&e.to_string())
        }
    }
}

#[derive(Deserialize)]
pub struct Credentials {
    username: String,
    password: String,
}

/// Password login. Same write checks as the API (X-Homie header, Origin).
pub async fn local(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(c): Json<Credentials>,
) -> Response {
    if let Some(denied) = super::guard::write_denied(&state, &headers) {
        return denied;
    }
    let Some(auth) = &state.auth else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "login is not configured"})),
        )
            .into_response();
    };
    match auth.local_login(&c.username, &c.password).await {
        Ok(sid) => {
            state.record(Entry::new(
                Some(&c.username),
                Some(&sid),
                "login",
                "local",
                "ok",
                None,
            ));
            (
                StatusCode::NO_CONTENT,
                [(header::SET_COOKIE, auth.cookie(&sid, SESSION_SECS))],
            )
                .into_response()
        }
        // a wrong password, or the lockout's "too many failed logins"
        Err(e) => {
            state.record(Entry::new(
                Some(&c.username),
                None,
                "login",
                "local",
                "denied",
                Some(e.to_string()),
            ));
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

pub async fn logout(
    State(state): State<Shared>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
) -> Response {
    let Some(auth) = &state.auth else {
        return StatusCode::NO_CONTENT.into_response();
    };
    if let Some(sid) = super::guard::cookie(&headers, auth.cookie_name()) {
        let user = caller.email.as_deref().unwrap_or("local");
        tracing::info!(msg = "logout", user, result = "ok");
        state.record(Entry::by(&caller, "logout", "", "ok", None));
        auth.logout(sid);
        // the session's GitHub token and its own watches go with it
        if let Some(gh) = &state.lab.github {
            gh.forget(sid);
        }
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, auth.cookie("", 0))],
    )
        .into_response()
}

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

use super::Shared;

const SESSION_SECS: u64 = 12 * 3600;

/// Public: the login page asks which methods to offer.
pub async fn options(State(state): State<Shared>) -> Response {
    let (oidc, local) = match &state.auth {
        Some(a) => (a.oidc.is_some(), a.local.is_some()),
        None => (false, false),
    };
    Json(json!({ "oidc": oidc, "local": local })).into_response()
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
        Ok(sid) => (
            [(header::SET_COOKIE, auth.cookie(&sid, SESSION_SECS))],
            Redirect::to("/"),
        )
            .into_response(),
        Err(e) => failed(&e.to_string()),
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
        Ok(sid) => (
            StatusCode::NO_CONTENT,
            [(header::SET_COOKIE, auth.cookie(&sid, SESSION_SECS))],
        )
            .into_response(),
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn logout(State(state): State<Shared>, headers: HeaderMap) -> Response {
    let Some(auth) = &state.auth else {
        return StatusCode::NO_CONTENT.into_response();
    };
    if let Some(sid) = super::guard::cookie(&headers, auth.cookie_name()) {
        auth.logout(sid);
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, auth.cookie("", 0))],
    )
        .into_response()
}

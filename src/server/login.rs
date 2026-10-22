//! Login, OIDC callback and logout routes.

use axum::{
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use super::Shared;
use crate::auth::COOKIE;

pub async fn login(State(state): State<Shared>) -> Response {
    match &state.auth {
        Some(auth) => Redirect::to(&auth.login_url()).into_response(),
        None => Redirect::to("/").into_response(),
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
    let body = format!("Login failed: {msg}\n\nTry again: /auth/login\n");
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
    match auth.callback(&code, &st).await {
        Ok((sid, _)) => {
            let cookie =
                format!("{COOKIE}={sid}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=43200");
            ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response()
        }
        Err(e) => failed(&e.to_string()),
    }
}

pub async fn logout(State(state): State<Shared>, headers: axum::http::HeaderMap) -> Response {
    if let (Some(auth), Some(sid)) = (&state.auth, super::guard::cookie(&headers, COOKIE)) {
        auth.logout(sid);
    }
    let clear = format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, clear)]).into_response()
}

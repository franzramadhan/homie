//! The GitHub picker helper (RFC 0014): sign in with the GitHub App's user
//! authorization, list repositories and workflows, copy `[[github.watch]]`
//! lines, or save them (RFC 0015). Runs are read with the app's installation
//! token, never with these user tokens. Tokens stay on the server, keyed by
//! the Homie user.

use std::time::{Duration, Instant};

use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{Shared, api::log_action};
use crate::integrations::{Caller, github::valid_repo};

const STATE_TTL: Duration = Duration::from_secs(600);

fn user_key(c: &Caller) -> String {
    c.email.clone().unwrap_or_else(|| "local".into())
}

fn redirect_uri(state: &Shared) -> String {
    let base = if state.cfg.public_url.is_empty() {
        format!("http://{}", state.cfg.listen)
    } else {
        state.cfg.public_url.clone()
    };
    format!("{}/github/callback", base.trim_end_matches('/'))
}

fn error(status: StatusCode, msg: impl ToString) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

pub async fn login(State(state): State<Shared>, Extension(caller): Extension<Caller>) -> Response {
    let Some(gh) = state.lab.github.as_ref().filter(|g| g.helper) else {
        return error(
            StatusCode::NOT_FOUND,
            "the GitHub sign-in helper is not configured",
        );
    };
    let nonce = openidconnect::CsrfToken::new_random().secret().clone();
    {
        let mut states = state.github_states.lock().unwrap();
        states.retain(|_, (_, at)| at.elapsed() < STATE_TTL);
        states.insert(nonce.clone(), (user_key(&caller), Instant::now()));
    }
    match gh.authorize_url(&redirect_uri(&state), &nonce) {
        Some(url) => Redirect::to(&url).into_response(),
        None => error(
            StatusCode::NOT_FOUND,
            "the GitHub sign-in helper is not configured",
        ),
    }
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
}

pub async fn callback(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<Callback>,
) -> Response {
    let Some(gh) = state.lab.github.as_ref() else {
        return Redirect::to("/").into_response();
    };
    let (Some(code), Some(nonce)) = (q.code, q.state) else {
        return error(StatusCode::BAD_REQUEST, "missing code or state");
    };
    // the state must have been issued to this same Homie user, recently
    let owner = state
        .github_states
        .lock()
        .unwrap()
        .remove(&nonce)
        .filter(|(_, at)| at.elapsed() < STATE_TTL);
    if owner.map(|(u, _)| u) != Some(user_key(&caller)) {
        return error(
            StatusCode::FORBIDDEN,
            "unknown or expired GitHub sign-in, start again",
        );
    }
    match gh.exchange(&code, &redirect_uri(&state)).await {
        Ok(token) => {
            state
                .github_users
                .lock()
                .unwrap()
                .insert(user_key(&caller), token);
            Redirect::to("/workflows?picker=1").into_response()
        }
        Err(e) => error(StatusCode::FORBIDDEN, e),
    }
}

fn user_token(state: &Shared, caller: &Caller) -> Option<String> {
    state
        .github_users
        .lock()
        .unwrap()
        .get(&user_key(caller))
        .cloned()
}

pub async fn repos(State(state): State<Shared>, Extension(caller): Extension<Caller>) -> Response {
    let Some(gh) = state.lab.github.as_ref() else {
        return error(StatusCode::NOT_FOUND, "github is not configured");
    };
    let Some(token) = user_token(&state, &caller) else {
        return error(StatusCode::PRECONDITION_REQUIRED, "connect GitHub first");
    };
    match gh.user_repos(&token).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => error(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
pub struct RepoQuery {
    repo: String,
}

pub async fn workflows(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<RepoQuery>,
) -> Response {
    let Some(gh) = state.lab.github.as_ref() else {
        return error(StatusCode::NOT_FOUND, "github is not configured");
    };
    let Some(token) = user_token(&state, &caller) else {
        return error(StatusCode::PRECONDITION_REQUIRED, "connect GitHub first");
    };
    match gh.user_workflows(&token, &q.repo).await {
        Ok(w) => Json(w).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct SaveWatch {
    repo: String,
    workflows: Vec<String>,
}

/// Saves the picked workflows of one repository (RFC 0015). The caller's
/// GitHub sign-in must see each workflow, so nobody adds a repository they
/// cannot read themselves through the app's shared token.
pub async fn save_watch(
    State(state): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<SaveWatch>,
) -> Response {
    let Some(gh) = state.lab.github.as_ref() else {
        return error(StatusCode::NOT_FOUND, "github is not configured");
    };
    if gh.store.name().is_none() {
        return error(StatusCode::CONFLICT, "saving watches is turned off");
    }
    if !valid_repo(&body.repo) {
        return error(StatusCode::BAD_REQUEST, "repository must be owner/name");
    }
    let Some(token) = user_token(&state, &caller) else {
        return error(StatusCode::PRECONDITION_REQUIRED, "connect GitHub first");
    };
    let available = match gh.user_workflows(&token, &body.repo).await {
        Ok(w) => w,
        Err(e) => return error(StatusCode::FORBIDDEN, e),
    };
    if let Some(f) = body
        .workflows
        .iter()
        .find(|f| !available.iter().any(|w| w["file"] == f.as_str()))
    {
        return error(
            StatusCode::BAD_REQUEST,
            format!("no workflow {f:?} in {}", body.repo),
        );
    }
    let result = gh.save_repo(&body.repo, &body.workflows).await;
    let logged = result
        .as_ref()
        .map(|(a, r)| format!("added {a:?} removed {r:?}"))
        .map_err(|e| e.to_string().into());
    log_action("github.watch", &caller, &body.repo, &logged);
    match result {
        Ok(_) => {
            state.cache.invalidate("github:");
            Json(json!({ "watch": gh.watch().len() })).into_response()
        }
        Err(e) => error(StatusCode::BAD_GATEWAY, e),
    }
}

//! Request guards: Host check (DNS rebinding) and security headers on every
//! response; session, CSRF and Origin checks on the API.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::Shared;
use crate::{auth::COOKIE, integrations::Caller};

fn deny(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn host_only(host: &str) -> &str {
    // "[::1]:7777" -> "[::1]", "localhost:7777" -> "localhost"
    match host.rfind(':') {
        Some(i) if !host[i..].contains(']') => &host[..i],
        _ => host,
    }
}

/// Without login, only loopback names and the public_url host may reach
/// Homie, so a page on another site cannot rebind its DNS name to 127.0.0.1
/// and read the API.
fn host_allowed(state: &Shared, host: &str) -> bool {
    let host = host_only(host);
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    match url::Url::parse(&state.cfg.public_url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
    {
        Some(public) => host == public || host == format!("[{public}]"),
        // allow_unauthenticated behind a proxy, with no public_url to compare against
        None => state.cfg.allow_unauthenticated,
    }
}

pub async fn host(State(state): State<Shared>, req: Request, next: Next) -> Response {
    if state.auth.is_none() {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        if !host_allowed(&state, host) {
            return deny(StatusCode::MISDIRECTED_REQUEST, "unexpected Host header");
        }
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    for (k, v) in [
        ("x-frame-options", "DENY"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'",
        ),
    ] {
        h.insert(k, HeaderValue::from_static(v));
    }
    resp
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(name)?.strip_prefix('='))
}

/// Writes need the custom header (a cross-site form cannot set it) and an
/// Origin from this site. Reads and writes need a session when login is on.
pub async fn session(State(state): State<Shared>, mut req: Request, next: Next) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        if req.headers().get("x-homie").and_then(|v| v.to_str().ok()) != Some("1") {
            return deny(StatusCode::FORBIDDEN, "missing X-Homie header");
        }
        if let Some(origin) = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
        {
            let ok = match &state.auth {
                Some(a) => origin == a.public_url.origin().ascii_serialization(),
                None => url::Url::parse(origin).is_ok_and(|u| {
                    let host = match u.port() {
                        Some(p) => format!("{}:{p}", u.host_str().unwrap_or_default()),
                        None => u.host_str().unwrap_or_default().to_string(),
                    };
                    host_allowed(&state, &host)
                }),
            };
            if !ok {
                return deny(StatusCode::FORBIDDEN, "cross-origin request");
            }
        }
    }

    let caller = match &state.auth {
        None => Caller::default(),
        Some(auth) => {
            let user = match cookie(req.headers(), COOKIE) {
                Some(sid) => auth.user(sid).await,
                None => None,
            };
            match user {
                Some(u) => Caller {
                    email: Some(u.email),
                    id_token: Some(u.id_token),
                },
                None => return deny(StatusCode::UNAUTHORIZED, "login required"),
            }
        }
    };
    req.extensions_mut().insert(caller);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_cookies() {
        assert_eq!(host_only("localhost:7777"), "localhost");
        assert_eq!(host_only("[::1]:7777"), "[::1]");
        assert_eq!(host_only("[::1]"), "[::1]");
        assert_eq!(host_only("homie.example"), "homie.example");

        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; __Host-homie=abc; b=2"),
        );
        assert_eq!(cookie(&h, COOKIE), Some("abc"));
        assert_eq!(cookie(&h, "missing"), None);
    }
}

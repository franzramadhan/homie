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
use crate::{
    config::Role,
    integrations::{Caller, RoleRef},
};

/// Routes a role may use: reads only (RFC 0016). Whether a query's page is
/// allowed is checked per query. With per-session GitHub watches (RFC 0017)
/// a role with the workflows page may also sign in to GitHub and pick: there
/// it only ever touches the visitor's own data.
fn role_route(role: &Role, method: &Method, path: &str, github_session: bool) -> bool {
    let page = |p: &str| role.pages.iter().any(|x| x == p);
    let picker = github_session && page("workflows");
    match *method {
        Method::GET | Method::HEAD => {
            path == "/api/meta"
                || path == "/api/stream"
                || path.starts_with("/api/q/")
                || (picker
                    && matches!(
                        path,
                        "/github/login"
                            | "/github/callback"
                            | "/api/github/repos"
                            | "/api/github/workflows"
                    ))
        }
        Method::POST => {
            path == "/auth/logout"
                || (path == "/api/logs" && page("logs"))
                || (picker && path == "/api/github/watch")
        }
        _ => false,
    }
}

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
fn host_allowed(cfg: &crate::config::Config, host: &str) -> bool {
    let host = host_only(host);
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    match url::Url::parse(&cfg.public_url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
    {
        Some(public) => host == public || host == format!("[{public}]"),
        // allow_unauthenticated behind a proxy, with no public_url to compare against
        None => cfg.allow_unauthenticated,
    }
}

pub async fn host(State(state): State<Shared>, req: Request, next: Next) -> Response {
    if state.auth.is_none() {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        if !host_allowed(&state.cfg, host) {
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

/// A write needs the custom header (a cross-site form cannot set it) and,
/// when the browser sends one, an Origin from this site.
/// `Some(response)` when the write must be refused.
pub fn write_denied(state: &Shared, headers: &HeaderMap) -> Option<Response> {
    if headers.get("x-homie").and_then(|v| v.to_str().ok()) != Some("1") {
        return Some(deny(StatusCode::FORBIDDEN, "missing X-Homie header"));
    }
    if cross_origin(state, headers) {
        return Some(deny(StatusCode::FORBIDDEN, "cross-origin request"));
    }
    None
}

/// An Origin header from another site (none sent counts as same site).
pub fn cross_origin(state: &Shared, headers: &HeaderMap) -> bool {
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        let ok = match &state.auth {
            Some(a) => origin == a.public_url.origin().ascii_serialization(),
            None => url::Url::parse(origin).is_ok_and(|u| {
                // the public_url host: scheme and port must match too, so
                // another service on that host is another site
                if let Ok(p) = url::Url::parse(&state.cfg.public_url)
                    && p.host_str().is_some()
                    && u.host_str() == p.host_str()
                {
                    return u.origin() == p.origin();
                }
                let host = match u.port() {
                    Some(p) => format!("{}:{p}", u.host_str().unwrap_or_default()),
                    None => u.host_str().unwrap_or_default().to_string(),
                };
                host_allowed(&state.cfg, &host)
            }),
        };
        return !ok;
    }
    false
}

/// Writes pass `write_denied`. Reads and writes need a session when a login
/// method is configured.
pub async fn session(State(state): State<Shared>, mut req: Request, next: Next) -> Response {
    if req.method() != Method::GET
        && req.method() != Method::HEAD
        && let Some(denied) = write_denied(&state, req.headers())
    {
        return denied;
    }
    let caller = match &state.auth {
        None => Caller::default(),
        Some(auth) => {
            let user = match cookie(req.headers(), auth.cookie_name()) {
                Some(sid) => auth.user(sid).await,
                None => None,
            };
            match user {
                Some(u) => {
                    let role = match u.role {
                        // a role that left the config: fail closed, never full access
                        Some(name) => match state.lab.roles.get(&name) {
                            Some(r) => Some(RoleRef {
                                name,
                                role: r.clone(),
                            }),
                            None => return deny(StatusCode::FORBIDDEN, "unknown role"),
                        },
                        None => None,
                    };
                    Caller {
                        email: Some(u.name),
                        id_token: u.id_token,
                        role,
                        sid: cookie(req.headers(), auth.cookie_name()).map(String::from),
                        service_account: None,
                    }
                }
                None => return deny(StatusCode::UNAUTHORIZED, "login required"),
            }
        }
    };
    let github_session = state.lab.github.as_ref().is_some_and(|g| g.session_mode());
    if let Some(r) = &caller.role
        && !role_route(&r.role, req.method(), req.uri().path(), github_session)
    {
        tracing::info!(msg = "denied", role = %r.name, route = %req.uri().path());
        return deny(StatusCode::FORBIDDEN, "not available in this view");
    }
    req.extensions_mut().insert(caller);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_routes() {
        let r = Role {
            pages: vec!["workloads".into()],
            namespaces: vec!["apps".into()],
            proxmox_tags: vec![],
            kubernetes_service_account: None,
        };
        assert!(role_route(&r, &Method::GET, "/api/q/k8s/main", false));
        assert!(role_route(&r, &Method::GET, "/api/stream", false));
        assert!(role_route(&r, &Method::POST, "/auth/logout", false));
        for (m, p) in [
            (Method::POST, "/api/act/pve/lab/power"),
            (Method::GET, "/api/act/pve/lab/task"),
            (Method::POST, "/api/logs"),
            (Method::POST, "/api/github/watch"),
            (Method::GET, "/api/github/repos"),
            (Method::GET, "/github/login"),
        ] {
            assert!(!role_route(&r, &m, p, false), "{m} {p}");
        }
        // RFC 0017: the picker opens for a role only with session watches
        // and the workflows page
        let ci = Role {
            pages: vec!["workflows".into()],
            ..r.clone()
        };
        for (m, p) in [
            (Method::GET, "/github/login"),
            (Method::GET, "/github/callback"),
            (Method::GET, "/api/github/repos"),
            (Method::GET, "/api/github/workflows"),
            (Method::POST, "/api/github/watch"),
        ] {
            assert!(role_route(&ci, &m, p, true), "{m} {p}");
            assert!(
                !role_route(&ci, &m, p, false),
                "{m} {p} without session store"
            );
            assert!(
                !role_route(&r, &m, p, true),
                "{m} {p} without the workflows page"
            );
        }
        assert!(!role_route(
            &ci,
            &Method::POST,
            "/api/act/pve/lab/power",
            true
        ));
    }

    /// Keeps `role_route` equal to its Bend model, spec/roles.bend, whose laws
    /// (LAWS.bend) are proven by `bend PROOF.bend`: every route, method, role
    /// and store, against a direct transcription of the model.
    #[test]
    fn role_routes_match_spec() {
        // (path, the model's Route) for each Route
        let routes = [
            ("/api/meta", "Meta"),
            ("/api/stream", "Stream"),
            ("/api/q/k8s/main", "Query"),
            ("/auth/logout", "Logout"),
            ("/api/logs", "Logs"),
            ("/github/login", "GhLogin"),
            ("/github/callback", "GhCallback"),
            ("/api/github/repos", "GhRepos"),
            ("/api/github/workflows", "GhWorkflows"),
            ("/api/github/watch", "GhWatch"),
            ("/api/act/pve/lab/power", "Act"),
            ("/api/act/pve/lab/console", "Act"),
            ("/api/act/pve/lab/console/id", "Act"),
            ("/admin", "Unknown"),
        ];
        let methods = [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::DELETE,
        ];
        let model = |m: &Method, route: &str, logs: bool, workflows: bool, ghs: bool| {
            let read = *m == Method::GET || *m == Method::HEAD;
            let post = *m == Method::POST;
            let picker = ghs && workflows;
            match route {
                "Meta" | "Stream" | "Query" => read,
                "Logout" => post,
                "Logs" => post && logs,
                "GhLogin" | "GhCallback" | "GhRepos" | "GhWorkflows" => read && picker,
                "GhWatch" => post && picker,
                _ => false,
            }
        };
        for (path, route) in routes {
            for m in &methods {
                for logs in [false, true] {
                    for workflows in [false, true] {
                        for ghs in [false, true] {
                            let mut pages = vec!["infra".to_string()];
                            if logs {
                                pages.push("logs".into());
                            }
                            if workflows {
                                pages.push("workflows".into());
                            }
                            let role = Role {
                                pages,
                                namespaces: vec![],
                                proxmox_tags: vec![],
                                kubernetes_service_account: None,
                            };
                            assert_eq!(
                                role_route(&role, m, path, ghs),
                                model(m, route, logs, workflows, ghs),
                                "{m} {path} logs={logs} workflows={workflows} ghs={ghs}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Keeps `host_allowed` equal to `host_allowed` in spec/guard.bend, whose
    /// laws (LAWS.bend) are proven by `bend PROOF.bend`.
    #[test]
    fn hosts_match_spec() {
        // (Host header, the model's Host)
        let hosts = [
            ("localhost:7777", "Loopback"),
            ("127.0.0.1", "Loopback"),
            ("[::1]:8080", "Loopback"),
            ("homie.example", "Public"),
            ("homie.example:443", "Public"),
            ("evil.example", "Elsewhere"),
            ("homie.example.evil.example", "Elsewhere"),
            ("", "Elsewhere"),
        ];
        let model = |h: &str, public_url: bool, allow: bool| match h {
            "Loopback" => true,
            "Public" => public_url || allow,
            _ => !public_url && allow,
        };
        for (host, class) in hosts {
            for public_url in [false, true] {
                for allow in [false, true] {
                    let cfg = crate::config::Config {
                        public_url: if public_url {
                            "http://homie.example".into()
                        } else {
                            String::new()
                        },
                        allow_unauthenticated: allow,
                        ..Default::default()
                    };
                    assert_eq!(
                        host_allowed(&cfg, host),
                        model(class, public_url, allow),
                        "{host} public_url={public_url} allow={allow}"
                    );
                }
            }
        }
    }

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
        assert_eq!(cookie(&h, "__Host-homie"), Some("abc"));
        assert_eq!(cookie(&h, "missing"), None);
    }
}

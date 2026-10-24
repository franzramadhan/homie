//! HTTP clients for the integrations, with the per-integration TLS options:
//! custom CA, verification off, plain http, and `tls_server_name` (connect to
//! one address, verify the certificate against another name).

use std::{net::SocketAddr, sync::Arc, time::Duration};

use reqwest::{Client, Response, StatusCode};
use url::Url;

use crate::{Result, config::Tls};

/// A client and the base URL to use with it. With `tls_server_name`, the URL
/// host becomes that name and the client resolves it to the configured address.
pub struct Target {
    pub client: Client,
    pub base: Url,
    pub insecure: Option<&'static str>,
    /// for connections outside reqwest (Proxmox console WebSockets): the
    /// address `tls_server_name` resolves to, and the TLS config (none for http)
    pub addr: Option<SocketAddr>,
    pub tls: Option<Arc<rustls::ClientConfig>>,
}

/// reqwest is built without a crypto provider so it shares rustls' ring with
/// kube. Installing it twice is a no-op.
pub fn init_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub async fn target(url: &str, tls: &Tls) -> Result<Target> {
    init_tls();
    let mut base = Url::parse(url)?;
    let mut builder = Client::builder()
        .user_agent(concat!("homie/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        // a backend cannot bounce Homie (and its token) to another address
        .redirect(reqwest::redirect::Policy::none());

    // Built-in Mozilla roots, so a scratch image without /etc/ssl works.
    let roots = crate::tls::roots(tls.ca_file.as_deref())?;

    let mut resolved = None;
    if let Some(name) = &tls.tls_server_name {
        let host = base.host_str().ok_or("URL has no host")?.to_string();
        let port = base.port_or_known_default().ok_or("URL has no port")?;
        let addr: SocketAddr = match host.trim_matches(['[', ']']).parse() {
            Ok(ip) => SocketAddr::new(ip, port),
            Err(_) => tokio::net::lookup_host((host.as_str(), port))
                .await?
                .next()
                .ok_or_else(|| format!("{host} does not resolve"))?,
        };
        builder = builder.resolve(name, addr);
        resolved = Some(addr);
        base.set_host(Some(name))?;
    }
    // the certificate of every handshake is recorded for RFC 0018
    let key = format!(
        "{}:{}",
        base.host_str().unwrap_or_default(),
        base.port_or_known_default().unwrap_or(443)
    );
    let config = crate::tls::http_config(key, roots, tls.tls_verify)?;
    builder = builder.tls_backend_preconfigured(config.clone());

    let insecure = if base.scheme() == "http" {
        Some("plain_http")
    } else if !tls.tls_verify {
        Some("tls_verify_off")
    } else {
        None
    };
    let tls = (base.scheme() == "https").then(|| Arc::new(config));
    Ok(Target {
        client: builder.build()?,
        base,
        insecure,
        addr: resolved,
        tls,
    })
}

/// `base` joined with an absolute API path, keeping any path prefix in
/// `base`. A query string in `path` stays a query string.
pub fn join(base: &Url, path: &str) -> Url {
    let mut u = base.clone();
    let prefix = u.path().trim_end_matches('/').to_string();
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    u.set_path(&format!("{prefix}{path}"));
    u.set_query(query);
    u
}

/// Turns a non-2xx response into an error with the status and a short body.
pub async fn ok(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.chars().take(200).collect();
    Err(HttpError {
        status,
        body: body.trim().to_string(),
    }
    .into())
}

#[derive(Debug)]
pub struct HttpError {
    pub status: StatusCode,
    pub body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.body.is_empty() {
            write!(f, "HTTP {}", self.status)
        } else {
            write!(f, "HTTP {}: {}", self.status, self.body)
        }
    }
}

impl std::error::Error for HttpError {}

pub fn is_unauthorized(e: &crate::Error) -> bool {
    e.downcast_ref::<HttpError>()
        .is_some_and(|h| h.status == StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_keeps_prefix() {
        let b = Url::parse("https://grafana.example/sub/?x=1").unwrap();
        assert_eq!(
            join(&b, "/api/health").as_str(),
            "https://grafana.example/sub/api/health"
        );
        let b = Url::parse("https://pve:8006").unwrap();
        assert_eq!(
            join(&b, "/api2/json/version").as_str(),
            "https://pve:8006/api2/json/version"
        );
        assert_eq!(
            join(&b, "/api2/json/cluster/resources?type=vm").as_str(),
            "https://pve:8006/api2/json/cluster/resources?type=vm"
        );
    }

    #[tokio::test]
    async fn server_name_override() {
        let tls = Tls {
            tls_verify: true,
            ca_file: None,
            tls_server_name: Some("pve-01.example.ts.net".into()),
        };
        let t = target("https://192.0.2.10:8006", &tls).await.unwrap();
        assert_eq!(t.base.as_str(), "https://pve-01.example.ts.net:8006/");
        assert_eq!(t.insecure, None);
        let tls = Tls {
            tls_verify: false,
            ca_file: None,
            tls_server_name: None,
        };
        assert_eq!(
            target("https://x", &tls).await.unwrap().insecure,
            Some("tls_verify_off")
        );
        assert_eq!(
            target("http://x", &tls).await.unwrap().insecure,
            Some("plain_http")
        );
    }

    fn tls(ca_file: Option<&str>, server_name: Option<&str>) -> Tls {
        Tls {
            tls_verify: true,
            ca_file: ca_file.map(Into::into),
            tls_server_name: server_name.map(Into::into),
        }
    }

    #[tokio::test]
    async fn target_errors_and_lookups() {
        let e = target("https://x", &tls(Some("/nonexistent/ca.pem"), None))
            .await
            .err()
            .unwrap();
        assert!(
            e.to_string().starts_with("reading /nonexistent/ca.pem: "),
            "{e}"
        );
        // a bundle without certificates adds no roots
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test-rsa.pem");
        assert!(target("https://x", &tls(Some(fixture), None)).await.is_ok());

        let t = target(
            "http://localhost:3000/sub",
            &tls(None, Some("grafana.example")),
        )
        .await
        .unwrap();
        assert_eq!(t.base.as_str(), "http://grafana.example:3000/sub");
        assert_eq!(t.insecure, Some("plain_http"));
        let e = target("https://homie.invalid", &tls(None, Some("x.example"))).await;
        assert!(e.is_err());
        let e = target("data:text/plain,x", &tls(None, Some("x.example")))
            .await
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "URL has no host");
        assert!(target("not a url", &tls(None, None)).await.is_err());
    }

    #[tokio::test]
    async fn status_errors() {
        use axum::{Router, http::StatusCode, routing::get};
        let long = "x".repeat(300);
        let url = crate::testkit::serve(
            Router::new()
                .route("/ok", get(|| async { "fine" }))
                .route(
                    "/denied",
                    get(|| async { (StatusCode::UNAUTHORIZED, " no token \n") }),
                )
                .route("/empty", get(|| async { StatusCode::BAD_GATEWAY }))
                .route(
                    "/long",
                    get(move || async move { (StatusCode::INTERNAL_SERVER_ERROR, long) }),
                ),
        )
        .await;
        let t = target(&url, &tls(None, None)).await.unwrap();
        let fetch = |path: &str| t.client.get(join(&t.base, path)).send();
        assert_eq!(
            ok(fetch("/ok").await.unwrap())
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "fine"
        );

        let e = ok(fetch("/denied").await.unwrap()).await.unwrap_err();
        assert_eq!(e.to_string(), "HTTP 401 Unauthorized: no token");
        assert!(is_unauthorized(&e));
        let e = ok(fetch("/empty").await.unwrap()).await.unwrap_err();
        assert_eq!(e.to_string(), "HTTP 502 Bad Gateway");
        assert!(!is_unauthorized(&e));
        assert!(!is_unauthorized(&"plain".into()));
        let e = ok(fetch("/long").await.unwrap()).await.unwrap_err();
        let h = e.downcast_ref::<HttpError>().unwrap();
        assert_eq!(h.body.len(), 200);
    }
}

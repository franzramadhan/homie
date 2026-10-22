//! HTTP clients for the integrations, with the per-integration TLS options:
//! custom CA, verification off, plain http, and `tls_server_name` (connect to
//! one address, verify the certificate against another name).

use std::{net::SocketAddr, time::Duration};

use reqwest::{Certificate, Client, Response, StatusCode};
use url::Url;

use crate::{Result, config::Tls};

/// A client and the base URL to use with it. With `tls_server_name`, the URL
/// host becomes that name and the client resolves it to the configured address.
pub struct Target {
    pub client: Client,
    pub base: Url,
    pub insecure: Option<&'static str>,
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
        .timeout(Duration::from_secs(15));

    // Built-in Mozilla roots, so a scratch image without /etc/ssl works.
    let mut roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|c| Certificate::from_der(c))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(ca) = &tls.ca_file {
        let path = crate::config::expand(ca);
        let pem = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        roots.extend(Certificate::from_pem_bundle(&pem)?);
    }
    builder = builder.tls_certs_only(roots);
    if !tls.tls_verify {
        builder = builder.danger_accept_invalid_certs(true);
    }

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
        base.set_host(Some(name))?;
    }

    let insecure = if base.scheme() == "http" {
        Some("plain_http")
    } else if !tls.tls_verify {
        Some("tls_verify_off")
    } else {
        None
    };
    Ok(Target {
        client: builder.build()?,
        base,
        insecure,
    })
}

/// `base` joined with an absolute API path, keeping any path prefix in `base`.
pub fn join(base: &Url, path: &str) -> Url {
    let mut u = base.clone();
    let prefix = u.path().trim_end_matches('/').to_string();
    u.set_path(&format!("{prefix}{path}"));
    u.set_query(None);
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
}

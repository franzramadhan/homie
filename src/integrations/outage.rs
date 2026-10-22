//! Off-site signals that still answer when the lab is down: Tailscale device
//! presence and healthchecks.io checks.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    Result, config,
    http::{self, Target},
    secret::SecretCell,
};

pub struct Tailscale {
    tailnet: String,
    target: Target,
    auth: TsAuth,
    hosts: Vec<String>,
    quiet_hosts: Vec<String>,
}

enum TsAuth {
    ApiKey(SecretCell),
    OAuth {
        id: SecretCell,
        secret: SecretCell,
        token: Mutex<Option<(String, Instant)>>,
    },
}

#[derive(Serialize)]
pub struct Device {
    name: String,
    hostname: String,
    online: bool,
    last_seen: String,
    os: String,
    addresses: Vec<String>,
    tags: Vec<String>,
    user: String,
    client_version: String,
    update_available: bool,
    /// node key expiry; None when expiry is disabled
    key_expiry: Option<String>,
    /// shared into this tailnet from another one
    external: bool,
    /// matches `hosts`: counted on the overview and in alerts
    watched: bool,
    /// matches `quiet_hosts` while quiet hours are on
    quiet: bool,
}

/// `*` matches any run of characters. Enough for host name patterns.
pub fn glob(pattern: &str, s: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == s,
        Some((head, rest)) => {
            let Some(tail) = s.strip_prefix(head) else {
                return false;
            };
            (0..=tail.len()).any(|i| tail.is_char_boundary(i) && glob(rest, &tail[i..]))
        }
    }
}

impl Device {
    pub fn is_watched(&self) -> bool {
        self.watched
    }
}

impl Tailscale {
    pub async fn new(cfg: &config::Tailscale) -> Result<Self> {
        let auth = match (&cfg.api_key, &cfg.oauth_client_id, &cfg.oauth_client_secret) {
            (Some(key), _, _) => TsAuth::ApiKey(SecretCell::new(key).await?),
            (None, Some(id), Some(secret)) => TsAuth::OAuth {
                id: SecretCell::new(id).await?,
                secret: SecretCell::new(secret).await?,
                token: Mutex::new(None),
            },
            _ => return Err("tailscale: no credentials".into()),
        };
        let tls = config::Tls {
            tls_verify: true,
            ca_file: None,
            tls_server_name: None,
        };
        Ok(Self {
            tailnet: cfg.tailnet.clone(),
            target: http::target("https://api.tailscale.com", &tls).await?,
            auth,
            hosts: cfg.hosts.clone(),
            quiet_hosts: cfg.quiet_hosts.clone(),
        })
    }

    async fn bearer(&self) -> Result<String> {
        match &self.auth {
            TsAuth::ApiKey(k) => Ok(k.get().await),
            TsAuth::OAuth { id, secret, token } => {
                let mut cached = token.lock().await;
                if let Some((t, until)) = cached.as_ref()
                    && Instant::now() < *until
                {
                    return Ok(t.clone());
                }
                let resp = self
                    .target
                    .client
                    .post(http::join(&self.target.base, "/api/v2/oauth/token"))
                    .form(&[
                        ("client_id", id.get().await),
                        ("client_secret", secret.get().await),
                        ("grant_type", "client_credentials".into()),
                    ])
                    .send()
                    .await?;
                let body: Value = http::ok(resp).await?.json().await?;
                let t = body["access_token"]
                    .as_str()
                    .ok_or("tailscale: no access_token")?
                    .to_string();
                let ttl = body["expires_in"]
                    .as_u64()
                    .unwrap_or(3600)
                    .saturating_sub(60);
                *cached = Some((t.clone(), Instant::now() + Duration::from_secs(ttl)));
                Ok(t)
            }
        }
    }

    pub async fn devices(&self, quiet_active: bool) -> Result<Vec<Device>> {
        let path = format!(
            "/api/v2/tailnet/{}/devices",
            url::form_urlencoded::byte_serialize(self.tailnet.as_bytes()).collect::<String>()
        );
        let resp = self
            .target
            .client
            .get(http::join(&self.target.base, &path))
            .query(&[("fields", "all")])
            .bearer_auth(self.bearer().await?)
            .send()
            .await?;
        let body: Value = http::ok(resp).await?.json().await?;
        let now = jiff::Timestamp::now();
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let mut devices: Vec<Device> = body["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| {
                // short name only: the tailnet domain stays out of the UI
                let name = d["name"].as_str()?.split('.').next()?.to_string();
                let last_seen = text(&d["lastSeen"]);
                let recent = last_seen
                    .parse::<jiff::Timestamp>()
                    .is_ok_and(|t| now.duration_since(t) < jiff::SignedDuration::from_mins(5));
                let strings = |v: &Value| -> Vec<String> {
                    v.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                };
                Some(Device {
                    online: d["connectedToControl"].as_bool().unwrap_or(recent),
                    watched: self.hosts.iter().any(|p| glob(p, &name)),
                    quiet: quiet_active && self.quiet_hosts.iter().any(|p| glob(p, &name)),
                    hostname: text(&d["hostname"]),
                    os: text(&d["os"]),
                    addresses: strings(&d["addresses"]),
                    tags: strings(&d["tags"]),
                    user: text(&d["user"]),
                    client_version: text(&d["clientVersion"])
                        .split('-')
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    update_available: d["updateAvailable"].as_bool().unwrap_or(false),
                    key_expiry: (!d["keyExpiryDisabled"].as_bool().unwrap_or(false))
                        .then(|| text(&d["expires"]))
                        .filter(|e| !e.is_empty() && !e.starts_with("0001")),
                    external: d["isExternal"].as_bool().unwrap_or(false),
                    name,
                    last_seen,
                })
            })
            .collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(devices)
    }
}

pub struct Healthchecks {
    pub quiet: bool,
    pub url: String,
    target: Target,
    key: SecretCell,
}

#[derive(Serialize)]
pub struct Check {
    name: String,
    status: String,
    last_ping: Option<String>,
    next_ping: Option<String>,
}

impl Healthchecks {
    pub async fn new(cfg: &config::Healthchecks) -> Result<Self> {
        let target = http::target(&cfg.url, &cfg.tls())
            .await
            .map_err(|e| format!("healthchecks: {e}"))?;
        Ok(Self {
            quiet: cfg.quiet,
            url: target.base.as_str().trim_end_matches('/').to_string(),
            target,
            key: SecretCell::new(&cfg.api_key)
                .await
                .map_err(|e| format!("healthchecks.api_key: {e}"))?,
        })
    }

    pub async fn checks(&self) -> Result<Vec<Check>> {
        let resp = self
            .target
            .client
            .get(http::join(&self.target.base, "/api/v3/checks/"))
            .header("X-Api-Key", self.key.get().await)
            .send()
            .await?;
        let body: Value = http::ok(resp).await?.json().await?;
        let text = |v: &Value| v.as_str().map(String::from);
        Ok(body["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| Check {
                name: text(&c["name"]).unwrap_or_default(),
                status: text(&c["status"]).unwrap_or_default(),
                last_ping: text(&c["last_ping"]),
                next_ping: text(&c["next_ping"]),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::serve;
    use axum::{
        Form, Json, Router,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    fn header(h: &HeaderMap, k: &str) -> String {
        h.get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// Answers the device list only for `Bearer tok`.
    fn devices_route(body: Value) -> Router {
        Router::new().route(
            "/api/v2/tailnet/-/devices",
            get(move |h: HeaderMap| async move {
                if header(&h, "authorization") != "Bearer tok" {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                Ok(Json(body))
            }),
        )
    }

    /// A client from `cfg`, pointed at the fake API instead of api.tailscale.com.
    async fn tailscale(cfg: Value, url: &str) -> Tailscale {
        let mut ts = Tailscale::new(&serde_json::from_value(cfg).unwrap())
            .await
            .unwrap();
        let tls = config::Tls {
            tls_verify: true,
            ca_file: None,
            tls_server_name: None,
        };
        ts.target = http::target(url, &tls).await.unwrap();
        ts
    }

    #[tokio::test]
    async fn no_credentials() {
        let cfg = serde_json::from_value(json!({"oauth_client_id": "id"})).unwrap();
        let err = Tailscale::new(&cfg).await.err().unwrap();
        assert_eq!(err.to_string(), "tailscale: no credentials");
    }

    #[tokio::test]
    async fn devices_with_an_api_key() {
        let now = jiff::Timestamp::now().to_string();
        let url = serve(devices_route(json!({"devices": [
            {"name": "web-1.example.ts.net", "lastSeen": now, "hostname": "web-1",
             "os": "linux", "addresses": ["100.64.0.2"], "tags": ["tag:server"],
             "user": "user@example.com", "clientVersion": "1.70.0-tabc-gdef",
             "updateAvailable": true, "keyExpiryDisabled": true,
             "expires": "2030-01-01T00:00:00Z", "isExternal": true},
            {"name": "db-1.example.ts.net", "connectedToControl": false, "lastSeen": now,
             "expires": "2030-01-01T00:00:00Z"},
            {"name": "laptop.example.ts.net", "lastSeen": "2020-01-01T00:00:00Z",
             "expires": "0001-01-01T00:00:00Z"},
            {"hostname": "nameless"},
        ]})))
        .await;
        let ts = tailscale(
            json!({"api_key": "tok", "hosts": ["web-*", "db-*"], "quiet_hosts": ["db-*"]}),
            &url,
        )
        .await;

        let d = ts.devices(true).await.unwrap();
        let names: Vec<&str> = d.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["db-1", "laptop", "web-1"]);
        let (db, laptop, web) = (&d[0], &d[1], &d[2]);

        // connectedToControl wins over a recent lastSeen
        assert!(!db.online && db.watched && db.quiet && db.is_watched());
        assert_eq!(db.key_expiry.as_deref(), Some("2030-01-01T00:00:00Z"));
        assert!(!laptop.online && !laptop.watched && !laptop.quiet);
        assert_eq!(laptop.key_expiry, None);
        assert!(web.online && web.watched && !web.quiet);
        assert_eq!(web.client_version, "1.70.0");
        assert_eq!(
            (web.addresses.clone(), web.tags.clone()),
            (
                vec!["100.64.0.2".to_string()],
                vec!["tag:server".to_string()]
            )
        );
        assert!(web.update_available && web.external);
        assert_eq!(web.key_expiry, None);
        assert_eq!((web.hostname.as_str(), web.os.as_str()), ("web-1", "linux"));

        // quiet hours off: nothing is quiet
        assert!(ts.devices(false).await.unwrap().iter().all(|d| !d.quiet));
    }

    #[tokio::test]
    async fn device_list_errors() {
        let url = serve(devices_route(json!({"devices": []}))).await;
        let ts = tailscale(json!({"api_key": "wrong"}), &url).await;
        let err = ts.devices(false).await.err().unwrap();
        assert!(http::is_unauthorized(&err), "{err}");
    }

    #[tokio::test]
    async fn oauth_token_is_fetched_once_and_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let app = devices_route(json!({"devices": [{"name": "a"}]})).route(
            "/api/v2/oauth/token",
            post(move |Form(f): Form<HashMap<String, String>>| async move {
                c.fetch_add(1, Ordering::Relaxed);
                assert_eq!(
                    (f["client_id"].as_str(), f["client_secret"].as_str()),
                    ("id", "secret")
                );
                assert_eq!(f["grant_type"], "client_credentials");
                Json(json!({"access_token": "tok", "expires_in": 3600}))
            }),
        );
        let url = serve(app).await;
        let ts = tailscale(
            json!({"oauth_client_id": "id", "oauth_client_secret": "secret"}),
            &url,
        )
        .await;
        assert_eq!(ts.devices(false).await.unwrap().len(), 1);
        assert_eq!(ts.devices(false).await.unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn oauth_token_errors() {
        let app = Router::new()
            .route(
                "/a/api/v2/oauth/token",
                post(|| async { Json(json!({"error": "nope"})) }),
            )
            .route(
                "/b/api/v2/oauth/token",
                post(|| async { StatusCode::UNAUTHORIZED }),
            );
        let url = serve(app).await;
        let cfg = json!({"oauth_client_id": "id", "oauth_client_secret": "secret"});

        let ts = tailscale(cfg.clone(), &format!("{url}/a")).await;
        let err = ts.devices(false).await.err().unwrap();
        assert_eq!(err.to_string(), "tailscale: no access_token");

        let ts = tailscale(cfg, &format!("{url}/b")).await;
        assert!(http::is_unauthorized(
            &ts.devices(false).await.err().unwrap()
        ));
    }

    #[tokio::test]
    async fn healthchecks_list() {
        let app = Router::new().route(
            "/hc/api/v3/checks/",
            get(|h: HeaderMap| async move {
                if header(&h, "x-api-key") != "key" {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                Ok(Json(json!({"checks": [
                    {"name": "backup", "status": "up", "last_ping": "2026-01-01T00:00:00+00:00",
                     "next_ping": "2026-01-02T00:00:00+00:00"},
                    {"status": "new"},
                ]})))
            }),
        );
        let url = serve(app).await;
        let cfg = |key: &str| -> config::Healthchecks {
            serde_json::from_value(
                json!({"url": format!("{url}/hc/"), "api_key": key, "quiet": true}),
            )
            .unwrap()
        };

        let hc = Healthchecks::new(&cfg("key")).await.unwrap();
        assert!(hc.quiet);
        assert_eq!(hc.url, format!("{url}/hc"));
        let checks = hc.checks().await.unwrap();
        assert_eq!(checks.len(), 2);
        assert_eq!(
            (checks[0].name.as_str(), checks[0].status.as_str()),
            ("backup", "up")
        );
        assert_eq!(
            checks[0].last_ping.as_deref(),
            Some("2026-01-01T00:00:00+00:00")
        );
        assert_eq!(
            checks[0].next_ping.as_deref(),
            Some("2026-01-02T00:00:00+00:00")
        );
        assert_eq!(
            (checks[1].name.as_str(), checks[1].last_ping.as_deref()),
            ("", None)
        );

        let wrong = Healthchecks::new(&cfg("wrong")).await.unwrap();
        assert!(http::is_unauthorized(&wrong.checks().await.err().unwrap()));
    }

    #[tokio::test]
    async fn healthchecks_config_errors() {
        let cfg = |v: Value| -> config::Healthchecks { serde_json::from_value(v).unwrap() };
        let err = Healthchecks::new(&cfg(json!({"url": "nope", "api_key": "k"})))
            .await
            .err()
            .unwrap();
        assert!(err.to_string().starts_with("healthchecks: "), "{err}");
        let err = Healthchecks::new(&cfg(json!({
            "url": "http://127.0.0.1:1",
            "api_key": {"env": "HOMIE_TEST_UNSET_HEALTHCHECKS_KEY"}
        })))
        .await
        .err()
        .unwrap();
        assert!(
            err.to_string().starts_with("healthchecks.api_key: "),
            "{err}"
        );
    }

    #[test]
    fn globs() {
        assert!(glob("*", "pve-01"));
        assert!(glob("pve-*", "pve-01"));
        assert!(glob("k0s-*-worker-*", "k0s-pve-01-worker-1"));
        assert!(!glob("pve-*", "k0s-controller-01"));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
    }
}

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
    use super::glob;

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

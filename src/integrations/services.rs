//! Status of the services a lab depends on (RFC 0011): OpenBao or Vault
//! seal state, and CloudNativePG clusters with their last backup.

use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ListParams},
    core::GroupVersionKind,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    Result, config,
    http::{self, Target},
};

pub struct OpenBao {
    pub name: String,
    pub quiet: bool,
    pub url: String,
    pub insecure: Option<&'static str>,
    target: Target,
}

#[derive(Serialize)]
pub struct BaoHealth {
    /// active, standby, sealed, uninitialized, or the HTTP code
    state: String,
    initialized: bool,
    sealed: bool,
    version: String,
    cluster_name: String,
}

/// `/v1/sys/health` answers with the state in the status code.
fn bao_state(code: u16) -> String {
    match code {
        200 => "active".into(),
        429 | 472 | 473 => "standby".into(),
        501 => "uninitialized".into(),
        503 => "sealed".into(),
        other => format!("HTTP {other}"),
    }
}

impl OpenBao {
    pub async fn new(name: &str, cfg: &config::OpenBao) -> Result<Self> {
        let target = http::target(&cfg.url, &cfg.tls())
            .await
            .map_err(|e| format!("openbao.{name}: {e}"))?;
        Ok(Self {
            name: name.to_string(),
            quiet: cfg.quiet,
            url: target.base.as_str().trim_end_matches('/').to_string(),
            insecure: target.insecure,
            target,
        })
    }

    /// Unauthenticated health; every documented state code is an answer, not an error.
    pub async fn health(&self) -> Result<BaoHealth> {
        let resp = self
            .target
            .client
            .get(http::join(&self.target.base, "/v1/sys/health"))
            .send()
            .await?;
        let code = resp.status().as_u16();
        if !matches!(code, 200 | 429 | 472 | 473 | 501 | 503) {
            return Err(http::ok(resp).await.err().unwrap());
        }
        let body: Value = resp.json().await?;
        let text = |k: &str| body[k].as_str().unwrap_or_default().to_string();
        Ok(BaoHealth {
            state: bao_state(code),
            initialized: body["initialized"].as_bool().unwrap_or(false),
            sealed: body["sealed"].as_bool().unwrap_or(code == 503),
            version: text("version"),
            cluster_name: text("cluster_name"),
        })
    }
}

#[derive(Serialize)]
pub struct PgCluster {
    namespace: String,
    name: String,
    phase: String,
    healthy: bool,
    instances: u64,
    ready: u64,
    primary: String,
    archiving: bool,
    /// stoppedAt of the newest completed Backup (or status.lastSuccessfulBackup)
    last_backup: Option<String>,
    backup_age_hours: Option<f64>,
    backup_stale: bool,
}

fn resource(kind: &str) -> ApiResource {
    ApiResource::from_gvk(&GroupVersionKind::gvk("postgresql.cnpg.io", "v1", kind))
}

fn condition(o: &DynamicObject, t: &str) -> bool {
    o.data["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|c| c["type"] == t && c["status"] == "True")
}

/// CloudNativePG clusters. Empty when the CRD is not installed. Backups made
/// through a plugin leave `lastSuccessfulBackup` empty, so the newest
/// completed Backup object counts.
pub async fn cnpg(client: Client, max_backup_age_hours: f64) -> Result<Vec<PgCluster>> {
    let clusters = match Api::<DynamicObject>::all_with(client.clone(), &resource("Cluster"))
        .list(&ListParams::default())
        .await
    {
        Ok(l) => l.items,
        Err(kube::Error::Api(s)) if s.code == 404 => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let backups = Api::<DynamicObject>::all_with(client, &resource("Backup"))
        .list(&ListParams::default())
        .await
        .map(|l| l.items)
        .unwrap_or_default();
    let now = jiff::Timestamp::now();
    Ok(clusters
        .iter()
        .map(|c| {
            let ns = c.metadata.namespace.clone().unwrap_or_default();
            let name = c.metadata.name.clone().unwrap_or_default();
            let st = &c.data["status"];
            let newest = backups
                .iter()
                .filter(|b| {
                    b.metadata.namespace.as_deref() == Some(ns.as_str())
                        && b.data["spec"]["cluster"]["name"] == name.as_str()
                        && b.data["status"]["phase"] == "completed"
                })
                .filter_map(|b| b.data["status"]["stoppedAt"].as_str().map(String::from))
                .max()
                .or_else(|| st["lastSuccessfulBackup"].as_str().map(String::from));
            let age = newest
                .as_deref()
                .and_then(|t| t.parse::<jiff::Timestamp>().ok())
                .map(|t| now.duration_since(t).as_secs_f64() / 3600.0);
            let instances = st["instances"].as_u64().unwrap_or(0);
            let ready = st["readyInstances"].as_u64().unwrap_or(0);
            PgCluster {
                phase: st["phase"].as_str().unwrap_or_default().to_string(),
                healthy: condition(c, "Ready") && ready >= instances && instances > 0,
                archiving: condition(c, "ContinuousArchiving"),
                primary: st["currentPrimary"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                backup_stale: age.is_none_or(|a| a > max_backup_age_hours),
                backup_age_hours: age.map(|a| (a * 10.0).round() / 10.0),
                last_backup: newest,
                instances,
                ready,
                namespace: ns,
                name,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_codes() {
        assert_eq!(bao_state(200), "active");
        assert_eq!(bao_state(429), "standby");
        assert_eq!(bao_state(473), "standby");
        assert_eq!(bao_state(503), "sealed");
        assert_eq!(bao_state(501), "uninitialized");
        assert_eq!(bao_state(500), "HTTP 500");
    }

    async fn bao(url: &str) -> Result<OpenBao> {
        let cfg = serde_json::from_value(serde_json::json!({"url": url, "quiet": true})).unwrap();
        OpenBao::new("bao", &cfg).await
    }

    /// One fake server; the URL path prefix picks the answer.
    #[tokio::test]
    async fn health_states() {
        use axum::{Json, Router, http::StatusCode, routing::get};
        let route = |code: u16, body: Value| {
            get(move || async move { (StatusCode::from_u16(code).unwrap(), Json(body)) })
        };
        let app = Router::new()
            .route(
                "/active/v1/sys/health",
                route(
                    200,
                    serde_json::json!({"initialized": true, "sealed": false, "version": "2.4.1", "cluster_name": "c1"}),
                ),
            )
            .route("/standby/v1/sys/health", route(429, serde_json::json!({"initialized": true})))
            .route("/sealed/v1/sys/health", route(503, serde_json::json!({})))
            .route("/broken/v1/sys/health", route(500, serde_json::json!({"errors": ["boom"]})));
        let url = crate::testkit::serve(app).await;

        let b = bao(&format!("{url}/active/")).await.unwrap();
        assert_eq!((b.name.as_str(), b.quiet), ("bao", true));
        assert_eq!(b.url, format!("{url}/active"));
        assert_eq!(b.insecure, Some("plain_http"));
        let h = b.health().await.unwrap();
        assert_eq!(
            (h.state.as_str(), h.initialized, h.sealed),
            ("active", true, false)
        );
        assert_eq!(
            (h.version.as_str(), h.cluster_name.as_str()),
            ("2.4.1", "c1")
        );

        let h = bao(&format!("{url}/standby"))
            .await
            .unwrap()
            .health()
            .await
            .unwrap();
        assert_eq!(
            (h.state.as_str(), h.initialized, h.sealed),
            ("standby", true, false)
        );

        // no body fields: sealed follows the status code
        let h = bao(&format!("{url}/sealed"))
            .await
            .unwrap()
            .health()
            .await
            .unwrap();
        assert_eq!(
            (h.state.as_str(), h.initialized, h.sealed),
            ("sealed", false, true)
        );
        assert_eq!(h.version, "");

        let err = bao(&format!("{url}/broken"))
            .await
            .unwrap()
            .health()
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("500"), "{err}");
    }

    #[tokio::test]
    async fn bad_url_names_the_instance() {
        let err = bao("not a url").await.err().unwrap();
        assert!(err.to_string().starts_with("openbao.bao:"), "{err}");
    }
}

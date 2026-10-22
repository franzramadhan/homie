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
            return Err(http::ok(resp).await.unwrap_err());
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
}

#[cfg(test)]
mod cnpg_tests {
    use super::*;
    use crate::testkit::{json as route, kube_client, list, serve};
    use axum::{Router, http::StatusCode, routing::get};
    use serde_json::json;

    fn hours_ago(h: i64) -> String {
        (jiff::Timestamp::now() - jiff::SignedDuration::from_hours(h)).to_string()
    }

    fn cluster(ns: &str, name: &str, status: Value) -> Value {
        json!({"metadata": {"name": name, "namespace": ns}, "status": status})
    }

    fn backup(ns: &str, cluster: &str, phase: &str, stopped: &str) -> Value {
        json!({"metadata": {"name": format!("{cluster}-{phase}"), "namespace": ns},
               "spec": {"cluster": {"name": cluster}},
               "status": {"phase": phase, "stoppedAt": stopped}})
    }

    #[tokio::test]
    async fn clusters_with_their_newest_backup() {
        let ready = json!([{"type": "Ready", "status": "True"},
                           {"type": "ContinuousArchiving", "status": "True"}]);
        let clusters = json!([
            cluster(
                "apps",
                "pg",
                json!({"phase": "Cluster in healthy state", "instances": 2,
                "readyInstances": 2, "currentPrimary": "pg-1", "conditions": ready})
            ),
            cluster(
                "apps",
                "legacy",
                json!({"instances": 1, "readyInstances": 0,
                "conditions": [{"type": "Ready", "status": "True"}],
                "lastSuccessfulBackup": hours_ago(48)})
            ),
            cluster("other", "pg", json!({})),
        ]);
        let backups = json!([
            backup("apps", "pg", "completed", &hours_ago(5)),
            backup("apps", "pg", "completed", &hours_ago(2)),
            backup("apps", "pg", "failed", &hours_ago(1)),
            backup("apps", "legacy", "running", &hours_ago(1)),
            backup("other", "legacy", "completed", &hours_ago(1)),
        ]);
        let app = route(
            "/apis/postgresql.cnpg.io/v1/clusters",
            list("Cluster", clusters),
        )
        .merge(route(
            "/apis/postgresql.cnpg.io/v1/backups",
            list("Backup", backups),
        ));
        let out = cnpg(kube_client(&serve(app).await), 26.0).await.unwrap();

        let pg = &out[0];
        assert!(pg.healthy && pg.archiving && !pg.backup_stale);
        assert_eq!(
            (pg.instances, pg.ready, pg.primary.as_str()),
            (2, 2, "pg-1")
        );
        assert_eq!(pg.backup_age_hours, Some(2.0));

        let legacy = &out[1];
        assert!(!legacy.healthy && !legacy.archiving);
        assert_eq!(legacy.backup_age_hours, Some(48.0));
        assert!(legacy.backup_stale, "older than 26 hours");

        let empty = &out[2];
        assert!(!empty.healthy && empty.backup_stale);
        assert_eq!(
            (empty.last_backup.as_deref(), empty.backup_age_hours),
            (None, None)
        );
    }

    #[tokio::test]
    async fn cnpg_not_installed_or_not_allowed() {
        let url = serve(Router::new()).await;
        assert!(cnpg(kube_client(&url), 26.0).await.unwrap().is_empty());

        // unreadable Backups only lose the backup age
        let clusters = list("Cluster", json!([cluster("apps", "pg", json!({}))]));
        let app = route("/apis/postgresql.cnpg.io/v1/clusters", clusters).route(
            "/apis/postgresql.cnpg.io/v1/backups",
            get(|| async { StatusCode::FORBIDDEN }),
        );
        let out = cnpg(kube_client(&serve(app).await), 26.0).await.unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].backup_stale);

        let app = Router::new().route(
            "/apis/postgresql.cnpg.io/v1/clusters",
            get(|| async { StatusCode::FORBIDDEN }),
        );
        assert!(cnpg(kube_client(&serve(app).await), 26.0).await.is_err());
    }
}

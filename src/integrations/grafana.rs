//! Grafana (self-hosted or Grafana Cloud) through its HTTP API with a Viewer
//! service account token: alert state, and queries through the datasource
//! proxy, so Homie never needs a route to the data stores themselves.

use serde::Serialize;
use serde_json::{Value, json};
use url::Url;

use crate::{
    Result, config,
    http::{self, Target},
    secret::SecretCell,
};

pub struct Grafana {
    pub name: String,
    pub quiet: bool,
    pub url: String,
    pub insecure: Option<&'static str>,
    target: Target,
    token: SecretCell,
    ignore: Vec<String>,
}

#[derive(Serialize)]
pub struct Alert {
    name: String,
    state: String,
    severity: String,
    summary: String,
    active_at: String,
    labels: serde_json::Map<String, Value>,
}

#[derive(Serialize)]
pub struct Alerts {
    alerts: Vec<Alert>,
    link: String,
}

/// One series of an instant PromQL query.
pub struct Sample {
    pub labels: serde_json::Map<String, Value>,
    pub value: f64,
}

impl Grafana {
    pub async fn new(name: &str, cfg: &config::Grafana) -> Result<Self> {
        let target = http::target(&cfg.url, &cfg.tls())
            .await
            .map_err(|e| format!("grafana.{name}: {e}"))?;
        Ok(Self {
            name: name.to_string(),
            quiet: cfg.quiet,
            // links only; requests go through `target`
            url: cfg
                .ui_url
                .as_deref()
                .unwrap_or(target.base.as_str())
                .trim_end_matches('/')
                .to_string(),
            insecure: target.insecure,
            token: SecretCell::new(&cfg.token)
                .await
                .map_err(|e| format!("grafana.{name}.token: {e}"))?,
            target,
            ignore: cfg.ignore_alerts.clone(),
        })
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Response> {
        let mut reloaded = false;
        loop {
            let resp = self
                .target
                .client
                .get(http::join(&self.target.base, path))
                .query(query)
                .bearer_auth(self.token.get().await)
                .send()
                .await?;
            match http::ok(resp).await {
                Err(e) if http::is_unauthorized(&e) && !reloaded => {
                    reloaded = true;
                    self.token.reload().await?;
                }
                other => return other,
            }
        }
    }

    /// Firing and pending Grafana-managed alerts.
    pub async fn alerts(&self) -> Result<Alerts> {
        let body: Value = self
            .get("/api/prometheus/grafana/api/v1/alerts", &[])
            .await?
            .json()
            .await?;
        let mut alerts: Vec<Alert> = body["data"]["alerts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let state = state(a["state"].as_str().unwrap_or_default())?;
                let labels = a["labels"].as_object().cloned().unwrap_or_default();
                let label = |k: &str| {
                    labels
                        .get(k)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let annotation =
                    |k: &str| a["annotations"][k].as_str().unwrap_or_default().to_string();
                Some(Alert {
                    name: label("alertname"),
                    severity: label("severity"),
                    summary: Some(annotation("summary"))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| annotation("description")),
                    active_at: a["activeAt"].as_str().unwrap_or_default().to_string(),
                    state: state.to_string(),
                    labels,
                })
            })
            .collect();
        alerts.retain(|a| !self.ignore.contains(&a.name));
        alerts.sort_by(|a, b| (a.state != "firing", &a.name).cmp(&(b.state != "firing", &b.name)));
        Ok(Alerts {
            alerts,
            link: format!("{}/alerting/list", self.url),
        })
    }

    /// Range PromQL query over the last `seconds`, one point per `step`.
    /// Returns each series' labels and values.
    pub async fn promql_range(
        &self,
        datasource: &str,
        query: &str,
        seconds: i64,
        step: i64,
    ) -> Result<Vec<(serde_json::Map<String, Value>, Vec<f64>)>> {
        let series = self.promql_points(datasource, query, seconds, step).await?;
        Ok(series
            .into_iter()
            .map(|(l, p)| (l, p.into_iter().map(|(_, v)| v).collect()))
            .collect())
    }

    /// `promql_range` with each value's timestamp (seconds).
    pub async fn promql_points(
        &self,
        datasource: &str,
        query: &str,
        seconds: i64,
        step: i64,
    ) -> Result<Vec<(serde_json::Map<String, Value>, Vec<(f64, f64)>)>> {
        let path = format!(
            "/api/datasources/proxy/uid/{}/api/v1/query_range",
            enc(datasource)
        );
        let end = jiff::Timestamp::now().as_second();
        let (start, end, step) = (
            (end - seconds).to_string(),
            end.to_string(),
            step.to_string(),
        );
        let body: Value = self
            .get(
                &path,
                &[
                    ("query", query),
                    ("start", &start),
                    ("end", &end),
                    ("step", &step),
                ],
            )
            .await?
            .json()
            .await?;
        if body["status"] != "success" {
            return Err(format!(
                "query failed: {}",
                body["error"].as_str().unwrap_or("unknown error")
            )
            .into());
        }
        Ok(body["data"]["result"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                let values = r["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Some((p[0].as_f64()?, p[1].as_str()?.parse().ok()?)))
                    .collect();
                (r["metric"].as_object().cloned().unwrap_or_default(), values)
            })
            .collect())
    }

    /// Instant PromQL query through a Prometheus-compatible datasource.
    pub async fn promql(&self, datasource: &str, query: &str) -> Result<Vec<Sample>> {
        let path = format!(
            "/api/datasources/proxy/uid/{}/api/v1/query",
            enc(datasource)
        );
        let body: Value = self.get(&path, &[("query", query)]).await?.json().await?;
        if body["status"] != "success" {
            return Err(format!(
                "query failed: {}",
                body["error"].as_str().unwrap_or("unknown error")
            )
            .into());
        }
        Ok(body["data"]["result"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| Sample {
                labels: r["metric"].as_object().cloned().unwrap_or_default(),
                value: r["value"][1]
                    .as_str()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(f64::NAN),
            })
            .collect())
    }

    /// LogsQL query through a VictoriaLogs datasource. Returns one JSON object per row.
    pub async fn logsql(
        &self,
        datasource: &str,
        query: &str,
    ) -> Result<Vec<serde_json::Map<String, Value>>> {
        self.logsql_params(datasource, &[("query", query)]).await
    }

    /// LogsQL with extra arguments (`start`, `limit`).
    pub async fn logsql_params(
        &self,
        datasource: &str,
        params: &[(&str, &str)],
    ) -> Result<Vec<serde_json::Map<String, Value>>> {
        let path = format!(
            "/api/datasources/proxy/uid/{}/select/logsql/query",
            enc(datasource)
        );
        let text = self.get(&path, params).await?.text().await?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                Ok(serde_json::from_str::<Value>(l)?
                    .as_object()
                    .cloned()
                    .unwrap_or_default())
            })
            .collect()
    }

    /// LogQL range query through a Loki datasource from `start` (Unix
    /// nanoseconds) to now, newest first.
    pub async fn loki_range(
        &self,
        datasource: &str,
        query: &str,
        start: i128,
        limit: usize,
    ) -> Result<Value> {
        let path = format!(
            "/api/datasources/proxy/uid/{}/loki/api/v1/query_range",
            enc(datasource)
        );
        let end = jiff::Timestamp::now().as_nanosecond();
        let (start, end, limit) = (start.to_string(), end.to_string(), limit.to_string());
        let params = [
            ("query", query),
            ("start", &start),
            ("end", &end),
            ("limit", &limit),
            ("direction", "backward"),
        ];
        Ok(self.get(&path, &params).await?.json().await?)
    }

    /// Explore with this query prefilled, last 24 h.
    pub fn explore(&self, datasource: &str, expr: &str) -> String {
        let panes = json!({"a": {
            "datasource": datasource,
            "queries": [{"refId": "A", "expr": expr, "datasource": {"uid": datasource}}],
            "range": {"from": "now-24h", "to": "now"}
        }});
        let mut u =
            Url::parse(&format!("{}/explore", self.url)).expect("grafana URL was validated");
        u.query_pairs_mut()
            .append_pair("schemaVersion", "1")
            .append_pair("panes", &panes.to_string());
        u.to_string()
    }

    pub async fn probe(&self) -> Result<String> {
        let health: Value = self.get("/api/health", &[]).await?.json().await?;
        // /api/search needs a valid token, /api/health does not
        self.get("/api/search", &[("limit", "1")]).await?;
        Ok(format!(
            "Grafana {}, token accepted",
            health["version"].as_str().unwrap_or("?")
        ))
    }
}

/// Grafana's alert state, reduced to what the UI shows. "Normal (NoData)" is
/// normal: the rule is fine, its query returned nothing. Returns None for
/// states that are not worth listing.
fn state(raw: &str) -> Option<&'static str> {
    let raw = raw.to_lowercase();
    if raw.starts_with("normal") || raw == "inactive" {
        None
    } else if raw.starts_with("alerting") || raw == "firing" {
        Some("firing")
    } else if raw.starts_with("pending") {
        Some("pending")
    } else if raw.starts_with("error") || raw.starts_with("nodata") {
        Some("error")
    } else {
        None
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{secret::Secret, testkit};
    use axum::{
        Json, Router,
        extract::{Path, Query},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use std::{collections::HashMap, path::PathBuf};

    fn cfg(url: &str, token: Secret) -> config::Grafana {
        config::Grafana {
            url: url.into(),
            ui_url: None,
            token,
            tls_verify: true,
            ca_file: None,
            tls_server_name: None,
            quiet: false,
            ignore_alerts: vec!["Watchdog".into()],
        }
    }

    fn bearer(h: &HeaderMap) -> String {
        h.get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .trim_start_matches("Bearer ")
            .to_string()
    }

    /// A Grafana that accepts the token "new". Seeing "old", it rotates the
    /// token file to "new" and answers 401, as after a rotation.
    async fn fake(token_file: PathBuf) -> String {
        let alerts = json!({"data": {"alerts": [
            {"state": "Pending", "labels": {"alertname": "B", "severity": "warning"},
             "annotations": {"description": "only a description"}, "activeAt": "t1"},
            {"state": "Alerting", "labels": {"alertname": "Z", "severity": "critical"},
             "annotations": {"summary": "disk full", "description": "ignored"}, "activeAt": "t2"},
            {"state": "Alerting", "labels": {"alertname": "A"}},
            {"state": "Normal (NoData)", "labels": {"alertname": "C"}},
            {"state": "Alerting", "labels": {"alertname": "Watchdog"}},
            {"state": "NoData", "labels": {"alertname": "D"}},
            {"state": "Unknown", "labels": {"alertname": "E"}},
        ]}});
        let search = move |h: HeaderMap| {
            let file = token_file.clone();
            async move {
                match bearer(&h).as_str() {
                    "new" => (StatusCode::OK, "[]").into_response(),
                    "old" => {
                        std::fs::write(&file, "new\n").unwrap();
                        (StatusCode::UNAUTHORIZED, "token expired").into_response()
                    }
                    _ => (StatusCode::UNAUTHORIZED, "invalid API key").into_response(),
                }
            }
        };
        let prom = |Path((uid, api)): Path<(String, String)>,
                    Query(q): Query<HashMap<String, String>>| async move {
            let body = match (uid.as_str(), api.as_str()) {
                ("vm", "query") => {
                    assert_eq!(q["query"], "up");
                    json!({"status": "success", "data": {"result": [
                        {"metric": {"job": "node"}, "value": [1, "1"]},
                        {"metric": {"job": "bad"}, "value": [1, "x"]},
                    ]}})
                }
                ("vm", "query_range") => {
                    let span: i64 =
                        q["end"].parse::<i64>().unwrap() - q["start"].parse::<i64>().unwrap();
                    assert_eq!((span, q["step"].as_str()), (3600, "60"));
                    json!({"status": "success", "data": {"result": [
                        {"metric": {"job": "node"}, "values": [[1, "1.5"], [2, "oops"], [3, "2"]]},
                        {"values": []},
                    ]}})
                }
                ("broken", _) => json!({"status": "error", "error": "parse error"}),
                _ => json!({"status": "error"}),
            };
            Json(body)
        };
        let logsql = |Path(uid): Path<String>| async move {
            match uid.as_str() {
                "vl" => "{\"_msg\": \"a\"}\n\n[1]\n{\"_msg\": \"b\"}\n".to_string(),
                _ => "not json\n".to_string(),
            }
        };
        let loki = |Query(q): Query<HashMap<String, String>>| async move {
            assert_eq!(q["start"], "42");
            assert!(q["end"].parse::<i128>().unwrap() > 42);
            assert_eq!(
                (q["limit"].as_str(), q["direction"].as_str()),
                ("10", "backward")
            );
            Json(json!({"status": "success", "query": q["query"]}))
        };
        testkit::serve(
            Router::new()
                .route(
                    "/api/health",
                    get(|| async { Json(json!({"version": "11.2.0"})) }),
                )
                .route("/api/search", get(search))
                .route(
                    "/api/prometheus/grafana/api/v1/alerts",
                    get(move || async move { Json(alerts) }),
                )
                .route("/api/datasources/proxy/uid/{uid}/api/v1/{api}", get(prom))
                .route(
                    "/api/datasources/proxy/uid/{uid}/select/logsql/query",
                    get(logsql),
                )
                .route(
                    "/api/datasources/proxy/uid/{uid}/loki/api/v1/query_range",
                    get(loki),
                ),
        )
        .await
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("homie-grafana-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn alerts_filtered_and_sorted() {
        let url = fake(tmp("alerts").join("unused")).await;
        let mut c = cfg(&url, Secret::Plain("new".into()));
        c.ui_url = Some("https://grafana.example/".into());
        let g = Grafana::new("lab", &c).await.unwrap();
        assert_eq!((g.name.as_str(), g.insecure), ("lab", Some("plain_http")));
        let a = g.alerts().await.unwrap();
        let got: Vec<_> = a
            .alerts
            .iter()
            .map(|a| (a.name.as_str(), a.state.as_str(), a.summary.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("A", "firing", ""),
                ("Z", "firing", "disk full"),
                ("B", "pending", "only a description"),
                ("D", "error", ""),
            ]
        );
        assert_eq!(a.alerts[1].severity, "critical");
        assert_eq!(a.alerts[1].active_at, "t2");
        assert_eq!(a.link, "https://grafana.example/alerting/list");
    }

    #[tokio::test]
    async fn token_reloaded_once_after_401() {
        let dir = tmp("reload");
        let file = dir.join("token");
        std::fs::write(&file, "old").unwrap();
        let url = fake(file.clone()).await;
        let g = Grafana::new("lab", &cfg(&url, Secret::File { file }))
            .await
            .unwrap();
        assert_eq!(g.url, url);
        assert_eq!(g.probe().await.unwrap(), "Grafana 11.2.0, token accepted");
        assert_eq!(g.token.get().await, "new");

        let g = Grafana::new("lab", &cfg(&url, Secret::Plain("bad".into())))
            .await
            .unwrap();
        let e = g.probe().await.unwrap_err().to_string();
        assert_eq!(e, "HTTP 401 Unauthorized: invalid API key");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn datasource_queries() {
        let url = fake(tmp("ds").join("unused")).await;
        let g = Grafana::new("lab", &cfg(&url, Secret::Plain("new".into())))
            .await
            .unwrap();

        let s = g.promql("vm", "up").await.unwrap();
        assert_eq!(
            (s[0].labels["job"].as_str(), s[0].value),
            (Some("node"), 1.0)
        );
        assert!(s[1].value.is_nan());
        let e = g.promql("broken", "up").await.err().unwrap();
        assert_eq!(e.to_string(), "query failed: parse error");
        let e = g.promql("other", "up").await.err().unwrap();
        assert_eq!(e.to_string(), "query failed: unknown error");

        let r = g.promql_range("vm", "rate(x[5m])", 3600, 60).await.unwrap();
        assert_eq!(r[0].0["job"], "node");
        assert_eq!(r[0].1, [1.5, 2.0]);
        assert!(r[1].0.is_empty() && r[1].1.is_empty());
        let e = g.promql_range("broken", "x", 60, 60).await.unwrap_err();
        assert_eq!(e.to_string(), "query failed: parse error");

        let rows = g.logsql("vl", "*").await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0]["_msg"].as_str(), rows[2]["_msg"].as_str()),
            (Some("a"), Some("b"))
        );
        assert!(rows[1].is_empty(), "a non-object row is an empty row");
        assert!(g.logsql("garbage", "*").await.is_err());

        let body = g.loki_range("lk", "{unit=\"x\"}", 42, 10).await.unwrap();
        assert_eq!(body["query"], "{unit=\"x\"}");
    }

    #[tokio::test]
    async fn explore_link_and_config_errors() {
        let mut c = cfg("http://127.0.0.1:9", Secret::Plain("t".into()));
        c.ui_url = Some("https://grafana.example/sub/".into());
        let g = Grafana::new("lab", &c).await.unwrap();
        let link = Url::parse(&g.explore("vm", "up{job=\"x\"}")).unwrap();
        assert_eq!(link.path(), "/sub/explore");
        let q: HashMap<_, _> = link.query_pairs().into_owned().collect();
        assert_eq!(q["schemaVersion"], "1");
        let panes: Value = serde_json::from_str(&q["panes"]).unwrap();
        assert_eq!(panes["a"]["queries"][0]["expr"], "up{job=\"x\"}");
        assert_eq!(panes["a"]["datasource"], "vm");

        let e = Grafana::new("lab", &cfg("not a url", Secret::Plain("t".into())))
            .await
            .err()
            .unwrap();
        assert!(e.to_string().starts_with("grafana.lab: "), "{e}");
        let unset = Secret::Env {
            env: "HOMIE_TEST_GRAFANA_UNSET".into(),
        };
        let e = Grafana::new("lab", &cfg("http://x", unset))
            .await
            .err()
            .unwrap();
        assert_eq!(
            e.to_string(),
            "grafana.lab.token: environment variable HOMIE_TEST_GRAFANA_UNSET is not set"
        );
    }

    #[test]
    fn alert_states() {
        assert_eq!(state("Normal"), None);
        assert_eq!(state("Normal (NoData)"), None);
        assert_eq!(state("Normal (Error)"), None);
        assert_eq!(state("Alerting"), Some("firing"));
        assert_eq!(state("Alerting (NoData)"), Some("firing"));
        assert_eq!(state("firing"), Some("firing"));
        assert_eq!(state("Pending"), Some("pending"));
        assert_eq!(state("NoData"), Some("error"));
        assert_eq!(state("Error"), Some("error"));
    }
}

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
            url: target.base.as_str().trim_end_matches('/').to_string(),
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
                    .filter_map(|p| p[1].as_str()?.parse().ok())
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

    /// LogQL range query through a Loki datasource, newest first.
    pub async fn loki_range(
        &self,
        datasource: &str,
        query: &str,
        seconds: i64,
        limit: usize,
    ) -> Result<Value> {
        let path = format!(
            "/api/datasources/proxy/uid/{}/loki/api/v1/query_range",
            enc(datasource)
        );
        let end = jiff::Timestamp::now().as_nanosecond();
        let start = end - i128::from(seconds) * 1_000_000_000;
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
    use super::state;

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

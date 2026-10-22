//! Log search (RFC 0008) over the configured log datasources, through
//! Grafana: VictoriaLogs (LogsQL) and Loki (LogQL).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::grafana::Grafana;
use crate::{
    Result,
    config::{LogKind, LogSource},
};

#[derive(Deserialize)]
pub struct SearchRequest {
    pub source: String,
    pub query: String,
    pub minutes: i64,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Row {
    time: String,
    message: String,
    labels: String,
}

#[derive(Serialize)]
pub struct SearchResult {
    rows: Vec<Row>,
    truncated: bool,
}

const MAX_MINUTES: i64 = 7 * 24 * 60;

/// Terminal color codes (ESC [ ... m) make log lines unreadable in a table.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Checks the request against the server-side caps.
pub fn validate(req: &SearchRequest, max_rows: usize) -> Result<usize> {
    let q = req.query.trim();
    if q.is_empty() || q.len() > 4096 {
        return Err("query must be 1 to 4096 characters".into());
    }
    if !(1..=MAX_MINUTES).contains(&req.minutes) {
        return Err("time range must be 1 minute to 7 days".into());
    }
    Ok(req.limit.unwrap_or(max_rows).clamp(1, max_rows))
}

fn victorialogs_rows(rows: Vec<serde_json::Map<String, Value>>) -> Vec<Row> {
    rows.into_iter()
        .map(|r| {
            let text = |k: &str| {
                r.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Row {
                time: text("_time"),
                message: strip_ansi(&text("_msg")),
                labels: text("_stream"),
            }
        })
        .collect()
}

fn loki_rows(body: &Value) -> Vec<Row> {
    let mut rows = Vec::new();
    for s in body["data"]["result"].as_array().into_iter().flatten() {
        let labels = s["stream"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        for v in s["values"].as_array().into_iter().flatten() {
            let ns: i128 = v[0].as_str().and_then(|t| t.parse().ok()).unwrap_or(0);
            let time = jiff::Timestamp::from_nanosecond(ns)
                .map(|t| t.to_string())
                .unwrap_or_default();
            rows.push(Row {
                time,
                message: strip_ansi(v[1].as_str().unwrap_or_default()),
                labels: labels.clone(),
            });
        }
    }
    rows
}

pub async fn search(
    g: &Grafana,
    src: &LogSource,
    req: &SearchRequest,
    limit: usize,
) -> Result<SearchResult> {
    let query = req.query.trim();
    let mut rows = match src.kind {
        LogKind::Victorialogs => {
            let start = (jiff::Timestamp::now().as_second() - req.minutes * 60).to_string();
            // one extra row tells whether the result was cut
            let lim = (limit + 1).to_string();
            victorialogs_rows(
                g.logsql_params(
                    &src.datasource,
                    &[("query", query), ("start", &start), ("limit", &lim)],
                )
                .await?,
            )
        }
        LogKind::Loki => {
            let body = g
                .loki_range(&src.datasource, query, req.minutes * 60, limit + 1)
                .await?;
            if body["status"] != "success" {
                return Err(format!(
                    "query failed: {}",
                    body["error"].as_str().unwrap_or("unknown error")
                )
                .into());
            }
            loki_rows(&body)
        }
    };
    rows.sort_by(|a, b| b.time.cmp(&a.time));
    let truncated = rows.len() > limit;
    rows.truncate(limit);
    Ok(SearchResult { rows, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn caps_and_parsing() {
        let req = |q: &str, m: i64, l: Option<usize>| SearchRequest {
            source: "x".into(),
            query: q.into(),
            minutes: m,
            limit: l,
        };
        assert_eq!(validate(&req("a", 15, None), 1000).unwrap(), 1000);
        assert_eq!(validate(&req("a", 15, Some(5000)), 1000).unwrap(), 1000);
        assert!(validate(&req(" ", 15, None), 1000).is_err());
        assert!(validate(&req("a", 0, None), 1000).is_err());
        assert!(validate(&req("a", MAX_MINUTES + 1, None), 1000).is_err());

        assert_eq!(
            strip_ansi("\u{1b}[90mtime\u{1b}[0m INF \u{1b}[1mmsg"),
            "time INF msg"
        );

        let vl = vec![
            serde_json::from_value(
                json!({"_time": "2026-10-22T08:00:00Z", "_msg": "hello", "_stream": "{a=\"b\"}"}),
            )
            .unwrap(),
        ];
        assert_eq!(victorialogs_rows(vl)[0].message, "hello");
        let loki = json!({"data": {"result": [{"stream": {"unit": "x"}, "values": [["1792656000000000000", "line"]]}]}});
        let r = loki_rows(&loki);
        assert_eq!(
            (r[0].labels.as_str(), r[0].message.as_str()),
            ("unit=x", "line")
        );
        assert!(r[0].time.starts_with("2026-10-22T08:00:00"));
    }
}

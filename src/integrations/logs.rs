//! Log search (RFC 0008) over the configured log datasources, through
//! Grafana: VictoriaLogs (LogsQL) and Loki (LogQL). Export (RFC 0029) pages
//! a search through its whole range.

use std::collections::HashSet;

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
    /// live tail (RFC 0022): only lines from this time on, at most 5 minutes back
    #[serde(default)]
    pub since: Option<String>,
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
const MAX_SINCE_SECONDS: i64 = 5 * 60;

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

/// Checks the request against the server-side caps: the row limit and the
/// start of the window.
pub fn validate(req: &SearchRequest, max_rows: usize) -> Result<(usize, jiff::Timestamp)> {
    let q = req.query.trim();
    if q.is_empty() || q.len() > 4096 {
        return Err("query must be 1 to 4096 characters".into());
    }
    if !(1..=MAX_MINUTES).contains(&req.minutes) {
        return Err("time range must be 1 minute to 7 days".into());
    }
    let now = jiff::Timestamp::now();
    let start = match &req.since {
        None => now - jiff::SignedDuration::from_mins(req.minutes),
        Some(s) => {
            let since: jiff::Timestamp = s.parse().map_err(|_| "since must be an RFC 3339 time")?;
            since.clamp(
                now - jiff::SignedDuration::from_secs(MAX_SINCE_SECONDS),
                now,
            )
        }
    };
    Ok((req.limit.unwrap_or(max_rows).clamp(1, max_rows), start))
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

/// Lines from `start` to `end` (inclusive), newest first, at most `limit`.
pub async fn search(
    g: &Grafana,
    src: &LogSource,
    req: &SearchRequest,
    limit: usize,
    start: jiff::Timestamp,
    end: jiff::Timestamp,
) -> Result<SearchResult> {
    let query = req.query.trim();
    let mut rows = match src.kind {
        LogKind::Victorialogs => {
            // RFC 3339 keeps the nanoseconds a tail's `since` carries
            let (start, end) = (start.to_string(), end.to_string());
            // one extra row tells whether the result was cut
            let lim = (limit + 1).to_string();
            victorialogs_rows(
                g.logsql_params(
                    &src.datasource,
                    &[
                        ("query", query),
                        ("start", &start),
                        ("end", &end),
                        ("limit", &lim),
                    ],
                )
                .await?,
            )
        }
        LogKind::Loki => {
            let body = g
                // Loki's end is exclusive
                .loki_range(
                    &src.datasource,
                    query,
                    start.as_nanosecond(),
                    end.as_nanosecond() + 1,
                    limit + 1,
                )
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

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Ndjson,
}

#[derive(Deserialize)]
pub struct ExportRequest {
    #[serde(flatten)]
    pub search: SearchRequest,
    pub format: Format,
}

impl Row {
    fn key(&self) -> String {
        format!("{}|{}|{}", self.time, self.labels, self.message)
    }
}

/// A CSV cell, quoted when needed. A leading `=`, `+`, `-`, `@`, tab or
/// carriage return gets a `'` so a spreadsheet does not run it as a formula.
fn csv_cell(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Ndjson => "ndjson",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Format::Csv => "text/csv; charset=utf-8",
            Format::Ndjson => "application/x-ndjson",
        }
    }

    pub fn head(self) -> &'static str {
        match self {
            Format::Csv => "time,stream,message\n",
            Format::Ndjson => "",
        }
    }

    pub fn line(self, r: &Row) -> String {
        match self {
            Format::Csv => format!(
                "{},{},{}\n",
                csv_cell(&r.time),
                csv_cell(&r.labels),
                csv_cell(&r.message)
            ),
            Format::Ndjson => serde_json::to_string(r).unwrap_or_default() + "\n",
        }
    }

    /// The last line when an export stops early: the cap or an error.
    pub fn note(self, msg: &str) -> String {
        match self {
            Format::Csv => format!(",,{}\n", csv_cell(msg)),
            Format::Ndjson => {
                serde_json::json!({ "truncated": true, "message": msg }).to_string() + "\n"
            }
        }
    }
}

/// Where an export stands between pages (RFC 0029): the end of the next
/// page, inclusive, and the lines on that end already written.
pub struct Cursor {
    end: Option<jiff::Timestamp>,
    seen: HashSet<String>,
}

impl Cursor {
    pub fn new(end: jiff::Timestamp) -> Self {
        Self {
            end: Some(end),
            seen: HashSet::new(),
        }
    }

    /// The next page, newest first, without lines already written. `None`
    /// once the range is done.
    pub async fn next(
        &mut self,
        g: &Grafana,
        src: &LogSource,
        req: &SearchRequest,
        size: usize,
        start: jiff::Timestamp,
    ) -> Result<Option<Vec<Row>>> {
        let Some(end) = self.end else {
            return Ok(None);
        };
        let r = search(g, src, req, size, start, end).await?;
        let oldest = r
            .rows
            .last()
            .and_then(|row| row.time.parse::<jiff::Timestamp>().ok());
        let rows: Vec<Row> = r
            .rows
            .into_iter()
            .filter(|row| !self.seen.contains(&row.key()))
            .collect();
        self.end = match oldest {
            // a whole page on one timestamp: step past it.
            // ponytail: lines beyond `size` on one nanosecond are skipped
            Some(o) if r.truncated && o == end => {
                self.seen.clear();
                Some(o - jiff::SignedDuration::from_nanos(1))
            }
            // the next page ends at this page's oldest line, which comes
            // back again with any others on the same timestamp
            Some(o) if r.truncated => {
                self.seen = rows
                    .iter()
                    .filter(|row| row.time.parse::<jiff::Timestamp>().ok() == Some(o))
                    .map(Row::key)
                    .collect();
                Some(o)
            }
            _ => None,
        };
        Ok(Some(rows))
    }
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
            since: None,
        };
        assert_eq!(validate(&req("a", 15, None), 1000).unwrap().0, 1000);
        assert_eq!(validate(&req("a", 15, Some(5000)), 1000).unwrap().0, 1000);
        assert!(validate(&req(" ", 15, None), 1000).is_err());
        assert!(validate(&req("a", 0, None), 1000).is_err());
        assert!(validate(&req("a", MAX_MINUTES + 1, None), 1000).is_err());

        // a tail's `since`: kept when recent, at most 5 minutes back, never ahead
        let tail = |since: &str| {
            let mut r = req("a", 15, None);
            r.since = Some(since.into());
            validate(&r, 1000).map(|v| v.1)
        };
        let now = jiff::Timestamp::now();
        let recent = now - jiff::SignedDuration::from_secs(10);
        assert_eq!(tail(&recent.to_string()).unwrap(), recent);
        let old = tail("2020-01-01T00:00:00Z").unwrap();
        assert!((now.as_second() - MAX_SINCE_SECONDS - old.as_second()).abs() < 5);
        assert!(tail("2999-01-01T00:00:00Z").unwrap() >= now);
        assert!(tail("yesterday").is_err());

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

    async fn grafana() -> Grafana {
        use axum::{
            Json, Router,
            extract::{Path, Query},
            routing::get,
        };
        use std::collections::HashMap;

        let vl = |Query(q): Query<HashMap<String, String>>| async move {
            assert_eq!((q["query"].as_str(), q["limit"].as_str()), ("error", "3"));
            let start: jiff::Timestamp = q["start"].parse().unwrap();
            assert!((jiff::Timestamp::now().as_second() - start.as_second() - 900).abs() < 5);
            [
                r#"{"_time": "2026-10-22T08:00:01Z", "_msg": "b", "_stream": "{unit=\"x\"}"}"#,
                r#"{"_time": "2026-10-22T08:00:03Z", "_msg": "\u001b[31mc\u001b[0m"}"#,
                r#"{"_time": "2026-10-22T08:00:02Z", "_msg": "a"}"#,
            ]
            .join("\n")
        };
        let loki = |Path(uid): Path<String>, Query(q): Query<HashMap<String, String>>| async move {
            assert_eq!(q["limit"], "3");
            let start: i128 = q["start"].parse().unwrap();
            let ago = jiff::Timestamp::now().as_nanosecond() - start;
            assert!(
                (ago / 1_000_000_000 - 900).abs() < 5,
                "start is 15 minutes back"
            );
            Json(match uid.as_str() {
                "lk" => json!({"status": "success", "data": {"result": [
                    {"stream": {"unit": "x"}, "values": [["1792656000000000000", "old"]]},
                    {"stream": {"unit": "y"}, "values": [["1792656060000000000", "new"]]},
                ]}}),
                "lk-bad" => json!({"status": "error", "error": "parse error at line 1"}),
                _ => json!({"status": "error"}),
            })
        };
        let url = crate::testkit::serve(
            Router::new()
                .route("/api/datasources/proxy/uid/vl/select/logsql/query", get(vl))
                .route(
                    "/api/datasources/proxy/uid/{uid}/loki/api/v1/query_range",
                    get(loki),
                ),
        )
        .await;
        let cfg = crate::config::Grafana {
            url,
            ui_url: None,
            token: crate::secret::Secret::Plain("t".into()),
            tls_verify: true,
            ca_file: None,
            tls_server_name: None,
            quiet: false,
            ignore_alerts: Vec::new(),
        };
        Grafana::new("lab", &cfg).await.unwrap()
    }

    fn source(kind: LogKind, datasource: &str) -> LogSource {
        LogSource {
            label: "logs".into(),
            grafana: "lab".into(),
            datasource: datasource.into(),
            kind,
            pod_query: None,
            node_query: None,
        }
    }

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            source: "logs".into(),
            query: query.into(),
            minutes: 15,
            limit: None,
            since: None,
        }
    }

    async fn run(
        g: &Grafana,
        src: &LogSource,
        req: &SearchRequest,
        limit: usize,
    ) -> Result<SearchResult> {
        let (_, start) = validate(req, 1000)?;
        search(g, src, req, limit, start, jiff::Timestamp::now()).await
    }

    #[tokio::test]
    async fn search_victorialogs_newest_first_and_truncated() {
        let g = grafana().await;
        let src = source(LogKind::Victorialogs, "vl");
        let r = run(&g, &src, &request("  error "), 2).await.unwrap();
        assert!(r.truncated);
        let msgs: Vec<_> = r.rows.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(msgs, ["c", "a"]);
    }

    #[test]
    fn export_lines() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\"\nbye"), "\"say \"\"hi\"\"\nbye\"");
        assert_eq!(csv_cell("=1+1"), "'=1+1");
        assert_eq!(csv_cell("-x,y"), "\"'-x,y\"");
        let row = Row {
            time: "2026-10-22T08:00:00Z".into(),
            message: "@a".into(),
            labels: "unit=x".into(),
        };
        assert_eq!(Format::Csv.line(&row), "2026-10-22T08:00:00Z,unit=x,'@a\n");
        let v: Value = serde_json::from_str(&Format::Ndjson.line(&row)).unwrap();
        assert_eq!(v["message"], "@a");
        assert_eq!(Format::Csv.note("stopped, at 5"), ",,\"stopped, at 5\"\n");
    }

    /// A VictoriaLogs that answers from a fixed list: lines up to `end`,
    /// newest first, at most `limit`.
    async fn paged() -> Grafana {
        use axum::{Router, extract::Query, routing::get};
        use std::collections::HashMap;

        let vl = |Query(q): Query<HashMap<String, String>>| async move {
            let end: jiff::Timestamp = q["end"].parse().unwrap();
            let limit: usize = q["limit"].parse().unwrap();
            [
                ("05", "e"),
                ("04", "d"),
                ("03", "c1"),
                ("03", "c2"),
                ("03", "c3"),
                ("02", "b"),
                ("01", "a"),
            ]
            .iter()
            .map(|(s, m)| (format!("2026-10-22T08:00:{s}Z"), m))
            .filter(|(t, _)| t.parse::<jiff::Timestamp>().unwrap() <= end)
            .take(limit)
            .map(|(t, m)| json!({"_time": t, "_msg": m}).to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };
        let url = crate::testkit::serve(
            Router::new().route("/api/datasources/proxy/uid/vl/select/logsql/query", get(vl)),
        )
        .await;
        let cfg = crate::config::Grafana {
            url,
            ui_url: None,
            token: crate::secret::Secret::Plain("t".into()),
            tls_verify: true,
            ca_file: None,
            tls_server_name: None,
            quiet: false,
            ignore_alerts: Vec::new(),
        };
        Grafana::new("lab", &cfg).await.unwrap()
    }

    async fn export(g: &Grafana, size: usize) -> Vec<String> {
        let src = source(LogKind::Victorialogs, "vl");
        let req = request("*");
        let start: jiff::Timestamp = "2026-10-22T07:00:00Z".parse().unwrap();
        let mut cursor = Cursor::new("2026-10-22T09:00:00Z".parse().unwrap());
        let mut out = Vec::new();
        while let Some(rows) = cursor.next(g, &src, &req, size, start).await.unwrap() {
            out.extend(rows.into_iter().map(|r| r.message));
        }
        out
    }

    #[tokio::test]
    async fn export_pages_through_the_range() {
        let g = paged().await;
        let all = ["e", "d", "c1", "c2", "c3", "b", "a"];
        // pages end on a shared timestamp: no line twice, none lost
        assert_eq!(export(&g, 3).await, all);
        assert_eq!(export(&g, 100).await, all);
        // more lines on one timestamp than a page holds: the rest is
        // skipped, and the export still ends
        assert_eq!(export(&g, 2).await, ["e", "d", "c1", "c2", "b", "a"]);
    }

    #[tokio::test]
    async fn search_loki() {
        let g = grafana().await;
        let r = run(
            &g,
            &source(LogKind::Loki, "lk"),
            &request("{unit=~\".+\"}"),
            2,
        )
        .await
        .unwrap();
        assert!(!r.truncated);
        assert_eq!(
            r.rows[0],
            Row {
                time: "2026-10-22T08:01:00Z".into(),
                message: "new".into(),
                labels: "unit=y".into(),
            }
        );
        assert_eq!(r.rows[1].message, "old");
        let e = run(&g, &source(LogKind::Loki, "lk-bad"), &request("x"), 2)
            .await
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "query failed: parse error at line 1");
        let e = run(&g, &source(LogKind::Loki, "other"), &request("x"), 2)
            .await
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "query failed: unknown error");
    }
}

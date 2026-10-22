//! Security presets. Each one is a named query against an integration Homie
//! already has, rendered as one table: columns, rows, a total and a link.

use std::collections::BTreeMap;

use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ListParams},
    core::GroupVersionKind,
};
use serde::Serialize;
use serde_json::{Value, json};

use super::grafana::Grafana;
use crate::{Result, config};

#[derive(Serialize)]
pub struct Table {
    pub columns: Vec<&'static str>,
    pub rows: Vec<Vec<Value>>,
    pub total: f64,
    pub total_label: &'static str,
    pub link: Option<String>,
}

/// Kyverno policy reports: fail and warn counts per policy and namespace.
/// The reports API group is whichever one the cluster serves.
pub async fn kyverno(client: Client) -> Result<Table> {
    let mut counts: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    let mut found = false;
    for (group, version) in [
        ("wgpolicyk8s.io", "v1alpha2"),
        ("openreports.io", "v1alpha1"),
    ] {
        for (kind, namespaced) in [("PolicyReport", true), ("ClusterPolicyReport", false)] {
            let kind = if group == "openreports.io" {
                kind.replace("Policy", "")
            } else {
                kind.to_string()
            };
            let ar = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, &kind));
            let reports = match Api::<DynamicObject>::all_with(client.clone(), &ar)
                .list(&ListParams::default())
                .await
            {
                Ok(l) => l,
                Err(kube::Error::Api(s)) if s.code == 404 => continue,
                Err(e) => return Err(e.into()),
            };
            found = true;
            for r in reports {
                let ns = if namespaced {
                    r.metadata.namespace.clone().unwrap_or_default()
                } else {
                    "(cluster)".into()
                };
                for res in r.data["results"].as_array().into_iter().flatten() {
                    let policy = res["policy"].as_str().unwrap_or_default().to_string();
                    let entry = counts.entry((policy, ns.clone())).or_default();
                    match res["result"].as_str() {
                        Some("fail" | "error") => entry.0 += 1,
                        Some("warn") => entry.1 += 1,
                        _ => {}
                    }
                }
            }
        }
        if found {
            break;
        }
    }
    if !found {
        return Err("no PolicyReport CRD found (wgpolicyk8s.io or openreports.io)".into());
    }
    let mut rows: Vec<(String, String, u64, u64)> = counts
        .into_iter()
        .filter(|(_, (f, w))| f + w > 0)
        .map(|((p, n), (f, w))| (p, n, f, w))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse((r.2, r.3)));
    Ok(Table {
        columns: vec!["policy", "namespace", "fail", "warn"],
        total: rows.iter().map(|r| r.2 as f64).sum(),
        total_label: "failing checks",
        rows: rows
            .into_iter()
            .map(|(p, n, f, w)| vec![json!(p), json!(n), json!(f), json!(w)])
            .collect(),
        link: None,
    })
}

fn log_table(
    rows: Vec<serde_json::Map<String, Value>>,
    columns: Vec<&'static str>,
    count: &'static str,
) -> (Vec<Vec<Value>>, f64) {
    let num = |v: Option<&Value>| {
        v.and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
            .unwrap_or(0.0)
    };
    let total = rows.iter().map(|r| num(r.get(count))).sum();
    let rows = rows
        .iter()
        .map(|r| {
            columns
                .iter()
                .map(|c| {
                    if *c == count {
                        json!(num(r.get(*c)))
                    } else {
                        r.get(*c).cloned().unwrap_or(Value::Null)
                    }
                })
                .collect()
        })
        .collect();
    (rows, total)
}

/// Tetragon kprobe policy hits (including Sigkill enforcement), last 24 h.
pub async fn tetragon(g: &Grafana, p: &config::LogPreset) -> Result<Table> {
    let q = format!(
        r#"_time:24h {} process_kprobe | extract "\"policy_name\":\"<policy>\"" | extract "\"namespace\":\"<namespace>\"" | extract "\"binary\":\"<binary>\"" | extract "\"action\":\"<action>\"" | stats by (policy, namespace, binary, action) count() events | sort by (events desc) | limit 100"#,
        p.filter
    );
    let columns = vec!["policy", "namespace", "binary", "action", "events"];
    let (rows, total) = log_table(
        g.logsql(&p.datasource, &q).await?,
        columns.clone(),
        "events",
    );
    Ok(Table {
        columns,
        rows,
        total,
        total_label: "events",
        link: Some(g.explore(&p.datasource, &q)),
    })
}

/// Wazuh alerts at or above `min_level`, last 24 h.
pub async fn wazuh(g: &Grafana, p: &config::WazuhPreset) -> Result<Table> {
    let q = format!(
        "_time:24h {} | unpack_json fields (rule.level, rule.description, agent.name) | filter rule.level:>={} | stats by (agent.name, rule.level, rule.description) count() alerts | sort by (alerts desc) | limit 100",
        p.filter, p.min_level
    );
    let columns = vec!["agent.name", "rule.level", "rule.description", "alerts"];
    let (rows, total) = log_table(
        g.logsql(&p.datasource, &q).await?,
        columns.clone(),
        "alerts",
    );
    Ok(Table {
        columns,
        rows,
        total,
        total_label: "alerts",
        link: Some(g.explore(&p.datasource, &q)),
    })
}

/// debsecan CVE counts per host and the age of the last scan.
pub async fn debsecan(g: &Grafana, p: &config::MetricPreset) -> Result<Table> {
    let counts_q = "sum by (instance, fixed, urgency) (debsecan_vulnerabilities)";
    let age_q = "max by (instance) (time() - debsecan_last_run_timestamp_seconds)";
    let (counts, ages) = tokio::try_join!(
        g.promql(&p.datasource, counts_q),
        g.promql(&p.datasource, age_q)
    )?;

    #[derive(Default)]
    struct Host {
        fixable: f64,
        fixable_high: f64,
        unfixed: f64,
        age_hours: Option<f64>,
    }
    let label = |s: &super::grafana::Sample, k: &str| {
        s.labels
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut hosts: BTreeMap<String, Host> = BTreeMap::new();
    for s in &counts {
        let h = hosts.entry(label(s, "instance")).or_default();
        if label(s, "fixed") == "true" {
            h.fixable += s.value;
            if label(s, "urgency") == "high" {
                h.fixable_high += s.value;
            }
        } else {
            h.unfixed += s.value;
        }
    }
    for s in &ages {
        hosts.entry(label(s, "instance")).or_default().age_hours = Some((s.value / 3600.0).round());
    }
    Ok(Table {
        columns: vec![
            "instance",
            "fixable",
            "fixable_high",
            "unfixed",
            "last_scan_hours_ago",
        ],
        total: hosts.values().map(|h| h.fixable).sum(),
        total_label: "fixable CVEs",
        rows: hosts
            .into_iter()
            .map(|(i, h)| {
                vec![
                    json!(i),
                    json!(h.fixable),
                    json!(h.fixable_high),
                    json!(h.unfixed),
                    json!(h.age_hours),
                ]
            })
            .collect(),
        link: Some(g.explore(&p.datasource, counts_q)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{json as route, kube_client, list, serve};
    use axum::{
        Router,
        extract::{Path, Query},
        http::StatusCode,
        response::IntoResponse,
        routing::get,
    };
    use std::collections::HashMap;

    fn report(ns: Option<&str>, results: Value) -> Value {
        json!({"metadata": {"name": "r", "namespace": ns}, "results": results})
    }

    fn results(policy: &str, outcomes: &[&str]) -> Vec<Value> {
        outcomes
            .iter()
            .map(|r| json!({"policy": policy, "result": r}))
            .collect()
    }

    #[tokio::test]
    async fn kyverno_counts_failures_per_policy_and_namespace() {
        let mut web = results("require-limits", &["fail", "fail", "pass", "warn"]);
        web.extend(results("disallow-latest", &["error", "skip"]));
        web.extend(results("all-good", &["pass"]));
        let app = route(
            "/apis/wgpolicyk8s.io/v1alpha2/policyreports",
            list(
                "PolicyReport",
                json!([
                    report(Some("web"), json!(web)),
                    report(Some("db"), json!(results("require-limits", &["warn"]))),
                    report(Some("empty"), json!(null)),
                ]),
            ),
        )
        .merge(route(
            "/apis/wgpolicyk8s.io/v1alpha2/clusterpolicyreports",
            list(
                "ClusterPolicyReport",
                json!([report(None, json!(results("require-labels", &["fail"])))]),
            ),
        ));
        let t = kyverno(kube_client(&serve(app).await)).await.unwrap();
        assert_eq!(t.columns, ["policy", "namespace", "fail", "warn"]);
        assert_eq!(
            t.rows,
            [
                vec![json!("require-limits"), json!("web"), json!(2), json!(1)],
                vec![json!("disallow-latest"), json!("web"), json!(1), json!(0)],
                vec![
                    json!("require-labels"),
                    json!("(cluster)"),
                    json!(1),
                    json!(0)
                ],
                vec![json!("require-limits"), json!("db"), json!(0), json!(1)],
            ]
        );
        assert_eq!(t.total, 4.0);
        assert_eq!(t.total_label, "failing checks");
    }

    #[tokio::test]
    async fn kyverno_falls_back_to_openreports() {
        let app = route(
            "/apis/openreports.io/v1alpha1/reports",
            list(
                "Report",
                json!([report(Some("web"), json!(results("p", &["fail"])))]),
            ),
        );
        let t = kyverno(kube_client(&serve(app).await)).await.unwrap();
        assert_eq!(t.rows, [vec![json!("p"), json!("web"), json!(1), json!(0)]]);
    }

    #[tokio::test]
    async fn kyverno_errors() {
        let e = kyverno(kube_client(&serve(Router::new()).await))
            .await
            .err()
            .unwrap();
        assert!(e.to_string().starts_with("no PolicyReport CRD found"));
        let forbidden = Router::new().route(
            "/apis/wgpolicyk8s.io/v1alpha2/policyreports",
            get(|| async { StatusCode::FORBIDDEN }),
        );
        assert!(kyverno(kube_client(&serve(forbidden).await)).await.is_err());
    }

    /// A Grafana whose datasource proxy answers LogsQL with `logs` (NDJSON)
    /// and PromQL with `prom(query)`.
    async fn grafana(logs: &'static str, prom: fn(&str) -> Value) -> Grafana {
        let app = Router::new()
            .route(
                "/api/datasources/proxy/uid/{uid}/select/logsql/query",
                get(move |Path(uid): Path<String>| async move {
                    if uid == "down" {
                        StatusCode::BAD_GATEWAY.into_response()
                    } else {
                        logs.into_response()
                    }
                }),
            )
            .route(
                "/api/datasources/proxy/uid/{uid}/api/v1/query",
                get(
                    move |Path(uid): Path<String>, Query(q): Query<HashMap<String, String>>| async move {
                        if uid == "down" {
                            StatusCode::BAD_GATEWAY.into_response()
                        } else {
                            axum::Json(prom(&q["query"])).into_response()
                        }
                    },
                ),
            );
        let url = serve(app).await;
        let cfg: config::Grafana =
            serde_json::from_value(json!({"url": url, "token": "viewer-token"})).unwrap();
        Grafana::new("lab", &cfg).await.unwrap()
    }

    #[tokio::test]
    async fn tetragon_and_wazuh_tables() {
        let g = grafana(
            "{\"policy\":\"block-nc\",\"namespace\":\"web\",\"binary\":\"/bin/nc\",\"action\":\"Sigkill\",\"events\":\"4\",\"alerts\":\"4\",\"agent.name\":\"host-a\"}\n\n{\"policy\":\"p2\",\"events\":\"1\",\"alerts\":\"2\"}\n",
            debsecan_prom,
        )
        .await;
        let preset = |ds: &str| config::LogPreset {
            grafana: "lab".into(),
            datasource: ds.into(),
            filter: "k8s.container.name:export-stdout".into(),
        };
        let t = tetragon(&g, &preset("vlogs")).await.unwrap();
        assert_eq!(t.total, 5.0);
        assert_eq!(
            t.rows[0],
            [
                json!("block-nc"),
                json!("web"),
                json!("/bin/nc"),
                json!("Sigkill"),
                json!(4.0)
            ]
        );
        assert!(t.link.unwrap().contains("/explore?"));
        assert!(tetragon(&g, &preset("down")).await.is_err());

        let wazuh_preset = |ds: &str| config::WazuhPreset {
            grafana: "lab".into(),
            datasource: ds.into(),
            filter: "k8s.namespace.name:wazuh".into(),
            min_level: 10,
        };
        let t = wazuh(&g, &wazuh_preset("vlogs")).await.unwrap();
        assert_eq!((t.total, t.total_label), (6.0, "alerts"));
        assert_eq!(
            t.rows[1],
            [Value::Null, Value::Null, Value::Null, json!(2.0)]
        );
        assert!(wazuh(&g, &wazuh_preset("down")).await.is_err());
    }

    fn debsecan_prom(query: &str) -> Value {
        let sample = |labels: Value, v: &str| json!({"metric": labels, "value": [0, v]});
        let result = if query.contains("debsecan_vulnerabilities") {
            json!([
                sample(
                    json!({"instance": "host-a", "fixed": "true", "urgency": "high"}),
                    "3"
                ),
                sample(
                    json!({"instance": "host-a", "fixed": "true", "urgency": "low"}),
                    "5"
                ),
                sample(
                    json!({"instance": "host-a", "fixed": "false", "urgency": "low"}),
                    "7"
                ),
                sample(
                    json!({"instance": "host-b", "fixed": "true", "urgency": "medium"}),
                    "1"
                ),
            ])
        } else {
            json!([
                sample(json!({"instance": "host-a"}), "7200"),
                sample(json!({"instance": "host-c"}), "90000"),
            ])
        };
        json!({"status": "success", "data": {"result": result}})
    }

    #[tokio::test]
    async fn debsecan_table() {
        let g = grafana("", debsecan_prom).await;
        let preset = |ds: &str| config::MetricPreset {
            grafana: "lab".into(),
            datasource: ds.into(),
        };
        let t = debsecan(&g, &preset("vm")).await.unwrap();
        assert_eq!((t.total, t.total_label), (9.0, "fixable CVEs"));
        assert_eq!(
            t.rows,
            [
                vec![
                    json!("host-a"),
                    json!(8.0),
                    json!(3.0),
                    json!(7.0),
                    json!(2.0)
                ],
                vec![
                    json!("host-b"),
                    json!(1.0),
                    json!(0.0),
                    json!(0.0),
                    Value::Null
                ],
                vec![
                    json!("host-c"),
                    json!(0.0),
                    json!(0.0),
                    json!(0.0),
                    json!(25.0)
                ],
            ]
        );
        assert!(t.link.is_some());
        assert!(debsecan(&g, &preset("down")).await.is_err());
    }

    #[test]
    fn log_rows_become_table_rows() {
        let rows = vec![
            serde_json::from_value(json!({"policy": "p1", "events": "3"})).unwrap(),
            serde_json::from_value(json!({"policy": "p2", "events": 2})).unwrap(),
        ];
        let (rows, total) = log_table(rows, vec!["policy", "namespace", "events"], "events");
        assert_eq!(total, 5.0);
        assert_eq!(rows[0], vec![json!("p1"), Value::Null, json!(3.0)]);
    }
}

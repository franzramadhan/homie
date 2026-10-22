//! Sparklines (RFC 0012): the last hour of CPU and memory as short series.
//! Proxmox from its own RRD data, Kubernetes nodes from a configured
//! Prometheus-compatible datasource.

use std::collections::BTreeMap;

use futures::future::join_all;
use serde::Serialize;
use serde_json::Value;

use super::{grafana::Grafana, proxmox::Proxmox};
use crate::{Result, config::NodeMetrics};

pub const POINTS: usize = 30;

#[derive(Serialize, Default)]
pub struct Series {
    cpu: Vec<f64>,
    mem: Vec<f64>,
}

#[derive(Serialize, Default)]
pub struct PveSparks {
    nodes: BTreeMap<String, Series>,
    guests: BTreeMap<String, Series>,
}

#[derive(Serialize, Default)]
pub struct NodeSparks {
    nodes: BTreeMap<String, Series>,
}

/// Averages `values` into at most `n` buckets; gaps (None) are skipped.
pub fn downsample(values: &[Option<f64>], n: usize) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let size = values.len().div_ceil(n);
    values
        .chunks(size)
        .filter_map(|c| {
            let v: Vec<f64> = c
                .iter()
                .flatten()
                .copied()
                .filter(|x| x.is_finite())
                .collect();
            (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
        })
        .collect()
}

fn ratio(row: &Value, num: &str, den: &str) -> Option<f64> {
    let (n, d) = (row[num].as_f64()?, row[den].as_f64()?);
    (d > 0.0).then_some(n / d)
}

/// RRD rows to CPU (0..1) and memory (0..1) series.
fn rrd_series(rows: &Value, mem: &str, mem_max: &str) -> Series {
    let rows = rows.as_array().cloned().unwrap_or_default();
    let cpu: Vec<Option<f64>> = rows.iter().map(|r| r["cpu"].as_f64()).collect();
    let m: Vec<Option<f64>> = rows.iter().map(|r| ratio(r, mem, mem_max)).collect();
    Series {
        cpu: downsample(&cpu, POINTS),
        mem: downsample(&m, POINTS),
    }
}

pub async fn proxmox(pve: &Proxmox) -> Result<PveSparks> {
    let resources = pve.get("/cluster/resources").await?;
    let rows = resources.as_array().cloned().unwrap_or_default();
    let text = |r: &Value, k: &str| r[k].as_str().unwrap_or_default().to_string();

    let nodes = join_all(
        rows.iter()
            .filter(|r| text(r, "type") == "node" && text(r, "status") == "online")
            .map(|r| async move {
                let node = text(r, "node");
                let data = pve
                    .get(&format!("/nodes/{node}/rrddata?timeframe=hour&cf=AVERAGE"))
                    .await
                    .ok()?;
                Some((node, rrd_series(&data, "memused", "memtotal")))
            }),
    );
    let guests = join_all(
        rows.iter()
            .filter(|r| {
                matches!(text(r, "type").as_str(), "qemu" | "lxc") && text(r, "status") == "running"
            })
            .map(|r| async move {
                let (kind, node, vmid) = (text(r, "type"), text(r, "node"), r["vmid"].as_u64()?);
                let data = pve
                    .get(&format!(
                        "/nodes/{node}/{kind}/{vmid}/rrddata?timeframe=hour&cf=AVERAGE"
                    ))
                    .await
                    .ok()?;
                Some((vmid.to_string(), rrd_series(&data, "mem", "maxmem")))
            }),
    );
    let (nodes, guests) = tokio::join!(nodes, guests);
    Ok(PveSparks {
        nodes: nodes.into_iter().flatten().collect(),
        guests: guests.into_iter().flatten().collect(),
    })
}

pub async fn kubernetes(g: &Grafana, m: &NodeMetrics) -> Result<NodeSparks> {
    let (cpu, mem) = tokio::try_join!(
        g.promql_range(&m.datasource, &m.cpu, 3600, 120),
        g.promql_range(&m.datasource, &m.memory, 3600, 120),
    )?;
    let mut out = NodeSparks::default();
    for (series, is_cpu) in [(cpu, true), (mem, false)] {
        for (labels, values) in series {
            let Some(node) = labels.get(&m.node_label).and_then(Value::as_str) else {
                continue;
            };
            let v: Vec<Option<f64>> = values.into_iter().map(Some).collect();
            let entry = out.nodes.entry(node.to_string()).or_default();
            let points = downsample(&v, POINTS);
            if is_cpu {
                entry.cpu = points
            } else {
                entry.mem = points
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsampling() {
        let v: Vec<Option<f64>> = (0..60).map(|i| Some(i as f64)).collect();
        let d = downsample(&v, 30);
        assert_eq!(d.len(), 30);
        assert_eq!(d[0], 0.5);
        assert_eq!(
            downsample(&[None, Some(2.0), Some(f64::NAN), Some(4.0)], 2),
            vec![2.0, 4.0]
        );
        assert!(downsample(&[], 30).is_empty());
        let rows = serde_json::json!([{"cpu": 0.5, "memused": 2.0, "memtotal": 4.0}, {"time": 1}]);
        let s = rrd_series(&rows, "memused", "memtotal");
        assert_eq!((s.cpu, s.mem), (vec![0.5], vec![0.5]));
    }

    use crate::testkit::{json, serve};
    use axum::{Json, Router, extract::Query, http::StatusCode, routing::get};
    use serde_json::json;
    use std::collections::HashMap;

    async fn pve(url: &str) -> Proxmox {
        let cfg =
            serde_json::from_value(json!({"endpoints": [url], "token": "u@pve!t=x"})).unwrap();
        Proxmox::new("lab", &cfg).await.unwrap()
    }

    #[tokio::test]
    async fn proxmox_rrd_for_online_nodes_and_running_guests() {
        let rrd = |cpu: f64| json!({"data": [{"cpu": cpu, "memused": 1.0, "memtotal": 4.0, "mem": 3.0, "maxmem": 4.0}]});
        let app = json(
            "/api2/json/cluster/resources",
            json!({"data": [
                {"type": "node", "node": "pve-01", "status": "online"},
                {"type": "node", "node": "pve-02", "status": "offline"},
                {"type": "node", "node": "pve-03", "status": "online"},
                {"type": "qemu", "node": "pve-01", "vmid": 101, "status": "running"},
                {"type": "lxc", "node": "pve-01", "vmid": 102, "status": "stopped"},
                {"type": "lxc", "node": "pve-01", "status": "running"},
                {"type": "storage", "node": "pve-01", "status": "available"},
            ]}),
        )
        .merge(json("/api2/json/nodes/pve-01/rrddata", rrd(0.25)))
        .merge(json("/api2/json/nodes/pve-01/qemu/101/rrddata", rrd(0.75)));
        // pve-03 has no RRD route: a 404 drops it instead of failing the rest
        let p = pve(&serve(app).await).await;
        let s = proxmox(&p).await.unwrap();
        assert_eq!(s.nodes.keys().collect::<Vec<_>>(), ["pve-01"]);
        assert_eq!(s.nodes["pve-01"].cpu, vec![0.25]);
        assert_eq!(s.nodes["pve-01"].mem, vec![0.25]);
        assert_eq!(s.guests.keys().collect::<Vec<_>>(), ["101"]);
        assert_eq!(s.guests["101"].cpu, vec![0.75]);
        assert_eq!(s.guests["101"].mem, vec![0.75]);
    }

    #[tokio::test]
    async fn proxmox_error_when_resources_fail() {
        let app = Router::new().route(
            "/api2/json/cluster/resources",
            get(|| async { StatusCode::FORBIDDEN }),
        );
        let p = pve(&serve(app).await).await;
        assert!(proxmox(&p).await.err().unwrap().to_string().contains("403"));
    }

    async fn grafana(app: Router) -> Grafana {
        let url = serve(app).await;
        let cfg = serde_json::from_value(json!({"url": url, "token": "t"})).unwrap();
        Grafana::new("g", &cfg).await.unwrap()
    }

    fn metrics() -> NodeMetrics {
        serde_json::from_value(
            json!({"grafana": "g", "datasource": "vm", "cpu": "cpu_q", "memory": "mem_q"}),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn kubernetes_series_by_node_label() {
        let app = Router::new().route(
            "/api/datasources/proxy/uid/vm/api/v1/query_range",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                let v = if q["query"] == "cpu_q" { "0.5" } else { "0.25" };
                Json(json!({"status": "success", "data": {"result": [
                    {"metric": {"node": "k1"}, "values": [[1, v], [2, v]]},
                    {"metric": {"instance": "x"}, "values": [[1, "1"]]},
                ]}}))
            }),
        );
        let g = grafana(app).await;
        let s = kubernetes(&g, &metrics()).await.unwrap();
        assert_eq!(s.nodes.len(), 1);
        assert_eq!(s.nodes["k1"].cpu, vec![0.5, 0.5]);
        assert_eq!(s.nodes["k1"].mem, vec![0.25, 0.25]);
    }

    #[tokio::test]
    async fn kubernetes_query_error() {
        let g = grafana(json(
            "/api/datasources/proxy/uid/vm/api/v1/query_range",
            json!({"status": "error", "error": "bad query"}),
        ))
        .await;
        let err = kubernetes(&g, &metrics()).await.err().unwrap();
        assert!(err.to_string().contains("bad query"), "{err}");
    }
}

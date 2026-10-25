//! Disk-full forecast (RFC 0025): a least-squares line over the last week
//! of usage says when a storage fills up. Proxmox storages from their own
//! RRD data, Kubernetes volumes from a configured Prometheus-compatible
//! datasource.

use std::collections::{BTreeMap, HashSet};

use futures::future::join_all;
use serde::Serialize;
use serde_json::Value;

use super::{
    grafana::Grafana,
    proxmox::{self, Proxmox},
    sparks,
};
use crate::{
    Result,
    config::{Forecast, NodeMetrics},
    status::Tone,
};

const WEEK: f64 = 7.0 * 86_400.0;
/// fewer points than a day of hours say nothing about next week
const MIN_POINTS: usize = 24;
/// below this, usage jumps around more than it follows a line
const MIN_R2: f64 = 0.5;

#[derive(Serialize, Default)]
pub struct Storages {
    storages: Vec<Storage>,
}

#[derive(Serialize)]
struct Storage {
    storage: String,
    node: String,
    shared: bool,
    used: f64,
    total: f64,
    /// seconds until full; none when usage is flat, shrinking or noisy
    full_in: Option<f64>,
    tone: Tone,
}

#[derive(Serialize, Default)]
pub struct Volumes {
    volumes: Vec<Volume>,
}

#[derive(Serialize)]
struct Volume {
    namespace: String,
    claim: String,
    used: f64,
    capacity: f64,
    full_in: Option<f64>,
    tone: Tone,
    /// used / capacity over the week
    spark: Vec<f64>,
}

/// Seconds until `capacity` is reached, from a line fitted to the last week
/// of `(time, used)` points. `None` for a flat, shrinking or noisy series
/// and for fewer than `MIN_POINTS` points.
pub fn full_in(points: &[(f64, f64)], capacity: f64) -> Option<f64> {
    let &(end, now) = points.last()?;
    let week: Vec<(f64, f64)> = points
        .iter()
        .filter(|(t, u)| t.is_finite() && u.is_finite() && *t > end - WEEK)
        .copied()
        .collect();
    if week.len() < MIN_POINTS {
        return None;
    }
    let n = week.len() as f64;
    let (mt, mu) = week
        .iter()
        .fold((0.0, 0.0), |(a, b), (t, u)| (a + t / n, b + u / n));
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (t, u) in &week {
        let (dt, du) = (t - mt, u - mu);
        sxx += dt * dt;
        sxy += dt * du;
        syy += du * du;
    }
    if sxx == 0.0 || syy == 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let r2 = sxy * sxy / (sxx * syy);
    if slope <= 0.0 || r2 < MIN_R2 {
        return None;
    }
    Some(((capacity - now) / slope).max(0.0))
}

pub fn tone(full_in: Option<f64>, f: &Forecast) -> Tone {
    match full_in.map(|s| s / 86_400.0) {
        Some(d) if d <= f.bad_days as f64 => Tone::Bad,
        Some(d) if d <= f.warn_days as f64 => Tone::Warn,
        _ => Tone::Ok,
    }
}

/// "full in 9 days", as the page says it.
pub fn words(seconds: f64) -> String {
    match (seconds / 86_400.0).floor() as u64 {
        0 => "full within a day".into(),
        1 => "full in 1 day".into(),
        d => format!("full in {d} days"),
    }
}

/// A storage row's name: `storage@node`, `storage` when shared, or
/// `namespace/claim` for a volume.
pub fn label(r: &Value) -> String {
    let t = |k: &str| r[k].as_str().unwrap_or_default();
    if r["claim"].is_string() {
        format!("{}/{}", t("namespace"), t("claim"))
    } else if r["shared"] == true {
        t("storage").to_string()
    } else {
        format!("{}@{}", t("storage"), t("node"))
    }
}

pub async fn proxmox(pve: &Proxmox, f: &Forecast) -> Result<Storages> {
    let resources = pve.get("/cluster/resources?type=storage").await?;
    let text = |r: &Value, k: &str| r[k].as_str().unwrap_or_default().to_string();
    let shared = |r: &Value| r["shared"] == 1 || r["shared"] == true;
    // a shared storage is the same on every node: read it once
    let mut seen = HashSet::new();
    let rows: Vec<&Value> = resources
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| text(r, "type") == "storage" && text(r, "status") == "available")
        .filter(|r| proxmox::safe(&text(r, "storage")) && proxmox::safe(&text(r, "node")))
        .filter(|r| !shared(r) || seen.insert(text(r, "storage")))
        .collect();
    let storages = join_all(rows.into_iter().map(|r| async move {
        let (storage, node) = (text(r, "storage"), text(r, "node"));
        let data = pve
            .get(&format!(
                "/nodes/{node}/storage/{storage}/rrddata?timeframe=week&cf=AVERAGE"
            ))
            .await
            .ok()?;
        let rows = data.as_array().cloned().unwrap_or_default();
        let points: Vec<(f64, f64)> = rows
            .iter()
            .filter_map(|p| Some((p["time"].as_f64()?, p["used"].as_f64()?)))
            .collect();
        let total = rows
            .iter()
            .rev()
            .find_map(|p| p["total"].as_f64())
            .or_else(|| r["maxdisk"].as_f64())?;
        let full_in = full_in(&points, total);
        Some(Storage {
            used: points
                .last()
                .map(|p| p.1)
                .or_else(|| r["disk"].as_f64())
                .unwrap_or_default(),
            total,
            shared: shared(r),
            tone: tone(full_in, f),
            full_in,
            storage,
            node,
        })
    }))
    .await;
    let mut storages: Vec<Storage> = storages.into_iter().flatten().collect();
    storages.sort_by(|a, b| (&a.storage, &a.node).cmp(&(&b.storage, &b.node)));
    Ok(Storages { storages })
}

pub async fn kubernetes(g: &Grafana, m: &NodeMetrics, f: &Forecast) -> Result<Volumes> {
    let (Some(used_q), Some(cap_q)) = (&m.volume_used, &m.volume_capacity) else {
        return Err("no volume queries configured".into());
    };
    let (used, capacity) = tokio::try_join!(
        g.promql_points(&m.datasource, used_q, WEEK as i64, 3600),
        g.promql(&m.datasource, cap_q),
    )?;
    let key = |l: &serde_json::Map<String, Value>| {
        let get = |k: &str| Some(l.get(k)?.as_str()?.to_string());
        Some((get(&m.namespace_label)?, get(&m.volume_label)?))
    };
    let capacities: BTreeMap<(String, String), f64> = capacity
        .iter()
        .filter(|s| s.value.is_finite() && s.value > 0.0)
        .filter_map(|s| Some((key(&s.labels)?, s.value)))
        .collect();
    let mut volumes: Vec<Volume> = used
        .into_iter()
        .filter_map(|(labels, points)| {
            let (namespace, claim) = key(&labels)?;
            let capacity = *capacities.get(&(namespace.clone(), claim.clone()))?;
            let &(_, used) = points.last()?;
            let full_in = if f.enabled {
                full_in(&points, capacity)
            } else {
                None
            };
            let ratio: Vec<Option<f64>> = points.iter().map(|(_, u)| Some(u / capacity)).collect();
            Some(Volume {
                namespace,
                claim,
                used,
                capacity,
                full_in,
                tone: tone(full_in, f),
                spark: sparks::downsample(&ratio, sparks::POINTS),
            })
        })
        .collect();
    volumes.sort_by(|a, b| (&a.namespace, &a.claim).cmp(&(&b.namespace, &b.claim)));
    Ok(Volumes { volumes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: f64 = 3600.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    /// A week of hourly points, `used(hour)`.
    fn week(used: impl Fn(f64) -> f64) -> Vec<(f64, f64)> {
        (0..168)
            .map(|h| (h as f64 * HOUR, used(h as f64)))
            .collect()
    }

    fn days(s: Option<f64>) -> Option<f64> {
        s.map(|s| (s / 86_400.0 * 10.0).round() / 10.0)
    }

    #[test]
    fn growing_flat_shrinking() {
        // 1 GiB a day from 50 GiB: 50 GiB left at the end of the week
        let growing = week(|h| 50.0 * GIB + h / 24.0 * GIB);
        let now = growing.last().unwrap().1;
        assert_eq!(days(full_in(&growing, now + 10.0 * GIB)), Some(10.0));
        // already past capacity: full now, not negative
        assert_eq!(full_in(&growing, 10.0 * GIB), Some(0.0));
        assert_eq!(full_in(&week(|_| 50.0 * GIB), 100.0 * GIB), None);
        assert_eq!(full_in(&week(|h| 90.0 * GIB - h * 1e6), 100.0 * GIB), None);
    }

    #[test]
    fn noisy_and_short_series_say_nothing() {
        // a backup job filling and emptying the store every other hour
        let noisy = week(|h| if (h as u64).is_multiple_of(2) { 10.0 } else { 90.0 } + h * 0.001);
        assert_eq!(full_in(&noisy, 100.0), None);
        // a daily job on a steady climb still fits
        let daily = week(|h| {
            h + if (h as u64).is_multiple_of(24) {
                5.0
            } else {
                0.0
            }
        });
        assert!(full_in(&daily, 1000.0).is_some());
        let day = week(|h| h);
        assert_eq!(full_in(&day[..23], 1000.0), None);
        assert!(full_in(&day[..24], 1000.0).is_some());
        assert_eq!(full_in(&[], 1.0), None);
        // only the last week counts: an old jump does not
        let mut old = vec![(-30.0 * 86_400.0, 0.0)];
        old.extend(week(|_| 50.0));
        assert_eq!(full_in(&old, 100.0), None);
        // gaps and NaN are skipped
        let mut gaps = week(|h| h);
        gaps[5].1 = f64::NAN;
        assert!(full_in(&gaps, 1000.0).is_some());
    }

    #[test]
    fn tones_and_words() {
        let f = Forecast::default();
        let d = |n: f64| Some(n * 86_400.0);
        assert_eq!(tone(None, &f), Tone::Ok);
        assert_eq!(tone(d(15.0), &f), Tone::Ok);
        assert_eq!(tone(d(14.0), &f), Tone::Warn);
        assert_eq!(tone(d(3.5), &f), Tone::Warn);
        assert_eq!(tone(d(3.0), &f), Tone::Bad);
        assert_eq!(tone(d(0.0), &f), Tone::Bad);
        assert_eq!(words(3600.0), "full within a day");
        assert_eq!(words(86_400.0 * 1.5), "full in 1 day");
        assert_eq!(words(86_400.0 * 9.9), "full in 9 days");
        assert_eq!(
            label(&json!({"storage": "local", "node": "n1"})),
            "local@n1"
        );
        assert_eq!(
            label(&json!({"storage": "nfs", "node": "n1", "shared": true})),
            "nfs"
        );
        assert_eq!(
            label(&json!({"namespace": "db", "claim": "data"})),
            "db/data"
        );
    }

    use crate::testkit::{json, serve};
    use axum::{Json, Router, extract::Query, routing::get};
    use serde_json::json;
    use std::collections::HashMap;

    fn rrd(start: f64, step: f64) -> Value {
        let rows: Vec<Value> = (0..168)
            .map(|h| json!({"time": h as f64 * HOUR, "used": start + h as f64 * step, "total": 1000.0}))
            .collect();
        json!({ "data": rows })
    }

    #[tokio::test]
    async fn proxmox_storages_from_rrd() {
        let app = json(
            "/api2/json/cluster/resources",
            json!({"data": [
                {"type": "storage", "storage": "local", "node": "pve-01", "status": "available", "shared": 0, "disk": 1, "maxdisk": 1000},
                {"type": "storage", "storage": "nfs", "node": "pve-01", "status": "available", "shared": 1},
                {"type": "storage", "storage": "nfs", "node": "pve-02", "status": "available", "shared": 1},
                {"type": "storage", "storage": "flat", "node": "pve-01", "status": "available", "shared": 0},
                {"type": "storage", "storage": "gone", "node": "pve-02", "status": "unknown"},
                {"type": "storage", "storage": "a b", "node": "pve-01", "status": "available"},
                {"type": "storage", "storage": "norrd", "node": "pve-01", "status": "available"},
                {"type": "storage", "storage": "empty", "node": "pve-01", "status": "available", "disk": 7, "maxdisk": 100},
            ]}),
        )
        .merge(json(
            "/api2/json/nodes/pve-01/storage/local/rrddata",
            // 1 an hour: 1000 - 167 = 833 left, about 34.7 days
            rrd(0.0, 1.0),
        ))
        .merge(json(
            "/api2/json/nodes/pve-01/storage/nfs/rrddata",
            // 4 a hour from 300: 1000 - 968 = 32 left, 8 hours
            rrd(300.0, 4.0),
        ))
        .merge(json("/api2/json/nodes/pve-01/storage/flat/rrddata", rrd(10.0, 0.0)))
        .merge(json(
            "/api2/json/nodes/pve-01/storage/empty/rrddata",
            json!({"data": [{"time": 1}]}),
        ));
        let cfg =
            serde_json::from_value(json!({"endpoints": [serve(app).await], "token": "u@pve!t=x"}))
                .unwrap();
        let p = Proxmox::new("lab", &cfg).await.unwrap();
        let s = serde_json::to_value(proxmox(&p, &Forecast::default()).await.unwrap()).unwrap();
        let rows = s["storages"].as_array().unwrap();
        let names: Vec<String> = rows.iter().map(label).collect();
        assert_eq!(
            names,
            ["empty@pve-01", "flat@pve-01", "local@pve-01", "nfs"]
        );
        assert_eq!(
            (rows[0]["used"].as_f64(), rows[0]["total"].as_f64()),
            (Some(7.0), Some(100.0))
        );
        assert_eq!(rows[0]["full_in"], Value::Null);
        assert_eq!(
            (rows[1]["full_in"].clone(), rows[1]["tone"].clone()),
            (Value::Null, json!("ok"))
        );
        assert_eq!(days(rows[2]["full_in"].as_f64()), Some(34.7));
        assert_eq!(rows[2]["tone"], "ok");
        assert_eq!(rows[3]["full_in"].as_f64(), Some(8.0 * HOUR));
        assert_eq!(
            (rows[3]["tone"].clone(), rows[3]["shared"].clone()),
            (json!("bad"), json!(true))
        );
    }

    async fn grafana(app: Router) -> Grafana {
        let cfg = serde_json::from_value(json!({"url": serve(app).await, "token": "t"})).unwrap();
        Grafana::new("g", &cfg).await.unwrap()
    }

    fn metrics(volumes: bool) -> NodeMetrics {
        let mut m = json!({"grafana": "g", "datasource": "vm", "cpu": "c", "memory": "m",
                           "namespace_label": "ns"});
        if volumes {
            m["volume_used"] = "used_q".into();
            m["volume_capacity"] = "cap_q".into();
        }
        serde_json::from_value(m).unwrap()
    }

    #[tokio::test]
    async fn kubernetes_volumes_from_promql() {
        let series = |ns: &str, claim: &str, step: f64| {
            let values: Vec<Value> = (0..168)
                .map(|h| json!([h as f64 * HOUR, format!("{}", 100.0 + h as f64 * step)]))
                .collect();
            json!({"metric": {"ns": ns, "persistentvolumeclaim": claim}, "values": values})
        };
        let app = Router::new()
            .route(
                "/api/datasources/proxy/uid/vm/api/v1/query_range",
                get(move |Query(q): Query<HashMap<String, String>>| async move {
                    assert_eq!((q["query"].as_str(), q["step"].as_str()), ("used_q", "3600"));
                    Json(json!({"status": "success", "data": {"result": [
                        series("db", "data", 2.0),
                        series("apps", "cache", 0.0),
                        series("apps", "nocap", 1.0),
                        {"metric": {"ns": "apps"}, "values": [[1, "1"]]},
                        {"metric": {"ns": "apps", "persistentvolumeclaim": "nopoints"}, "values": []},
                    ]}}))
                }),
            )
            .route(
                "/api/datasources/proxy/uid/vm/api/v1/query",
                get(|| async {
                    let cap = |ns: &str, claim: &str, v: &str| json!({"metric": {"ns": ns, "persistentvolumeclaim": claim}, "value": [1, v]});
                    Json(json!({"status": "success", "data": {"result": [
                        cap("db", "data", "1000"), cap("apps", "cache", "200"),
                        cap("apps", "nopoints", "10"), cap("apps", "zero", "0"),
                    ]}}))
                }),
            );
        let g = grafana(app).await;
        let v = serde_json::to_value(
            kubernetes(&g, &metrics(true), &Forecast::default())
                .await
                .unwrap(),
        )
        .unwrap();
        let rows = v["volumes"].as_array().unwrap();
        let names: Vec<String> = rows.iter().map(label).collect();
        assert_eq!(names, ["apps/cache", "db/data"]);
        assert_eq!(rows[0]["full_in"], Value::Null);
        assert_eq!(rows[0]["spark"].as_array().unwrap().len(), 28);
        assert_eq!(rows[0]["spark"][0], 0.5);
        // 2 an hour from 434: 566 left, 283 hours
        assert_eq!(rows[1]["used"], 434.0);
        assert_eq!(rows[1]["full_in"].as_f64(), Some(283.0 * HOUR));
        assert_eq!(rows[1]["tone"], "warn");

        let off = Forecast {
            enabled: false,
            ..Forecast::default()
        };
        let v = serde_json::to_value(kubernetes(&g, &metrics(true), &off).await.unwrap()).unwrap();
        assert_eq!(v["volumes"][1]["full_in"], Value::Null);
        assert_eq!(v["volumes"][1]["tone"], "ok");

        let err = kubernetes(&g, &metrics(false), &off).await.err().unwrap();
        assert_eq!(err.to_string(), "no volume queries configured");
    }
}

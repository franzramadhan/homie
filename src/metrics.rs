//! Prometheus text format 0.0.4 (RFC 0024), written by hand: counters bumped
//! where Homie logs, gauges read at scrape time. Labels are config names and
//! fixed words, never emails, IPs or versions (other than Homie's own).

use std::{
    collections::BTreeMap,
    fmt::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};

use serde_json::Value;

use crate::{integrations::Caller, server::AppState, status::Tone};

const BUCKETS: [f64; 8] = [0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

/// metric name: (type, rendered labels: value)
pub type Families = BTreeMap<&'static str, (&'static str, BTreeMap<String, f64>)>;

#[derive(Default)]
struct Hist {
    /// cumulative: each bucket counts every call at or under its bound
    buckets: [u64; 8],
    sum: f64,
    count: u64,
}

// ponytail: one global lock per map, per-family locks if scrapes ever contend
static FAMILIES: Mutex<Families> = Mutex::new(BTreeMap::new());
static HIST: Mutex<BTreeMap<String, Hist>> = Mutex::new(BTreeMap::new());
pub static STREAMS: AtomicI64 = AtomicI64::new(0);
pub static CONSOLES: AtomicI64 = AtomicI64::new(0);

/// Counts one open stream or console while it lives.
pub struct Open(&'static AtomicI64);

impl Open {
    pub fn new(g: &'static AtomicI64) -> Self {
        g.fetch_add(1, Ordering::Relaxed);
        Self(g)
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn labels(l: &[(&str, &str)]) -> String {
    if l.is_empty() {
        return String::new();
    }
    let l: Vec<String> = l
        .iter()
        .map(|(k, v)| {
            let v = v
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            format!("{k}=\"{v}\"")
        })
        .collect();
    format!("{{{}}}", l.join(","))
}

fn put(
    f: &mut Families,
    name: &'static str,
    kind: &'static str,
    l: &[(&str, &str)],
    op: impl FnOnce(&mut f64),
) {
    op(f.entry(name)
        .or_insert((kind, BTreeMap::new()))
        .1
        .entry(labels(l))
        .or_default());
}

pub fn gauge(f: &mut Families, name: &'static str, l: &[(&str, &str)], v: f64) {
    put(f, name, "gauge", l, |x| *x = v);
}

pub fn inc(name: &'static str, l: &[(&str, &str)]) {
    put(&mut FAMILIES.lock().unwrap(), name, "counter", l, |v| {
        *v += 1.0
    });
}

/// `pve.guest.start` and friends as the RFC's action words.
pub fn action(action: &str, result: &str) {
    let a = match action {
        a if a.starts_with("pve.guest.") => "power",
        "flux.reconcile" => "reconcile",
        "argocd.refresh" => "refresh",
        a => a,
    };
    inc("homie_actions_total", &[("action", a), ("result", result)]);
}

/// One backend call through the cache. Keys are `kind:name[:who]`; only the
/// first two parts become labels.
pub fn query(key: &str, result: &str, secs: f64) {
    let mut parts = key.splitn(3, ':');
    let (kind, name) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let l = [("kind", kind), ("name", name)];
    {
        let mut f = FAMILIES.lock().unwrap();
        put(
            &mut f,
            "homie_query_total",
            "counter",
            &[("kind", kind), ("name", name), ("result", result)],
            |v| *v += 1.0,
        );
        if result == "ok" {
            let now = jiff::Timestamp::now().as_millisecond() as f64 / 1000.0;
            put(
                &mut f,
                "homie_query_last_success_timestamp_seconds",
                "gauge",
                &l,
                |v| *v = now,
            );
        }
    }
    let mut hist = HIST.lock().unwrap();
    let h = hist.entry(labels(&l)).or_default();
    for (b, le) in h.buckets.iter_mut().zip(BUCKETS) {
        if secs <= le {
            *b += 1;
        }
    }
    h.sum += secs;
    h.count += 1;
}

/// The page: what happened so far plus `now`, the gauges of this scrape.
pub fn render(now: Families) -> String {
    let mut all: Families = FAMILIES
        .lock()
        .unwrap()
        .iter()
        .map(|(k, (t, s))| (*k, (*t, s.clone())))
        .collect();
    all.extend(now);
    let mut out = String::new();
    for (name, (kind, series)) in &all {
        let _ = writeln!(out, "# TYPE {name} {kind}");
        for (l, v) in series {
            let _ = writeln!(out, "{name}{l} {v}");
        }
    }
    let hist = HIST.lock().unwrap();
    if !hist.is_empty() {
        out.push_str("# TYPE homie_query_duration_seconds histogram\n");
    }
    for (l, h) in hist.iter() {
        let inner = &l[1..l.len() - 1];
        let name = "homie_query_duration_seconds";
        for (b, le) in h.buckets.iter().zip(BUCKETS) {
            let _ = writeln!(out, "{name}_bucket{{{inner},le=\"{le}\"}} {b}");
        }
        let _ = writeln!(out, "{name}_bucket{{{inner},le=\"+Inf\"}} {}", h.count);
        let _ = writeln!(out, "{name}_sum{l} {}", h.sum);
        let _ = writeln!(out, "{name}_count{l} {}", h.count);
    }
    out
}

/// Gauges read for one scrape; with `metrics.cards` the cards go through the
/// shared cache, like the notifier (RFC 0019).
pub async fn scrape(state: &AppState) -> String {
    let mut g = Families::new();
    gauge(
        &mut g,
        "homie_build_info",
        &[("version", crate::VERSION)],
        1.0,
    );
    let sessions = state.auth.as_ref().map_or(0, |a| a.sessions());
    gauge(&mut g, "homie_sessions_active", &[], sessions as f64);
    let streams = STREAMS.load(Ordering::Relaxed);
    gauge(&mut g, "homie_stream_clients", &[], streams as f64);
    let consoles = CONSOLES.load(Ordering::Relaxed);
    gauge(&mut g, "homie_consoles_active", &[], consoles as f64);
    let Some(m) = state.cfg.metrics.as_ref().filter(|m| m.cards) else {
        return render(g);
    };
    let caller = Caller {
        email: Some("homie-metrics".into()),
        service_account: m.kubernetes_service_account.clone(),
        ..Default::default()
    };
    let lab = &state.lab;
    // the lab is fixed at start: list (and log what is left out) once
    static CARDS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    let cards = CARDS.get_or_init(|| {
        let mut c = crate::notify::cards(lab, m.kubernetes_service_account.is_some(), &[]);
        if lab.updates_on() {
            c.push(("updates".into(), "_".into()));
        }
        c
    });
    let envs = futures::future::join_all(
        cards
            .iter()
            .map(|(k, n)| lab.query(&state.cache, k, n, &caller)),
    )
    .await;
    for ((kind, name), env) in cards.iter().zip(envs) {
        let Some(env) = env else { continue };
        if kind != "updates"
            && let Some(tone) = env.tone
        {
            let card = crate::notify::card_id(kind);
            for (t, word) in [
                (Tone::Ok, "ok"),
                (Tone::Warn, "warn"),
                (Tone::Bad, "bad"),
                (Tone::Quiet, "quiet"),
            ] {
                let l = [("card", card), ("name", name.as_str()), ("status", word)];
                gauge(&mut g, "homie_card_status", &l, (tone == t) as u8 as f64);
            }
        }
        match (kind.as_str(), &env.data) {
            ("certs", Some(d)) => certs(&mut g, d),
            ("updates", Some(d)) => updates(&mut g, d),
            _ => {}
        }
    }
    render(g)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn certs(g: &mut Families, d: &Value) {
    for r in arr(&d["rows"]) {
        let target = format!("{}:{}", r["host"].as_str().unwrap_or(""), r["port"]);
        let l = [("target", target.as_str())];
        let down = arr(&r["reasons"])
            .iter()
            .any(|s| s.as_str().is_some_and(|s| s.starts_with("unreachable")));
        let ok = !down && !arr(&r["chain"]).is_empty();
        gauge(g, "homie_cert_probe_success", &l, ok as u8 as f64);
        if let Some(t) = r["expires"]
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        {
            gauge(
                g,
                "homie_cert_expiry_timestamp_seconds",
                &l,
                t.as_second() as f64,
            );
        }
    }
}

fn updates(g: &mut Families, d: &Value) {
    let newer = |v: &Value| !v["patch"].is_null() || !v["newest"].is_null();
    let count = |key: &str, field: &str| arr(&d[key]).iter().filter(|x| newer(&x[field])).count();
    let packages: u64 = arr(&d["proxmox"])
        .iter()
        .filter_map(|n| n["updates"].as_u64())
        .sum();
    for (kind, n) in [
        ("packages", packages as usize),
        ("charts", count("charts", "latest")),
        ("images", count("images", "latest")),
        ("distribution", count("kubernetes", "distro_latest")),
        ("flux", count("kubernetes", "flux_latest")),
    ] {
        gauge(g, "homie_updates_available", &[("kind", kind)], n as f64);
    }
    for n in arr(&d["proxmox"]) {
        let l = [
            ("cluster", n["cluster"].as_str().unwrap_or("")),
            ("node", n["node"].as_str().unwrap_or("")),
        ];
        gauge(
            g,
            "homie_node_reboot_required",
            &l,
            (n["reboot"] == true) as u8 as f64,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_escape() {
        assert_eq!(labels(&[]), "");
        assert_eq!(
            labels(&[("a", "x\"y\\z\n"), ("b", "c")]),
            r#"{a="x\"y\\z\n",b="c"}"#
        );
    }

    #[test]
    fn histogram_is_cumulative_and_inf_is_count() {
        query("test-hist:one:someone@example.com", "ok", 0.3);
        query("test-hist:one", "error", 20.0);
        let out = render(Families::new());
        let line = |s: &str| {
            out.lines()
                .find(|l| l.starts_with(s))
                .unwrap_or_else(|| panic!("{s} missing in\n{out}"))
                .rsplit(' ')
                .next()
                .unwrap()
                .to_string()
        };
        let b = r#"homie_query_duration_seconds_bucket{kind="test-hist",name="one","#;
        assert_eq!(line(&format!("{b}le=\"0.25\"}}")), "0");
        assert_eq!(line(&format!("{b}le=\"0.5\"}}")), "1");
        assert_eq!(line(&format!("{b}le=\"10\"}}")), "1");
        assert_eq!(line(&format!("{b}le=\"+Inf\"}}")), "2");
        assert_eq!(
            line(r#"homie_query_duration_seconds_count{kind="test-hist",name="one"}"#),
            "2"
        );
        assert_eq!(
            line(r#"homie_query_total{kind="test-hist",name="one",result="error"}"#),
            "1"
        );
        assert!(out.contains("# TYPE homie_query_total counter\n"));
        // the third key part (a user) never becomes a label
        assert!(!out.contains("example.com"), "{out}");
    }

    #[test]
    fn updates_and_certs_without_versions() {
        let mut g = Families::new();
        updates(
            &mut g,
            &json!({
                "proxmox": [{"cluster": "pve", "node": "a", "updates": 3, "reboot": true, "running_kernel": "6.8.12-1-pve"}],
                "kubernetes": [{"cluster": "lab", "distro_latest": {"patch": "v1.33.9", "newest": null},
                                "flux_latest": {"patch": null, "newest": null}}],
                "charts": [{"latest": {"patch": null, "newest": "2.0.0"}}, {"latest": {"patch": null, "newest": null}}],
                "images": [],
            }),
        );
        certs(
            &mut g,
            &json!({"rows": [
                {"host": "a.example", "port": 443, "reasons": [], "chain": [{}], "expires": "2026-11-01T00:00:00Z"},
                {"host": "b.example", "port": 8443, "reasons": ["unreachable: timed out"], "chain": []},
            ]}),
        );
        let out = render(g);
        for want in [
            r#"homie_updates_available{kind="packages"} 3"#,
            r#"homie_updates_available{kind="charts"} 1"#,
            r#"homie_updates_available{kind="distribution"} 1"#,
            r#"homie_updates_available{kind="flux"} 0"#,
            r#"homie_node_reboot_required{cluster="pve",node="a"} 1"#,
            r#"homie_cert_probe_success{target="a.example:443"} 1"#,
            r#"homie_cert_probe_success{target="b.example:8443"} 0"#,
            r#"homie_cert_expiry_timestamp_seconds{target="a.example:443"} 1793491200"#,
        ] {
            assert!(out.contains(want), "{want} missing in\n{out}");
        }
        for leak in ["6.8.12", "v1.33", "2.0.0"] {
            assert!(!out.contains(leak), "{leak} leaked in\n{out}");
        }
    }
}

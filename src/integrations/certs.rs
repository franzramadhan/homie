//! Certificate expiry (RFC 0018): the chains seen by the integrations' own
//! HTTP clients, plus handshake probes of listed targets and of TLS
//! hostnames discovered in clusters, each judged by its place in the chain.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use futures::StreamExt;
use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ListParams},
    core::GroupVersionKind,
};
use serde::Serialize;
use serde_json::Value;

use super::kubernetes::Kube;
use crate::{
    Result,
    config::{CertDiscovery, CertSource, Certificates},
    status::Tone,
    tls::{self, Cert, Seen},
};

#[derive(Debug, Serialize)]
pub struct Certs {
    rows: Vec<Row>,
    /// discovery or cert-manager reads that failed, per cluster
    errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Row {
    host: String,
    port: u16,
    /// `integration`, `listed` or `<cluster>/<namespace>/<kind>/<name>`
    sources: Vec<String>,
    status: Tone,
    reasons: Vec<String>,
    /// the earliest expiry that counts (leaf, intermediates, private root)
    expires: Option<String>,
    /// which certificate that is: `leaf`, `intermediate R11`, `root ...`
    expires_cert: Option<String>,
    issuer: String,
    chain: Vec<Cert>,
}

struct Target {
    host: String,
    /// the port to connect to
    port: u16,
    address: Option<String>,
    ca_file: Option<PathBuf>,
    sources: BTreeSet<String>,
}

struct Probed {
    seen: Option<Seen>,
    unreachable: Option<String>,
    at: Instant,
}

/// The last probe per `host:port`: a target is probed again after
/// `interval_hours`, and a failed connection keeps the last chain.
static PROBES: LazyLock<Mutex<HashMap<String, Probed>>> = LazyLock::new(Default::default);

const PROBES_AT_ONCE: usize = 4;

/// Every row, probing the targets that are due.
pub async fn collect(cfg: &Certificates, clusters: Vec<(&Kube, Result<Client>)>) -> Certs {
    let mut errors = Vec::new();
    let mut targets: BTreeMap<String, Target> = BTreeMap::new();
    let mut add = |host: String,
                   port: u16,
                   address: Option<String>,
                   ca_file: Option<PathBuf>,
                   source: String| {
        let t = targets.entry(format!("{host}:{port}")).or_insert(Target {
            host,
            port,
            address,
            ca_file,
            sources: BTreeSet::new(),
        });
        t.sources.insert(source);
    };
    for t in &cfg.targets {
        add(
            t.host.clone(),
            t.port,
            t.address.clone(),
            t.ca_file.clone(),
            "listed".into(),
        );
    }
    let mut renewals = HashMap::new();
    for (k, client) in clusters {
        let client = match client {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{}: {e}", k.name));
                continue;
            }
        };
        if k.certificates.discover {
            match discover(&client, &k.name, &k.certificates).await {
                Ok(found) => {
                    let via = k
                        .certificates
                        .discover_address
                        .as_deref()
                        .and_then(|a| a.rsplit_once(':'));
                    for (host, port, source) in found {
                        match via {
                            Some((addr, p)) => add(
                                host,
                                p.parse().unwrap_or(port),
                                Some(addr.into()),
                                None,
                                source,
                            ),
                            None => add(host, port, None, None, source),
                        }
                    }
                }
                Err(e) => errors.push(format!("{}: discovery: {e}", k.name)),
            }
        }
        if k.cert_manager {
            match renewal_failures(&client).await {
                Ok(r) => renewals.extend(r),
                Err(e) => errors.push(format!("{}: cert-manager: {e}", k.name)),
            }
        }
    }

    probe_due(
        &targets,
        Duration::from_secs(u64::from(cfg.interval_hours) * 3600),
        &mut errors,
    )
    .await;

    let now = jiff::Timestamp::now().as_second();
    let probes = PROBES.lock().unwrap();
    let mut rows: Vec<Row> = targets
        .iter()
        .map(|(key, t)| {
            let p = probes.get(key);
            row(
                &t.host,
                t.port,
                t.sources.iter().cloned().collect(),
                p.and_then(|p| p.seen.as_ref()),
                p.and_then(|p| p.unreachable.as_deref()),
                renewals.get(&t.host).map(String::as_str),
                cfg,
                now,
            )
        })
        .collect();
    if cfg.endpoints {
        for (key, seen) in tls::ENDPOINTS.lock().unwrap().iter() {
            let (host, port) = key.rsplit_once(':').unwrap_or((key, "443"));
            match rows
                .iter_mut()
                .find(|r| format!("{}:{}", r.host, r.port) == *key)
            {
                Some(r) => r.sources.push("integration".into()),
                None => rows.push(row(
                    host,
                    port.parse().unwrap_or(443),
                    vec!["integration".into()],
                    Some(seen),
                    None,
                    renewals.get(host).map(String::as_str),
                    cfg,
                    now,
                )),
            }
        }
    }
    rows.sort_by(|a, b| {
        b.status
            .cmp(&a.status)
            .then(a.expires.cmp(&b.expires))
            .then(a.host.cmp(&b.host))
    });
    Certs { rows, errors }
}

/// key, host, port, address, ca_file
type Due = (String, String, u16, Option<String>, Option<PathBuf>);

async fn probe_due(
    targets: &BTreeMap<String, Target>,
    interval: Duration,
    errors: &mut Vec<String>,
) {
    // owned, so the stream's futures borrow nothing (and stay Send)
    let due: Vec<Due> = {
        let probes = PROBES.lock().unwrap();
        targets
            .iter()
            .filter(|(key, _)| probes.get(*key).is_none_or(|p| p.at.elapsed() >= interval))
            .map(|(key, t)| {
                (
                    key.clone(),
                    t.host.clone(),
                    t.port,
                    t.address.clone(),
                    t.ca_file.clone(),
                )
            })
            .collect()
    };
    let results: Vec<(String, Result<Seen>)> = futures::stream::iter(due)
        .map(|(key, host, port, address, ca_file)| async move {
            let r = match tls::roots(ca_file.as_deref()) {
                Ok(roots) => tls::probe(&host, port, address.as_deref(), roots).await,
                Err(e) => Err(e),
            };
            (key, r)
        })
        .buffer_unordered(PROBES_AT_ONCE)
        .collect()
        .await;
    let mut probes = PROBES.lock().unwrap();
    for (key, r) in results {
        let at = Instant::now();
        match r {
            Ok(seen) => {
                tracing::debug!(msg = "cert probe", target = %key, result = "ok");
                probes.insert(
                    key,
                    Probed {
                        seen: Some(seen),
                        unreachable: None,
                        at,
                    },
                );
            }
            Err(e) => {
                tracing::warn!(msg = "cert probe", target = %key, error = %e, result = "unreachable");
                if e.to_string().starts_with("reading ") {
                    errors.push(format!("{key}: {e}"));
                }
                let seen = probes.remove(&key).and_then(|p| p.seen);
                probes.insert(
                    key,
                    Probed {
                        seen,
                        unreachable: Some(e.to_string()),
                        at,
                    },
                );
            }
        }
    }
}

struct Verdict {
    tone: Tone,
    reasons: Vec<String>,
}

impl Verdict {
    fn flag(&mut self, tone: Tone, reason: String) {
        self.tone = self.tone.max(tone);
        self.reasons.push(reason);
    }
}

fn when(secs_left: i64) -> String {
    let days = secs_left.abs() / 86400;
    match (secs_left < 0, days) {
        (true, 0) => "expired today".into(),
        (true, 1) => "expired 1 day ago".into(),
        (true, d) => format!("expired {d} days ago"),
        (false, 0) => "expires today".into(),
        (false, 1) => "expires in 1 day".into(),
        (false, d) => format!("expires in {d} days"),
    }
}

/// One row's status. Thresholds by who can act on the certificate: leaf
/// and intermediates at `warn_days`, a private root at `root_warn_days`, a
/// bundled public root never (a Homie upgrade replaces it).
#[allow(clippy::too_many_arguments)]
fn row(
    host: &str,
    port: u16,
    sources: Vec<String>,
    seen: Option<&Seen>,
    unreachable: Option<&str>,
    renewal: Option<&str>,
    cfg: &Certificates,
    now: i64,
) -> Row {
    let mut v = Verdict {
        tone: Tone::Ok,
        reasons: Vec::new(),
    };
    let chain = seen.map(|s| s.chain.clone()).unwrap_or_default();
    let mut earliest: Option<&Cert> = None;
    for c in chain
        .iter()
        .filter(|c| c.role != "unused" && !(c.role == "root" && c.public))
    {
        if earliest.is_none_or(|e| c.not_after < e.not_after) {
            earliest = Some(c);
        }
    }
    let label = |c: &Cert| match c.role.as_str() {
        "leaf" => String::new(),
        role => format!("{role} {} ", c.subject),
    };
    for c in chain.iter().filter(|c| c.role != "unused") {
        let left = c.not_after - now;
        let day = 86400;
        let (warn, bad) = match (c.role.as_str(), c.public) {
            ("root", true) => continue,
            ("root", false) => (i64::from(cfg.root_warn_days) * day, 0),
            _ => (i64::from(cfg.warn_days) * day, 3 * day),
        };
        if left < bad {
            v.flag(Tone::Bad, format!("{}{}", label(c), when(left)));
        } else if left < warn {
            v.flag(Tone::Warn, format!("{}{}", label(c), when(left)));
        }
        if c.not_before > now {
            v.flag(Tone::Warn, format!("{}not valid yet", label(c)));
        }
    }
    match seen.and_then(|s| s.problem.as_deref()) {
        // the dates above explain these
        Some("expired" | "not_yet_valid") if !v.reasons.is_empty() => {}
        Some("expired") => v.flag(Tone::Bad, "chain expired".into()),
        Some("not_yet_valid") => v.flag(Tone::Warn, "chain not valid yet".into()),
        Some("unknown_issuer") => {
            // the last certificate sent: self-issued means a root (or a
            // self-signed leaf) that is not trusted, else a missing issuer
            let reason = match chain.last() {
                Some(c) if c.issuer == c.subject && chain.len() == 1 => {
                    "self-signed, not trusted".to_string()
                }
                Some(c) if c.issuer == c.subject => format!("untrusted root {}", c.subject),
                Some(c) => format!(
                    "issuer {} not found: incomplete chain or untrusted CA",
                    c.issuer
                ),
                None => "untrusted".into(),
            };
            v.flag(Tone::Bad, reason);
        }
        Some("name_mismatch") => {
            let names = chain
                .first()
                .map(|c| c.names.join(", "))
                .unwrap_or_default();
            v.flag(
                Tone::Warn,
                format!("name mismatch: certificate is for {names}"),
            );
        }
        Some(other) => v.flag(Tone::Bad, other.to_string()),
        None => {}
    }
    if seen.is_some_and(|s| !s.verified) {
        v.flag(Tone::Ok, "not verified (tls_verify = false)".into());
    }
    if let Some(e) = unreachable {
        v.flag(Tone::Warn, format!("unreachable: {e}"));
    }
    if let Some(m) = renewal {
        v.flag(Tone::Warn, format!("renewal failing: {m}"));
    }
    Row {
        host: host.into(),
        port,
        sources,
        status: v.tone,
        reasons: v.reasons,
        expires: earliest
            .and_then(|c| jiff::Timestamp::from_second(c.not_after).ok())
            .map(|t| t.to_string()),
        expires_cert: earliest.map(|c| match c.role.as_str() {
            "leaf" => "leaf".into(),
            role => format!("{role} {}", c.subject),
        }),
        issuer: chain.first().map(|c| c.issuer.clone()).unwrap_or_default(),
        chain,
    }
}

async fn list(
    client: &Client,
    group: &str,
    version: &str,
    kind: &str,
) -> Result<Vec<DynamicObject>> {
    let ar = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
    match Api::<DynamicObject>::all_with(client.clone(), &ar)
        .list(&ListParams::default())
        .await
    {
        Ok(l) => Ok(l.items),
        Err(kube::Error::Api(s)) if s.code == 404 => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn strs(v: &Value) -> impl Iterator<Item = &str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}

fn ns_name(o: &DynamicObject) -> (&str, &str) {
    (
        o.metadata.namespace.as_deref().unwrap_or_default(),
        o.metadata.name.as_deref().unwrap_or_default(),
    )
}

/// `*.example.com` covers `a.example.com` (and deeper, as Gateway API
/// listeners do); no hostname covers everything.
fn covers(listener: Option<&str>, host: &str) -> bool {
    match listener {
        None => true,
        Some(l) => match l.strip_prefix('*') {
            Some(suffix) => host.ends_with(suffix) && host.len() > suffix.len(),
            None => l == host,
        },
    }
}

const GATEWAY: &str = "gateway.networking.k8s.io";

/// TLS hostnames, with the port to probe and where each was found. Wildcard
/// names are never probed: no client connects to them.
pub async fn discover(
    client: &Client,
    cluster: &str,
    d: &CertDiscovery,
) -> Result<Vec<(String, u16, String)>> {
    let mut found = Vec::new();
    let mut push = |host: &str, port: u16, kind: &str, o: &DynamicObject| {
        let host = host.trim();
        if !host.is_empty() && !host.starts_with('*') {
            let (ns, name) = ns_name(o);
            found.push((
                host.to_string(),
                port,
                format!("{cluster}/{ns}/{kind}/{name}"),
            ));
        }
    };
    let wants = |s: CertSource| d.sources.contains(&s);
    let ingresses = if wants(CertSource::Ingress) || wants(CertSource::Annotations) {
        list(client, "networking.k8s.io", "v1", "Ingress").await?
    } else {
        Vec::new()
    };
    let gateways = if wants(CertSource::Gateway) || wants(CertSource::Annotations) {
        list(client, GATEWAY, "v1", "Gateway").await?
    } else {
        Vec::new()
    };
    if wants(CertSource::Ingress) {
        for i in &ingresses {
            for t in i.data["spec"]["tls"].as_array().into_iter().flatten() {
                strs(&t["hosts"]).for_each(|h| push(h, 443, "ingress", i));
            }
        }
    }
    if wants(CertSource::Gateway) {
        let mut routes: Vec<_> = list(client, GATEWAY, "v1", "HTTPRoute")
            .await?
            .into_iter()
            .map(|r| ("httproute", r))
            .collect();
        routes.extend(
            list(client, GATEWAY, "v1alpha2", "TLSRoute")
                .await?
                .into_iter()
                .map(|r| ("tlsroute", r)),
        );
        for g in &gateways {
            let (gns, gname) = ns_name(g);
            for l in g.data["spec"]["listeners"].as_array().into_iter().flatten() {
                if !matches!(l["protocol"].as_str(), Some("HTTPS" | "TLS")) {
                    continue;
                }
                let port = l["port"]
                    .as_u64()
                    .and_then(|p| u16::try_from(p).ok())
                    .unwrap_or(443);
                let hostname = l["hostname"].as_str();
                if let Some(h) = hostname.filter(|h| !h.starts_with('*')) {
                    push(h, port, "gateway", g);
                    continue;
                }
                // a wildcard or empty listener: the names of its routes
                for (kind, r) in &routes {
                    let (rns, _) = ns_name(r);
                    let attached = r.data["spec"]["parentRefs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|p| {
                            p["name"] == gname
                                && p["namespace"].as_str().unwrap_or(rns) == gns
                                && p["kind"].as_str().unwrap_or("Gateway") == "Gateway"
                                && p["sectionName"].as_str().is_none_or(|s| l["name"] == s)
                        });
                    if attached {
                        strs(&r.data["spec"]["hostnames"])
                            .filter(|h| covers(hostname, h))
                            .for_each(|h| push(h, port, kind, r));
                    }
                }
            }
        }
    }
    if wants(CertSource::Annotations) {
        let services = list(client, "", "v1", "Service").await?;
        let objects = services
            .iter()
            .map(|o| ("service", o))
            .chain(ingresses.iter().map(|o| ("ingress", o)))
            .chain(gateways.iter().map(|o| ("gateway", o)));
        for (kind, o) in objects {
            let Some(annotations) = &o.metadata.annotations else {
                continue;
            };
            // a Service's port named https or tls, else 443
            let port = o.data["spec"]["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| matches!(p["name"].as_str(), Some("https" | "tls")))
                .and_then(|p| p["port"].as_u64())
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(443);
            for key in &d.annotations {
                for h in annotations.get(key).into_iter().flat_map(|v| v.split(',')) {
                    push(h, port, kind, o);
                }
            }
        }
    }
    Ok(found)
}

/// DNS name to the message of a cert-manager Certificate that is not Ready.
pub async fn renewal_failures(client: &Client) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for c in list(client, "cert-manager.io", "v1", "Certificate").await? {
        let not_ready = c.data["status"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|x| x["type"] == "Ready" && x["status"] == "False");
        if let Some(cond) = not_ready {
            let msg = cond["message"].as_str().unwrap_or("not Ready").to_string();
            for h in strs(&c.data["spec"]["dnsNames"]) {
                out.insert(h.to_string(), msg.clone());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{json, kube_client, list as klist, serve};
    use serde_json::json;

    const DAY: i64 = 86400;
    const NOW: i64 = 1_800_000_000;

    fn cert(role: &str, days: i64, public: bool) -> Cert {
        Cert {
            role: role.into(),
            subject: format!("{role} CN"),
            issuer: "Issuer".into(),
            not_before: NOW - 100 * DAY,
            not_after: NOW + days * DAY + 60,
            names: if role == "leaf" {
                vec!["a.example".into()]
            } else {
                vec![]
            },
            public,
        }
    }

    fn seen(chain: Vec<Cert>, problem: Option<&str>) -> Seen {
        Seen {
            chain,
            problem: problem.map(Into::into),
            verified: true,
        }
    }

    fn judge(s: Option<Seen>) -> (Tone, Vec<String>) {
        let r = row(
            "a.example",
            443,
            vec![],
            s.as_ref(),
            None,
            None,
            &Certificates::default(),
            NOW,
        );
        (r.status, r.reasons)
    }

    #[test]
    fn thresholds_by_place_in_the_chain() {
        let ok = vec![
            cert("leaf", 60, false),
            cert("intermediate", 300, false),
            cert("root", 2, true),
        ];
        assert_eq!(judge(Some(seen(ok, None))), (Tone::Ok, vec![]));
        let leaf_soon = vec![cert("leaf", 9, false), cert("intermediate", 300, false)];
        assert_eq!(
            judge(Some(seen(leaf_soon, None))),
            (Tone::Warn, vec!["expires in 9 days".into()])
        );
        let leaf_now = vec![cert("leaf", 2, false)];
        assert_eq!(judge(Some(seen(leaf_now, None))).0, Tone::Bad);
        let inter = vec![cert("leaf", 60, false), cert("intermediate", 20, false)];
        assert_eq!(judge(Some(seen(inter, None))).0, Tone::Ok);
        let inter = vec![cert("leaf", 60, false), cert("intermediate", 10, false)];
        assert_eq!(
            judge(Some(seen(inter, None))),
            (
                Tone::Warn,
                vec!["intermediate intermediate CN expires in 10 days".into()]
            )
        );
        // a private root warns 90 days ahead, a public one never
        let private = vec![cert("leaf", 60, false), cert("root", 80, false)];
        assert_eq!(judge(Some(seen(private, None))).0, Tone::Warn);
        let public = vec![cert("leaf", 60, false), cert("root", -5, true)];
        assert_eq!(judge(Some(seen(public, None))).0, Tone::Ok);
        let unused = vec![cert("leaf", 60, false), cert("unused", -5, false)];
        assert_eq!(judge(Some(seen(unused, None))).0, Tone::Ok);
        let mut expired = cert("leaf", -2, false);
        expired.not_after = NOW - 2 * DAY - 60;
        assert_eq!(
            judge(Some(seen(vec![expired], Some("expired")))),
            (Tone::Bad, vec!["expired 2 days ago".into()])
        );
    }

    #[test]
    fn problems_and_extras() {
        let chain = || vec![cert("leaf", 60, false), cert("intermediate", 300, false)];
        let mut selfie = cert("leaf", 60, false);
        selfie.issuer = selfie.subject.clone();
        assert_eq!(
            judge(Some(seen(vec![selfie.clone()], Some("unknown_issuer")))).1,
            ["self-signed, not trusted"]
        );
        let mut root = cert("intermediate", 300, false);
        root.issuer = root.subject.clone();
        assert_eq!(
            judge(Some(seen(
                vec![cert("leaf", 60, false), root],
                Some("unknown_issuer")
            )))
            .1,
            ["untrusted root intermediate CN"]
        );
        let (t, r) = judge(Some(seen(chain(), Some("unknown_issuer"))));
        assert_eq!(
            (t, r[0].as_str()),
            (
                Tone::Bad,
                "issuer Issuer not found: incomplete chain or untrusted CA"
            )
        );
        let (t, r) = judge(Some(seen(chain(), Some("name_mismatch"))));
        assert_eq!(
            (t, r[0].as_str()),
            (Tone::Warn, "name mismatch: certificate is for a.example")
        );
        let mut s = seen(chain(), None);
        s.verified = false;
        assert_eq!(
            judge(Some(s)),
            (Tone::Ok, vec!["not verified (tls_verify = false)".into()])
        );
        assert_eq!(judge(None).0, Tone::Ok);
        let r = row(
            "a.example",
            443,
            vec![],
            None,
            Some("timed out"),
            Some("order failed"),
            &Certificates::default(),
            NOW,
        );
        assert_eq!(r.status, Tone::Warn);
        assert_eq!(
            r.reasons,
            ["unreachable: timed out", "renewal failing: order failed"]
        );
        let r = row(
            "a.example",
            443,
            vec![],
            Some(&seen(chain(), None)),
            None,
            None,
            &Certificates::default(),
            NOW,
        );
        assert_eq!(r.expires_cert.as_deref(), Some("leaf"));
        assert_eq!(r.issuer, "Issuer");
    }

    #[test]
    fn wildcards() {
        assert!(covers(None, "a.example"));
        assert!(covers(Some("*.example"), "a.example"));
        assert!(covers(Some("*.example"), "b.a.example"));
        assert!(!covers(Some("*.example"), "example"));
        assert!(!covers(Some("a.example"), "b.example"));
    }

    fn fake_cluster() -> axum::Router {
        let ing = json!([{"metadata": {"namespace": "web", "name": "site",
                                        "annotations": {"homie.dev/tls-hosts": "extra.example"}},
                          "spec": {"tls": [{"hosts": ["site.example", "*.wild.example"]}]}}]);
        let gw = json!([{"metadata": {"namespace": "gw", "name": "main"},
            "spec": {"listeners": [
                {"name": "exact", "protocol": "HTTPS", "port": 443, "hostname": "gw.example"},
                {"name": "wild", "protocol": "HTTPS", "port": 8443, "hostname": "*.apps.example"},
                {"name": "plain", "protocol": "HTTP", "port": 80, "hostname": "http.example"}]}}]);
        let routes = json!([
            {"apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
             "metadata": {"namespace": "app", "name": "a"},
             "spec": {"parentRefs": [{"name": "main", "namespace": "gw", "sectionName": "wild"}],
                      "hostnames": ["a.apps.example", "other.example"]}},
            {"apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
             "metadata": {"namespace": "app", "name": "b"},
             "spec": {"parentRefs": [{"name": "elsewhere"}], "hostnames": ["b.apps.example"]}}]);
        let svc = json!([{"metadata": {"namespace": "dns", "name": "lb",
                                        "annotations": {"external-dns.alpha.kubernetes.io/hostname": "lb.example, lb2.example"}},
                          "spec": {"ports": [{"name": "web", "port": 80}, {"name": "https", "port": 8443}]}}]);
        json("/apis/networking.k8s.io/v1/ingresses", klist("Ingress", ing))
            .merge(json("/apis/gateway.networking.k8s.io/v1/gateways", klist("Gateway", gw)))
            .merge(json("/apis/gateway.networking.k8s.io/v1/httproutes", klist("HTTPRoute", routes)))
            .merge(json("/api/v1/services", klist("Service", svc)))
            .merge(json(
                "/apis/cert-manager.io/v1/certificates",
                klist("Certificate", json!([
                    {"metadata": {"namespace": "web", "name": "site"},
                     "spec": {"dnsNames": ["site.example"]},
                     "status": {"conditions": [{"type": "Ready", "status": "False", "message": "order failed"}]}},
                    {"metadata": {"namespace": "web", "name": "fine"},
                     "spec": {"dnsNames": ["fine.example"]},
                     "status": {"conditions": [{"type": "Ready", "status": "True"}]}}])),
            ))
    }

    #[tokio::test]
    async fn discovers_hostnames_from_every_source() {
        let url = serve(fake_cluster()).await;
        let client = kube_client(&url);
        let mut d = CertDiscovery {
            discover: true,
            ..Default::default()
        };
        d.annotations.push("homie.dev/tls-hosts".into());
        let mut found = discover(&client, "lab", &d).await.unwrap();
        found.sort();
        let want = [
            ("a.apps.example", 8443, "lab/app/httproute/a"),
            ("extra.example", 443, "lab/web/ingress/site"),
            ("gw.example", 443, "lab/gw/gateway/main"),
            ("lb.example", 8443, "lab/dns/service/lb"),
            ("lb2.example", 8443, "lab/dns/service/lb"),
            ("site.example", 443, "lab/web/ingress/site"),
        ];
        let found: Vec<_> = found
            .iter()
            .map(|(h, p, s)| (h.as_str(), *p, s.as_str()))
            .collect();
        assert_eq!(found, want);
        // one source only; TLSRoutes are absent (404) without failing
        let d = CertDiscovery {
            discover: true,
            sources: vec![CertSource::Ingress],
            ..Default::default()
        };
        assert_eq!(discover(&client, "lab", &d).await.unwrap().len(), 1);
        let failing = renewal_failures(&client).await.unwrap();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing["site.example"], "order failed");
    }

    #[tokio::test]
    async fn collects_listed_targets_and_endpoints() {
        let pki = tls::tests::Pki::new(2045, 2040);
        let dir = std::env::temp_dir().join(format!("homie-certs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ca = dir.join("ca.pem");
        std::fs::write(&ca, &pki.root_pem).unwrap();
        let (leaf, key) = pki.leaf(&["listed.example"], 2024, 2040);
        let port = tls::tests::tls_server(vec![leaf, pki.inter.clone()], &key).await;

        // an integration's HTTP client records its endpoint
        let t = crate::http::target(
            &format!("https://127.0.0.1:{port}"),
            &crate::config::Tls {
                tls_verify: true,
                ca_file: Some(ca.clone()),
                tls_server_name: Some("listed.example".into()),
            },
        )
        .await
        .unwrap();
        let _ = t.client.get(t.base.clone()).send().await;
        assert!(
            tls::ENDPOINTS
                .lock()
                .unwrap()
                .contains_key(&format!("listed.example:{port}"))
        );

        let cfg = Certificates {
            targets: vec![
                crate::config::CertTarget {
                    host: "listed.example".into(),
                    port,
                    address: Some("127.0.0.1".into()),
                    ca_file: Some(ca),
                },
                crate::config::CertTarget {
                    host: "down.example".into(),
                    port: 1,
                    address: Some("127.0.0.1".into()),
                    ca_file: None,
                },
            ],
            ..Default::default()
        };
        let c = collect(&cfg, vec![]).await;
        let listed = c.rows.iter().find(|r| r.host == "listed.example").unwrap();
        assert_eq!(listed.status, Tone::Ok, "{listed:?}");
        assert_eq!(listed.sources, ["listed", "integration"]);
        assert_eq!(
            listed
                .chain
                .iter()
                .map(|c| c.role.as_str())
                .collect::<Vec<_>>(),
            ["leaf", "intermediate", "root"]
        );
        let down = c.rows.iter().find(|r| r.host == "down.example").unwrap();
        assert_eq!(down.status, Tone::Warn);
        assert!(down.reasons[0].starts_with("unreachable: "), "{down:?}");
        // rows sort worst first
        assert!(c.rows.windows(2).all(|w| w[0].status >= w[1].status));
        let _ = std::fs::remove_dir_all(dir);
    }
}

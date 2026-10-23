//! Roles (RFC 0016): what a read-only user may see. Queries of pages the role
//! lacks are refused; allowed results are filtered to its namespaces and
//! Proxmox tags, and lose their links to other UIs.

use std::collections::HashSet;

use serde_json::Value;

use crate::config::Role;

/// The page a named query belongs to. `None`: not for roles at all.
pub fn page_of(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "pve" | "spark-pve" => "infra",
        "k8s" | "spark-k8s" => "workloads",
        "flux" => "gitops",
        "alerts" => "alerts",
        "security" => "security",
        "tailscale" => "tailscale",
        "github" => "workflows",
        "openbao" | "cnpg" => "services",
        "healthchecks" => "outage",
        // Helm releases are read from Secrets, and Argo CD would need its own
        // RBAC for a role's ServiceAccount: never for a role
        _ => return None,
    })
}

pub fn allows(role: &Role, kind: &str) -> bool {
    page_of(kind).is_some_and(|p| role.pages.iter().any(|x| x == p))
}

/// Removes every `link`-like field, at any depth: deep links lead to UIs
/// outside the role's view.
fn strip_links(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|k, _| !matches!(k.as_str(), "link" | "ui_url" | "commit_link"));
            m.values_mut().for_each(strip_links);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_links),
        _ => {}
    }
}

/// An IP address, with or without a port (`10.0.0.1`, `10.0.0.1:8080`,
/// `fd00::1`, `[fd00::1]:80`).
fn is_ip(s: &str) -> bool {
    use std::net::{IpAddr, SocketAddr};
    s.parse::<IpAddr>().is_ok() || s.parse::<SocketAddr>().is_ok()
}

/// Replaces IP addresses in free text with `<ip>`: event messages, Proxmox
/// tags and the like name hosts and pods a public view must not show.
pub fn redact_ips(s: &str) -> String {
    let addr_char = |c: char| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '[' | ']');
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    // The run is ASCII. A trailing ':' or '.' (end of a sentence) sticks to it,
    // and so can hex letters before the address ("de10.0.0.1"): try every start.
    let flush = |run: &mut String, out: &mut String| {
        let end = run.trim_end_matches([':', '.']).len();
        let ok = |s: &str| s.contains(['.', ':']) && is_ip(s);
        let found = (0..end).find(|&i| {
            let t = &run[i..end];
            ok(t) || ok(t.trim_matches(['[', ']']))
        });
        match found {
            Some(i) => {
                out.push_str(&run[..i]);
                out.push_str("<ip>");
                out.push_str(&run[end..]);
            }
            None => out.push_str(run),
        }
        run.clear();
    };
    for c in s.chars() {
        if addr_char(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `redact_ips` on every string, at any depth.
fn redact_all(v: &mut Value) {
    match v {
        Value::String(s) => *s = redact_ips(s),
        Value::Object(m) => m.values_mut().for_each(redact_all),
        Value::Array(a) => a.iter_mut().for_each(redact_all),
        _ => {}
    }
}

fn retain_rows(v: &mut Value, key: &str, keep: impl Fn(&Value) -> bool) {
    if let Some(rows) = v[key].as_array_mut() {
        rows.retain(keep);
    }
}

/// Guests the role may see: those carrying one of its tags.
pub fn visible_vmids(role: &Role, pve: &Value) -> HashSet<u64> {
    pve["guests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| {
            g["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|t| role.proxmox_tags.iter().any(|r| t == r.as_str()))
        })
        .filter_map(|g| g["vmid"].as_u64())
        .collect()
}

/// Filters a query result for a role. `pve` is the role's Proxmox overview,
/// needed to filter the guest sparklines.
pub fn filter(role: &Role, kind: &str, mut data: Value, pve: Option<&Value>) -> Value {
    let ns_ok = |v: &Value| {
        v.as_str()
            .is_some_and(|n| role.namespaces.iter().any(|r| r == n))
    };
    match kind {
        "k8s" => {
            for key in ["workloads", "unhealthy_pods", "events"] {
                retain_rows(&mut data, key, |r| ns_ok(&r["namespace"]));
            }
            data["namespaces"] = role.namespaces.len().into();
        }
        "flux" => {
            if let Some(items) = data.as_array_mut() {
                // shown only when everything it deploys is in allowed namespaces
                items.retain(|i| {
                    let ns = i["namespaces"].as_array();
                    ns.is_some_and(|ns| !ns.is_empty() && ns.iter().all(&ns_ok))
                });
            }
        }
        "pve" => {
            let vmids = visible_vmids(role, &data);
            retain_rows(&mut data, "guests", |g| {
                g["vmid"].as_u64().is_some_and(|id| vmids.contains(&id))
            });
            // tags often carry the guest's address: drop those outright
            for g in data["guests"].as_array_mut().into_iter().flatten() {
                if let Some(tags) = g["tags"].as_array_mut() {
                    tags.retain(|t| !t.as_str().is_some_and(is_ip));
                }
            }
            retain_rows(&mut data, "ha", |h| {
                h["sid"]
                    .as_str()
                    .and_then(|s| s.rsplit(':').next()?.parse::<u64>().ok())
                    .is_some_and(|id| vmids.contains(&id))
            });
            // storage names and task history say more about the lab than a demo should
            data["storage"] = Value::Array(Vec::new());
            data["failed_tasks"] = Value::Array(Vec::new());
            data["can_power"] = false.into();
        }
        "spark-pve" => {
            let vmids = pve.map(|p| visible_vmids(role, p)).unwrap_or_default();
            if let Some(g) = data["guests"].as_object_mut() {
                g.retain(|id, _| id.parse::<u64>().is_ok_and(|id| vmids.contains(&id)));
            }
        }
        _ => {}
    }
    strip_links(&mut data);
    redact_all(&mut data);
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn demo() -> Role {
        Role {
            pages: vec!["infra".into(), "workloads".into(), "gitops".into()],
            namespaces: vec!["apps".into()],
            proxmox_tags: vec!["demo".into()],
            kubernetes_service_account: Some("homie/homie-demo".into()),
        }
    }

    #[test]
    fn addresses_are_redacted() {
        assert_eq!(
            redact_ips(
                r#"Readiness probe failed: Get "http://10.0.1.185:8080/healthz": dial tcp 10.0.1.185:8080: connect: refused"#
            ),
            r#"Readiness probe failed: Get "http://<ip>/healthz": dial tcp <ip>: connect: refused"#
        );
        assert_eq!(
            redact_ips("node at fd00::1 and [fd00::2]:443."),
            "node at <ip> and <ip>."
        );
        assert_eq!(
            redact_ips("peer [10.0.0.1] and [fd00::2], id de10.0.0.9"),
            "peer <ip> and <ip>, id de<ip>"
        );
        assert_eq!(redact_ips("error:: retry"), "error:: retry");
        // hashes, versions and times stay
        for keep in [
            "main@sha1:f5acd8e78b29be79070d7282eacf661cc74df011",
            "v1.36.4+k0s",
            "12:30:05",
            "litellm-helm@1.1.3",
            "deadbeef",
        ] {
            assert_eq!(redact_ips(keep), keep);
        }
        let pve = json!({"guests": [{"vmid": 1, "tags": ["10.0.0.40", "demo", "k8s"]}]});
        let f = filter(&demo(), "pve", pve, None);
        assert_eq!(f["guests"][0]["tags"], json!(["demo", "k8s"]));
    }

    #[test]
    fn pages() {
        let r = demo();
        assert!(allows(&r, "k8s") && allows(&r, "flux") && allows(&r, "spark-pve"));
        assert!(!allows(&r, "alerts") && !allows(&r, "security") && !allows(&r, "helm"));
        assert!(!allows(&r, "argocd"), "Argo CD is never for a role");
        assert!(!allows(&r, "logs") && !allows(&r, "nonsense"));
    }

    #[test]
    fn kubernetes_rows() {
        let k8s = json!({
            "nodes": [{"name": "n1", "link": "https://headlamp/n1"}],
            "namespaces": 30,
            "workloads": [{"namespace": "apps", "name": "web", "link": "x"}, {"namespace": "kube-system", "name": "coredns"}],
            "unhealthy_pods": [{"namespace": "openbao", "name": "bao-0"}],
            "events": [{"namespace": "apps"}, {"namespace": "default"}],
        });
        let f = filter(&demo(), "k8s", k8s, None);
        assert_eq!(
            f["workloads"],
            json!([{"namespace": "apps", "name": "web"}])
        );
        assert_eq!(f["unhealthy_pods"], json!([]));
        assert_eq!(f["events"], json!([{"namespace": "apps"}]));
        assert_eq!(f["nodes"], json!([{"name": "n1"}]));
        assert_eq!(f["namespaces"], 1);
    }

    #[test]
    fn flux_by_deployed_namespaces() {
        let flux = json!([
            {"name": "apps", "namespaces": ["apps"]},
            {"name": "mixed", "namespaces": ["apps", "kube-system"]},
            {"name": "source", "namespaces": []},
            {"name": "old"},
        ]);
        let f = filter(&demo(), "flux", flux, None);
        assert_eq!(f, json!([{"name": "apps", "namespaces": ["apps"]}]));
    }

    #[test]
    fn proxmox_by_tag() {
        let pve = json!({
            "guests": [{"vmid": 1, "tags": ["demo", "k0s"]}, {"vmid": 2, "tags": ["k0s"]}, {"vmid": 3, "tags": []}],
            "ha": [{"sid": "vm:1"}, {"sid": "vm:2"}],
            "storage": [{"storage": "local"}],
            "failed_tasks": [{"upid": "x"}],
            "can_power": true,
            "ui_url": "https://pve:8006",
        });
        let f = filter(&demo(), "pve", pve.clone(), None);
        assert_eq!(f["guests"], json!([{"vmid": 1, "tags": ["demo", "k0s"]}]));
        assert_eq!(f["ha"], json!([{"sid": "vm:1"}]));
        assert_eq!(
            (f["storage"].clone(), f["failed_tasks"].clone()),
            (json!([]), json!([]))
        );
        assert_eq!(f["can_power"], false);
        assert!(f.get("ui_url").is_none());
        let sparks = json!({"nodes": {"pve-01": {}}, "guests": {"1": {}, "2": {}}});
        let s = filter(&demo(), "spark-pve", sparks, Some(&pve));
        assert_eq!(s["guests"], json!({"1": {}}));
    }
}

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
        "flux" | "argocd" => "gitops",
        "alerts" => "alerts",
        "security" => "security",
        "tailscale" => "tailscale",
        "github" => "workflows",
        "openbao" | "cnpg" => "services",
        "healthchecks" => "outage",
        // Helm releases are read from Secrets: never for a role
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
        "argocd" => {
            if let Some(apps) = data.as_array_mut() {
                apps.retain(|a| ns_ok(&a["dest_namespace"]));
            }
        }
        "pve" => {
            let vmids = visible_vmids(role, &data);
            retain_rows(&mut data, "guests", |g| {
                g["vmid"].as_u64().is_some_and(|id| vmids.contains(&id))
            });
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
    fn pages() {
        let r = demo();
        assert!(allows(&r, "k8s") && allows(&r, "flux") && allows(&r, "spark-pve"));
        assert!(!allows(&r, "alerts") && !allows(&r, "security") && !allows(&r, "helm"));
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

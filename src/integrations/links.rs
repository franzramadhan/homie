//! Deep links to other Kubernetes UIs from URL templates (RFC 0007).
//! `headlamp_url` fills every template that is not set explicitly.

use crate::config::LinkTemplates;

pub struct Links {
    node: Option<String>,
    pod: Option<String>,
    workload: Option<String>,
    flux: Option<String>,
    cluster: String,
}

/// Percent-encodes everything outside the URL path's unreserved set, so a
/// name can never add a path segment, a query or a fragment.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn expand(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), &encode(v));
    }
    out
}

/// Flux kind to its lowercase plural and the Headlamp Flux plugin group.
fn flux_names(kind: &str) -> (String, &'static str) {
    let group = match kind {
        "Kustomization" => "kustomize",
        "HelmRelease" => "helm",
        _ => "source",
    };
    let lower = kind.to_lowercase();
    let plural = if lower.ends_with('y') {
        format!("{}ies", &lower[..lower.len() - 1])
    } else {
        format!("{lower}s")
    };
    (plural, group)
}

impl Links {
    pub fn new(t: &LinkTemplates, headlamp_url: Option<&str>, cluster: &str) -> Self {
        let headlamp = headlamp_url.map(|u| u.trim_end_matches('/').to_string());
        let default = |path: &str| {
            headlamp
                .as_ref()
                .map(|b| format!("{b}/c/{{cluster}}/{path}"))
        };
        Self {
            node: t.node.clone().or_else(|| default("nodes/{name}")),
            pod: t.pod.clone().or_else(|| default("pods/{namespace}/{name}")),
            workload: t
                .workload
                .clone()
                .or_else(|| default("{plural}/{namespace}/{name}")),
            flux: t
                .flux
                .clone()
                .or_else(|| default("flux/{group}/{plural}/{namespace}/{name}")),
            cluster: cluster.to_string(),
        }
    }

    pub fn node(&self, name: &str) -> Option<String> {
        Some(expand(
            self.node.as_ref()?,
            &[
                ("cluster", &self.cluster),
                ("name", name),
                ("kind", "Node"),
                ("plural", "nodes"),
            ],
        ))
    }

    pub fn pod(&self, namespace: &str, name: &str) -> Option<String> {
        let vars = [
            ("cluster", self.cluster.as_str()),
            ("namespace", namespace),
            ("name", name),
            ("kind", "Pod"),
            ("plural", "pods"),
        ];
        Some(expand(self.pod.as_ref()?, &vars))
    }

    pub fn workload(
        &self,
        kind: &str,
        plural: &str,
        namespace: &str,
        name: &str,
    ) -> Option<String> {
        let vars = [
            ("cluster", self.cluster.as_str()),
            ("namespace", namespace),
            ("name", name),
            ("kind", kind),
            ("plural", plural),
        ];
        Some(expand(self.workload.as_ref()?, &vars))
    }

    pub fn flux(&self, kind: &str, namespace: &str, name: &str) -> Option<String> {
        let (plural, group) = flux_names(kind);
        let vars = [
            ("cluster", self.cluster.as_str()),
            ("namespace", namespace),
            ("name", name),
            ("kind", kind),
            ("plural", plural.as_str()),
            ("group", group),
        ];
        Some(expand(self.flux.as_ref()?, &vars))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headlamp_defaults() {
        let l = Links::new(
            &LinkTemplates::default(),
            Some("https://headlamp.example/"),
            "main",
        );
        assert_eq!(
            l.node("n1").unwrap(),
            "https://headlamp.example/c/main/nodes/n1"
        );
        assert_eq!(
            l.pod("ns", "p").unwrap(),
            "https://headlamp.example/c/main/pods/ns/p"
        );
        assert_eq!(
            l.workload("Deployment", "deployments", "ns", "d").unwrap(),
            "https://headlamp.example/c/main/deployments/ns/d"
        );
        assert_eq!(
            l.flux("Kustomization", "flux-system", "apps").unwrap(),
            "https://headlamp.example/c/main/flux/kustomize/kustomizations/flux-system/apps"
        );
        assert_eq!(
            l.flux("GitRepository", "flux-system", "repo").unwrap(),
            "https://headlamp.example/c/main/flux/source/gitrepositories/flux-system/repo"
        );
    }

    #[test]
    fn templates_override_and_encode() {
        let t = LinkTemplates {
            pod: Some("https://lens.example/{namespace}/pods/{name}?k={kind}".into()),
            ..LinkTemplates::default()
        };
        let l = Links::new(&t, None, "c1");
        assert_eq!(
            l.pod("a b", "x/../y").unwrap(),
            "https://lens.example/a%20b/pods/x%2F..%2Fy?k=Pod"
        );
        assert!(l.node("n").is_none(), "no template and no headlamp_url");
        assert_eq!(flux_names("HelmRepository").0, "helmrepositories");
        assert_eq!(flux_names("OCIRepository").0, "ocirepositories");
    }
}

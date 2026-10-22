//! Argo CD Applications (RFC 0005), read through the Kubernetes integration
//! (same identity rules, no Argo CD API token), and the refresh action (an
//! annotation, the same as the Argo CD UI's Refresh button).

use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ListParams, Patch, PatchParams},
    core::GroupVersionKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Result;

pub(crate) fn resource() -> ApiResource {
    ApiResource::from_gvk(&GroupVersionKind::gvk(
        "argoproj.io",
        "v1alpha1",
        "Application",
    ))
}

#[derive(Serialize)]
pub struct App {
    namespace: String,
    name: String,
    project: String,
    source: String,
    target_revision: String,
    destination: String,
    sync: String,
    health: String,
    revision: String,
    operation: String,
    message: String,
    reconciled_at: String,
    link: Option<String>,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub namespace: String,
    pub name: String,
    #[serde(default)]
    pub hard: bool,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn app(o: &DynamicObject, argocd_url: Option<&str>) -> App {
    let spec = &o.data["spec"];
    let status = &o.data["status"];
    // single source, or the first of `sources`
    let src = if spec["source"].is_object() {
        &spec["source"]
    } else {
        &spec["sources"][0]
    };
    let path = src["path"]
        .as_str()
        .or_else(|| src["chart"].as_str())
        .unwrap_or_default();
    let dest = &spec["destination"];
    let namespace = o.metadata.namespace.clone().unwrap_or_default();
    let name = o.metadata.name.clone().unwrap_or_default();
    let message = status["operationState"]["message"]
        .as_str()
        .or_else(|| status["conditions"][0]["message"].as_str())
        .unwrap_or_default();
    App {
        link: argocd_url.map(|u| {
            format!(
                "{}/applications/{namespace}/{name}",
                u.trim_end_matches('/')
            )
        }),
        project: text(&spec["project"]),
        source: if path.is_empty() {
            text(&src["repoURL"])
        } else {
            format!("{} {path}", text(&src["repoURL"]))
        },
        target_revision: text(&src["targetRevision"]),
        destination: format!(
            "{}/{}",
            dest["name"]
                .as_str()
                .or_else(|| dest["server"].as_str())
                .unwrap_or_default(),
            text(&dest["namespace"])
        ),
        sync: status["sync"]["status"]
            .as_str()
            .unwrap_or("Unknown")
            .to_string(),
        health: status["health"]["status"]
            .as_str()
            .unwrap_or("Unknown")
            .to_string(),
        revision: status["sync"]["revision"]
            .as_str()
            .or_else(|| status["sync"]["revisions"][0].as_str())
            .unwrap_or_default()
            .to_string(),
        operation: text(&status["operationState"]["phase"]),
        message: message.to_string(),
        reconciled_at: text(&status["reconciledAt"]),
        namespace,
        name,
    }
}

/// Every Application. Empty when Argo CD is not installed.
pub async fn list(client: Client, argocd_url: Option<&str>) -> Result<Vec<App>> {
    let items = match Api::<DynamicObject>::all_with(client, &resource())
        .list(&ListParams::default())
        .await
    {
        Ok(l) => l.items,
        Err(kube::Error::Api(s)) if s.code == 404 => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut apps: Vec<App> = items.iter().map(|o| app(o, argocd_url)).collect();
    apps.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(apps)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

/// Asks the application controller to compare with git now.
pub async fn refresh(client: Client, req: &RefreshRequest) -> Result<String> {
    if !valid_name(&req.namespace) || !valid_name(&req.name) {
        return Err("invalid namespace or name".into());
    }
    let mode = if req.hard { "hard" } else { "normal" };
    let api: Api<DynamicObject> = Api::namespaced_with(client, &req.namespace, &resource());
    let patch = json!({"metadata": {"annotations": {"argocd.argoproj.io/refresh": mode}}});
    api.patch(&req.name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(format!(
        "Application/{}/{} ({mode} refresh)",
        req.namespace, req.name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_application_status() {
        let mut o = DynamicObject::new("guestbook", &resource()).within("argocd");
        o.data = json!({
            "spec": {"project": "default",
                     "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps", "path": "guestbook", "targetRevision": "HEAD"},
                     "destination": {"server": "https://kubernetes.default.svc", "namespace": "guestbook"}},
            "status": {"sync": {"status": "OutOfSync", "revision": "abc123"},
                       "health": {"status": "Degraded"},
                       "operationState": {"phase": "Failed", "message": "one or more objects failed"},
                       "reconciledAt": "2026-10-22T08:00:00Z"}
        });
        let a = app(&o, Some("https://argocd.example/"));
        assert_eq!(
            (a.sync.as_str(), a.health.as_str(), a.operation.as_str()),
            ("OutOfSync", "Degraded", "Failed")
        );
        assert_eq!(
            a.source,
            "https://github.com/argoproj/argocd-example-apps guestbook"
        );
        assert_eq!(a.destination, "https://kubernetes.default.svc/guestbook");
        assert_eq!(
            a.link.as_deref(),
            Some("https://argocd.example/applications/argocd/guestbook")
        );
        // multi-source apps and empty status
        o.data = json!({"spec": {"sources": [{"repoURL": "oci://charts", "chart": "x"}], "destination": {"name": "in-cluster"}}});
        let a = app(&o, None);
        assert_eq!(
            (a.sync.as_str(), a.health.as_str(), a.source.as_str()),
            ("Unknown", "Unknown", "oci://charts x")
        );
        assert!(!valid_name("a/b"));
    }
}

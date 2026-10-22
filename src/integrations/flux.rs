//! Flux v2: sources, Kustomizations and HelmReleases, and the reconcile
//! action (an annotation, the same as `flux reconcile`).

use futures::future::join_all;
use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ListParams, Patch, PatchParams},
    core::GroupVersionKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Result;

pub(crate) const KINDS: [(&str, &str, &str); 7] = [
    ("source.toolkit.fluxcd.io", "v1", "GitRepository"),
    ("source.toolkit.fluxcd.io", "v1", "OCIRepository"),
    ("source.toolkit.fluxcd.io", "v1", "HelmRepository"),
    ("source.toolkit.fluxcd.io", "v1", "HelmChart"),
    ("source.toolkit.fluxcd.io", "v1", "Bucket"),
    ("kustomize.toolkit.fluxcd.io", "v1", "Kustomization"),
    ("helm.toolkit.fluxcd.io", "v2", "HelmRelease"),
];

pub(crate) fn resource(kind: &str) -> Option<ApiResource> {
    let (group, version, kind) = KINDS.iter().find(|k| k.2 == kind)?;
    Some(ApiResource::from_gvk(&GroupVersionKind::gvk(
        group, version, kind,
    )))
}

#[derive(Serialize)]
pub struct Item {
    kind: String,
    namespace: String,
    name: String,
    ready: String,
    reason: String,
    message: String,
    revision: String,
    suspended: bool,
    since: String,
    link: Option<String>,
    /// where it deploys (RFC 0016 role filtering): a Kustomization's
    /// inventory, a HelmRelease's target; empty for sources
    namespaces: Vec<String>,
}

#[derive(Deserialize)]
pub struct ReconcileRequest {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    #[serde(default)]
    pub with_source: bool,
}

/// Every Flux object. Kinds whose CRD is not installed are skipped.
/// Every Flux object, with deep links from `links`.
/// `deploying_only`: Kustomizations and HelmReleases only, for a role
/// (RFC 0016): sources are never shown to one, and its ServiceAccount
/// need not read them.
pub async fn list_linked(
    client: Client,
    links: &super::links::Links,
    deploying_only: bool,
) -> Result<Vec<Item>> {
    let mut items = list(client, deploying_only).await?;
    for i in &mut items {
        i.link = links.flux(&i.kind, &i.namespace, &i.name);
    }
    Ok(items)
}

async fn list(client: Client, deploying_only: bool) -> Result<Vec<Item>> {
    let kinds = KINDS
        .iter()
        .filter(|k| !deploying_only || matches!(k.2, "Kustomization" | "HelmRelease"));
    let lists = join_all(kinds.map(|(_, _, kind)| {
        let client = client.clone();
        async move {
            let api: Api<DynamicObject> = Api::all_with(client, &resource(kind).unwrap());
            match api.list(&ListParams::default()).await {
                Ok(l) => Ok(l
                    .items
                    .into_iter()
                    .map(|o| item(kind, o))
                    .collect::<Vec<_>>()),
                Err(kube::Error::Api(s)) if s.code == 404 => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        }
    }))
    .await;
    let mut items = Vec::new();
    for l in lists {
        items.extend(l?);
    }
    Ok(items)
}

fn item(kind: &str, o: DynamicObject) -> Item {
    let status = &o.data["status"];
    let ready = status["conditions"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["type"] == "Ready"))
        .cloned()
        .unwrap_or(Value::Null);
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    Item {
        kind: kind.to_string(),
        namespace: o.metadata.namespace.clone().unwrap_or_default(),
        name: o.metadata.name.clone().unwrap_or_default(),
        ready: ready["status"].as_str().unwrap_or("Unknown").to_string(),
        reason: text(&ready["reason"]),
        message: text(&ready["message"]),
        revision: status["lastAppliedRevision"]
            .as_str()
            .or_else(|| status["artifact"]["revision"].as_str())
            .unwrap_or_default()
            .to_string(),
        suspended: o.data["spec"]["suspend"].as_bool().unwrap_or(false),
        since: text(&ready["lastTransitionTime"]),
        link: None,
        namespaces: deploys_into(kind, &o),
    }
}

/// Namespaces an object deploys into. Inventory ids are
/// `namespace_name_group_kind`; cluster-scoped entries have no namespace.
fn deploys_into(kind: &str, o: &DynamicObject) -> Vec<String> {
    let own = o.metadata.namespace.clone().unwrap_or_default();
    let mut ns: Vec<String> = match kind {
        "Kustomization" => o.data["status"]["inventory"]["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str()?.split('_').next())
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect(),
        "HelmRelease" => vec![
            o.data["spec"]["targetNamespace"]
                .as_str()
                .map(String::from)
                .unwrap_or(own),
        ],
        _ => Vec::new(),
    };
    ns.sort();
    ns.dedup();
    ns
}

/// DNS-1123 names only: these go into API paths.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

async fn annotate(
    client: &Client,
    kind: &str,
    namespace: &str,
    name: &str,
    now: &str,
) -> Result<()> {
    let ar = resource(kind).ok_or_else(|| format!("{kind} is not a Flux kind"))?;
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
    let patch = json!({"metadata": {"annotations": {"reconcile.fluxcd.io/requestedAt": now}}});
    api.patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

/// The source a Kustomization or HelmRelease reads from, as (kind, namespace, name).
fn source_of(kind: &str, o: &DynamicObject) -> Option<(String, String, String)> {
    let ns = o.metadata.namespace.clone().unwrap_or_default();
    let reference = |r: &Value| {
        Some((
            r["kind"].as_str()?.to_string(),
            r["namespace"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| ns.clone()),
            r["name"].as_str()?.to_string(),
        ))
    };
    match kind {
        "Kustomization" => reference(&o.data["spec"]["sourceRef"]),
        // a chart template produces a HelmChart named in status; chartRef points at the source directly
        "HelmRelease" => match o.data["status"]["helmChart"]
            .as_str()
            .and_then(|s| s.split_once('/'))
        {
            Some((chart_ns, chart)) => Some(("HelmChart".into(), chart_ns.into(), chart.into())),
            None => reference(&o.data["spec"]["chartRef"]),
        },
        _ => None,
    }
}

pub async fn reconcile(client: Client, req: &ReconcileRequest) -> Result<String> {
    if resource(&req.kind).is_none() {
        return Err(format!("{} is not a Flux kind", req.kind).into());
    }
    if !valid_name(&req.namespace) || !valid_name(&req.name) {
        return Err("invalid namespace or name".into());
    }
    let now = jiff::Timestamp::now().to_string();
    let mut done = Vec::new();
    if req.with_source {
        let api: Api<DynamicObject> = Api::namespaced_with(
            client.clone(),
            &req.namespace,
            &resource(&req.kind).unwrap(),
        );
        let obj = api.get(&req.name).await?;
        if let Some((kind, ns, name)) = source_of(&req.kind, &obj) {
            annotate(&client, &kind, &ns, &name, &now).await?;
            done.push(format!("{kind}/{ns}/{name}"));
        }
    }
    annotate(&client, &req.kind, &req.namespace, &req.name, &now).await?;
    done.push(format!("{}/{}/{}", req.kind, req.namespace, req.name));
    Ok(done.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{kube_client, list, serve};
    use axum::{
        Json, Router,
        extract::Path,
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::get,
    };
    use std::sync::{Arc, Mutex};

    fn obj(kind: &str, data: Value) -> DynamicObject {
        let mut o = DynamicObject::new("x", &resource(kind).unwrap()).within("apps");
        o.data = data;
        o
    }

    /// Answers every Flux list with `answer(plural)`.
    fn lists(answer: fn(&str) -> Response) -> Router {
        Router::new().route(
            "/apis/{group}/{version}/{plural}",
            get(
                move |Path((_, _, plural)): Path<(String, String, String)>| async move {
                    answer(&plural)
                },
            ),
        )
    }

    fn installed(plural: &str) -> Response {
        let items = match plural {
            "gitrepositories" => {
                json!([{"metadata": {"name": "flux-system", "namespace": "flux-system"},
                "status": {"artifact": {"revision": "main@sha1:abc"},
                           "conditions": [{"type": "Ready", "status": "True", "reason": "Succeeded",
                                           "message": "stored artifact", "lastTransitionTime": "2026-10-22T08:00:00Z"}]}}])
            }
            "kustomizations" => json!([{"metadata": {"name": "apps", "namespace": "flux-system"},
                "spec": {"suspend": true},
                "status": {"lastAppliedRevision": "main@sha1:abc",
                           "conditions": [{"type": "Healthy", "status": "True"},
                                          {"type": "Ready", "status": "False", "reason": "BuildFailed"}],
                           "inventory": {"entries": [
                               {"id": "web_web_apps_Deployment", "v": "v1"},
                               {"id": "_web__Namespace", "v": "v1"},
                               {"id": "db_pg_apps_StatefulSet", "v": "v1"},
                               {"id": "web_svc__Service", "v": "v1"}]}}}]),
            "helmreleases" => json!([
                {"metadata": {"name": "pihole", "namespace": "flux-system"}, "spec": {"targetNamespace": "dns"}},
                {"metadata": {"name": "grafana", "namespace": "monitoring"}, "spec": {}}]),
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        Json(list("Item", items)).into_response()
    }

    fn sources_forbidden(plural: &str) -> Response {
        match plural {
            "kustomizations" | "helmreleases" => installed(plural),
            _ => StatusCode::FORBIDDEN.into_response(),
        }
    }

    #[tokio::test]
    async fn lists_flux_objects_with_links() {
        let client = kube_client(&serve(lists(installed)).await);
        let links = super::super::links::Links::new(
            &Default::default(),
            Some("https://headlamp.example"),
            "main",
        );
        let items = list_linked(client, &links, false).await.unwrap();
        let summary: Vec<_> = items
            .iter()
            .map(|i| {
                (
                    i.kind.as_str(),
                    i.name.as_str(),
                    i.ready.as_str(),
                    i.namespaces.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("GitRepository", "flux-system", "True", vec![]),
                (
                    "Kustomization",
                    "apps",
                    "False",
                    vec!["db".into(), "web".into()]
                ),
                ("HelmRelease", "pihole", "Unknown", vec!["dns".into()]),
                (
                    "HelmRelease",
                    "grafana",
                    "Unknown",
                    vec!["monitoring".into()]
                ),
            ]
        );
        let (git, ks) = (&items[0], &items[1]);
        assert_eq!(git.revision, "main@sha1:abc");
        assert_eq!(
            (git.reason.as_str(), git.since.as_str()),
            ("Succeeded", "2026-10-22T08:00:00Z")
        );
        assert!(ks.suspended && !git.suspended);
        assert_eq!(ks.reason, "BuildFailed");
        assert_eq!(
            ks.link.as_deref(),
            Some("https://headlamp.example/c/main/flux/kustomize/kustomizations/flux-system/apps")
        );
    }

    #[tokio::test]
    async fn a_role_lists_only_what_deploys() {
        let client = kube_client(&serve(lists(sources_forbidden)).await);
        let items = super::list(client.clone(), true).await.unwrap();
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.kind != "GitRepository"));
        assert!(super::list(client, false).await.is_err());
    }

    type Patches = Arc<Mutex<Vec<(String, Value)>>>;

    /// A fake API server that answers GETs with `objects` (by name) and
    /// records every PATCH as (kind plural/namespace/name, body).
    async fn patchable(objects: Value) -> (Client, Patches) {
        let patches: Patches = Default::default();
        let log = patches.clone();
        let app =
            Router::new().route(
                "/apis/{group}/{version}/namespaces/{ns}/{plural}/{name}",
                get(
                    move |Path((_, _, _, _, name)): Path<(
                        String,
                        String,
                        String,
                        String,
                        String,
                    )>| async move {
                        match objects.get(&name) {
                            Some(o) => Json(o.clone()).into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        }
                    },
                )
                .patch(
                    move |Path((_, _, ns, plural, name)): Path<(
                        String,
                        String,
                        String,
                        String,
                        String,
                    )>,
                          body: String| async move {
                        let body: Value = serde_json::from_str(&body).unwrap();
                        log.lock()
                            .unwrap()
                            .push((format!("{plural}/{ns}/{name}"), body));
                        Json(json!({"metadata": {"name": name, "namespace": ns}}))
                    },
                ),
            );
        (kube_client(&serve(app).await), patches)
    }

    fn request(kind: &str, namespace: &str, name: &str, with_source: bool) -> ReconcileRequest {
        ReconcileRequest {
            kind: kind.into(),
            namespace: namespace.into(),
            name: name.into(),
            with_source,
        }
    }

    #[tokio::test]
    async fn reconcile_annotates() {
        let (client, patches) = patchable(json!({})).await;
        let done = reconcile(
            client,
            &request("Kustomization", "flux-system", "apps", false),
        )
        .await
        .unwrap();
        assert_eq!(done, "Kustomization/flux-system/apps");
        let patches = patches.lock().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].0, "kustomizations/flux-system/apps");
        let at = patches[0].1["metadata"]["annotations"]["reconcile.fluxcd.io/requestedAt"]
            .as_str()
            .unwrap();
        assert!(at.parse::<jiff::Timestamp>().is_ok(), "{at}");
    }

    #[tokio::test]
    async fn reconcile_with_source() {
        let (client, patches) = patchable(json!({
            "apps": {"metadata": {"name": "apps", "namespace": "flux-system"},
                     "spec": {"sourceRef": {"kind": "GitRepository", "name": "flux-system"}}},
            "loose": {"metadata": {"name": "loose", "namespace": "flux-system"}, "spec": {}},
            "odd": {"metadata": {"name": "odd", "namespace": "flux-system"},
                    "spec": {"sourceRef": {"kind": "Secret", "name": "x"}}},
        }))
        .await;
        let done = reconcile(
            client.clone(),
            &request("Kustomization", "flux-system", "apps", true),
        )
        .await
        .unwrap();
        assert_eq!(
            done,
            "GitRepository/flux-system/flux-system, Kustomization/flux-system/apps"
        );
        let paths: Vec<_> = patches
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.0.clone())
            .collect();
        assert_eq!(
            paths,
            [
                "gitrepositories/flux-system/flux-system",
                "kustomizations/flux-system/apps"
            ]
        );

        // no source to find: only the object itself
        let done = reconcile(
            client.clone(),
            &request("Kustomization", "flux-system", "loose", true),
        )
        .await
        .unwrap();
        assert_eq!(done, "Kustomization/flux-system/loose");

        let e = reconcile(
            client.clone(),
            &request("Kustomization", "flux-system", "odd", true),
        )
        .await
        .unwrap_err();
        assert_eq!(e.to_string(), "Secret is not a Flux kind");
        assert!(
            reconcile(
                client,
                &request("Kustomization", "flux-system", "gone", true)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn reconcile_validates_before_calling_the_api() {
        let (client, patches) = patchable(json!({})).await;
        let e = reconcile(client.clone(), &request("Secret", "default", "x", false))
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "Secret is not a Flux kind");
        let e = reconcile(
            client,
            &request("Kustomization", "flux-system", "../x", false),
        )
        .await
        .unwrap_err();
        assert_eq!(e.to_string(), "invalid namespace or name");
        assert!(patches.lock().unwrap().is_empty());
    }

    #[test]
    fn finds_sources() {
        let k = obj(
            "Kustomization",
            json!({"spec": {"sourceRef": {"kind": "GitRepository", "name": "flux-system", "namespace": "flux-system"}}}),
        );
        assert_eq!(
            source_of("Kustomization", &k),
            Some((
                "GitRepository".into(),
                "flux-system".into(),
                "flux-system".into()
            ))
        );
        let h = obj(
            "HelmRelease",
            json!({"status": {"helmChart": "flux-system/apps-x"}}),
        );
        assert_eq!(
            source_of("HelmRelease", &h),
            Some(("HelmChart".into(), "flux-system".into(), "apps-x".into()))
        );
        let h = obj(
            "HelmRelease",
            json!({"spec": {"chartRef": {"kind": "OCIRepository", "name": "x"}}}),
        );
        assert_eq!(
            source_of("HelmRelease", &h),
            Some(("OCIRepository".into(), "apps".into(), "x".into()))
        );
        assert_eq!(source_of("GitRepository", &h), None);
    }

    #[test]
    fn names_and_kinds() {
        assert!(valid_name("flux-system"));
        assert!(!valid_name("Flux"));
        assert!(!valid_name("a/b"));
        assert!(resource("Kustomization").is_some());
        assert!(resource("Secret").is_none());
    }
}

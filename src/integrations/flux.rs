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
pub async fn list_linked(client: Client, links: &super::links::Links) -> Result<Vec<Item>> {
    let mut items = list(client).await?;
    for i in &mut items {
        i.link = links.flux(&i.kind, &i.namespace, &i.name);
    }
    Ok(items)
}

pub async fn list(client: Client) -> Result<Vec<Item>> {
    let lists = join_all(KINDS.iter().map(|(_, _, kind)| {
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

    fn obj(kind: &str, data: Value) -> DynamicObject {
        let mut o = DynamicObject::new("x", &resource(kind).unwrap()).within("apps");
        o.data = data;
        o
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

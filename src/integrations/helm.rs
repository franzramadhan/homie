//! Helm 3 releases, read from the `helm.sh/release.v1` Secrets Helm keeps.
//! Only metadata leaves this module: values and manifests can hold secrets.

use std::{collections::HashMap, io::Read};

use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client, api::ListParams};
use serde::Serialize;
use serde_json::Value;

use crate::Result;

#[derive(Serialize, Debug, PartialEq)]
pub struct Release {
    pub name: String,
    pub namespace: String,
    pub revision: u64,
    pub status: String,
    pub chart: String,
    pub chart_version: String,
    pub app_version: String,
    pub updated: String,
}

pub async fn releases(client: Client) -> Result<Vec<Release>> {
    let secrets = Api::<Secret>::all(client)
        .list(
            &ListParams::default()
                .labels("owner=helm")
                .fields("type=helm.sh/release.v1"),
        )
        .await?;

    // Decode only the newest revision of each release: older ones are history,
    // and each decode inflates a whole manifest.
    let mut latest: HashMap<(String, String), (u64, &Secret)> = HashMap::new();
    for s in &secrets {
        let labels = s.metadata.labels.clone().unwrap_or_default();
        let (Some(name), Some(rev)) = (
            labels.get("name"),
            labels.get("version").and_then(|v| v.parse().ok()),
        ) else {
            continue;
        };
        let key = (
            s.metadata.namespace.clone().unwrap_or_default(),
            name.clone(),
        );
        if latest.get(&key).is_none_or(|(r, _)| rev > *r) {
            latest.insert(key, (rev, s));
        }
    }

    let mut out = Vec::new();
    for (_, s) in latest.values() {
        let Some(blob) = s.data.as_ref().and_then(|d| d.get("release")) else {
            continue;
        };
        match decode(&blob.0) {
            Ok(r) => out.push(r),
            Err(e) => {
                tracing::warn!(msg = "helm release undecodable", secret = ?s.metadata.name, error = %e)
            }
        }
    }
    out.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(out)
}

/// The `release` key holds base64 text of a gzipped JSON document.
pub fn decode(blob: &[u8]) -> Result<Release> {
    let raw = STANDARD.decode(blob)?;
    let json = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
        out
    } else {
        raw
    };
    let r: Value = serde_json::from_slice(&json)?;
    let meta = &r["chart"]["metadata"];
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    Ok(Release {
        name: text(&r["name"]),
        namespace: text(&r["namespace"]),
        revision: r["version"].as_u64().unwrap_or(0),
        status: text(&r["info"]["status"]),
        chart: text(&meta["name"]),
        chart_version: text(&meta["version"]),
        app_version: text(&meta["appVersion"]),
        updated: text(&r["info"]["last_deployed"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn decodes_release_without_leaking_values() {
        let doc = serde_json::json!({
            "name": "pihole", "namespace": "pihole", "version": 7,
            "info": {"status": "deployed", "last_deployed": "2026-10-17T10:00:00Z"},
            "chart": {"metadata": {"name": "pihole", "version": "2.38.0", "appVersion": "2026.07.2"}},
            "config": {"adminPassword": "hunter2"},
            "manifest": "kind: Secret"
        });
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(doc.to_string().as_bytes()).unwrap();
        let blob = STANDARD.encode(gz.finish().unwrap());

        let r = decode(blob.as_bytes()).unwrap();
        assert_eq!(r.revision, 7);
        assert_eq!(
            (r.chart.as_str(), r.chart_version.as_str()),
            ("pihole", "2.38.0")
        );
        assert!(!serde_json::to_string(&r).unwrap().contains("hunter2"));
        assert!(decode(b"not base64!").is_err());
    }

    /// A Helm release Secret; `blob` is the `release` value before the
    /// Kubernetes base64 layer.
    fn secret(ns: &str, name: &str, version: &str, blob: Option<String>) -> Value {
        let data = blob.map(|b| serde_json::json!({"release": STANDARD.encode(b)}));
        serde_json::json!({"metadata": {"name": format!("sh.helm.release.v1.{name}.v{version}"), "namespace": ns,
                                        "labels": {"owner": "helm", "name": name, "version": version}},
                           "type": "helm.sh/release.v1", "data": data})
    }

    fn release(ns: &str, name: &str, version: u64, gzip: bool) -> String {
        let doc = serde_json::json!({"name": name, "namespace": ns, "version": version,
                                     "info": {"status": "deployed"}, "chart": {"metadata": {"name": name}}});
        let raw = if gzip {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(doc.to_string().as_bytes()).unwrap();
            gz.finish().unwrap()
        } else {
            doc.to_string().into_bytes()
        };
        STANDARD.encode(raw)
    }

    #[tokio::test]
    async fn lists_the_newest_revision_of_each_release() {
        use crate::testkit::{json, kube_client, list, serve};
        let secrets = serde_json::json!([
            secret("web", "site", "1", Some(release("web", "site", 1, true))),
            secret("web", "site", "3", Some(release("web", "site", 3, true))),
            secret("web", "site", "2", Some(release("web", "site", 2, true))),
            secret("dns", "pihole", "1", Some(release("dns", "pihole", 1, false))),
            secret("dns", "broken", "1", Some("not base64!".into())),
            secret("dns", "empty", "1", None),
            {"metadata": {"name": "unlabelled", "namespace": "dns"}},
        ]);
        let url = serve(json("/api/v1/secrets", list("Secret", secrets))).await;
        let out = releases(kube_client(&url)).await.unwrap();
        let got: Vec<_> = out
            .iter()
            .map(|r| (r.namespace.as_str(), r.name.as_str(), r.revision))
            .collect();
        assert_eq!(got, [("dns", "pihole", 1), ("web", "site", 3)]);
        assert_eq!(out[0].status, "deployed");

        let url = serve(axum::Router::new()).await;
        assert!(releases(kube_client(&url)).await.is_err());
    }
}

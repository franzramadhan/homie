//! Where the workflow picker saves watches (RFC 0015): a ConfigMap in
//! Homie's own namespace when it runs in a cluster, else a file in
//! `state_dir`. Config watches are never written here.

use std::path::{Path, PathBuf};

use k8s_openapi::api::core::v1::ConfigMap;
use kube::{Api, api::PostParams};

use crate::{
    Result,
    config::{Watch, WatchStore},
};

pub const CONFIGMAP: &str = "homie-github-watch";
const KEY: &str = "watch.json";
const NAMESPACE_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/namespace";

pub enum Store {
    Off,
    /// watches live in the Homie session, never here (RFC 0017)
    Session,
    File(PathBuf),
    ConfigMap(Api<ConfigMap>),
}

impl Store {
    pub async fn new(kind: WatchStore, state_dir: Option<&Path>) -> Result<Self> {
        let in_cluster = Path::new(NAMESPACE_FILE).exists();
        let file = || -> Result<Self> {
            let dir = state_dir.ok_or("github.store = \"file\" needs state_dir")?;
            Ok(Self::File(dir.join("github-watch.json")))
        };
        Ok(match kind {
            WatchStore::Off => Self::Off,
            WatchStore::Session => Self::Session,
            WatchStore::File => file()?,
            WatchStore::Configmap => Self::configmap().await?,
            WatchStore::Auto if in_cluster => Self::configmap().await?,
            WatchStore::Auto if state_dir.is_some() => file()?,
            WatchStore::Auto => Self::Off,
        })
    }

    /// Homie's own ServiceAccount, never a user's token: the list is shared.
    async fn configmap() -> Result<Self> {
        let ns = std::fs::read_to_string(NAMESPACE_FILE)
            .map_err(|e| format!("github.store = \"configmap\" runs in a cluster only: {e}"))?;
        let client = kube::Client::try_from(kube::Config::incluster()?)?;
        Ok(Self::ConfigMap(Api::namespaced(client, ns.trim())))
    }

    pub fn name(&self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Session => Some("session"),
            Self::File(_) => Some("file"),
            Self::ConfigMap(_) => Some("configmap"),
        }
    }

    pub async fn load(&self) -> Result<Vec<Watch>> {
        let json = match self {
            Self::Off | Self::Session => return Ok(Vec::new()),
            Self::File(p) => match tokio::fs::read_to_string(p).await {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(format!("{}: {e}", p.display()).into()),
            },
            Self::ConfigMap(api) => match api.get_opt(CONFIGMAP).await? {
                Some(cm) => cm
                    .data
                    .and_then(|mut d| d.remove(KEY))
                    .unwrap_or_else(|| "[]".into()),
                None => return Ok(Vec::new()),
            },
        };
        Ok(serde_json::from_str(&json)?)
    }

    pub async fn save(&self, watch: &[Watch]) -> Result<()> {
        let json = serde_json::to_string_pretty(watch)?;
        match self {
            Self::Off => Err("saving watches is turned off (github.store = \"off\")".into()),
            Self::Session => Err("session watches are kept per session, not in a store".into()),
            Self::File(p) => {
                let tmp = p.with_extension("json.tmp");
                tokio::fs::write(&tmp, json).await?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                        .await?;
                }
                tokio::fs::rename(&tmp, p).await?;
                Ok(())
            }
            Self::ConfigMap(api) => {
                let mut cm = api.get_opt(CONFIGMAP).await?.unwrap_or_default();
                let exists = cm.metadata.resource_version.is_some();
                cm.metadata.name = Some(CONFIGMAP.into());
                cm.data = Some([(KEY.to_string(), json)].into());
                // replace carries the resourceVersion: a concurrent save fails instead of being lost
                if exists {
                    api.replace(CONFIGMAP, &PostParams::default(), &cm).await?;
                } else {
                    api.create(&PostParams::default(), &cm).await?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use axum::{
        Json, Router,
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn file_round_trip() {
        let dir = std::env::temp_dir().join(format!("homie-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::new(WatchStore::Auto, Some(&dir)).await.unwrap();
        assert_eq!(store.name(), Some("file"));
        assert!(store.load().await.unwrap().is_empty());
        let w = vec![Watch {
            repo: "o/r".into(),
            workflow: "ci.yml".into(),
        }];
        store.save(&w).await.unwrap();
        assert_eq!(store.load().await.unwrap(), w);
        assert!(
            Store::new(WatchStore::Auto, None)
                .await
                .unwrap()
                .name()
                .is_none()
        );
        assert!(Store::new(WatchStore::File, None).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn off_and_file_errors() {
        let off = Store::new(WatchStore::Off, Some(Path::new("/x")))
            .await
            .unwrap();
        assert!(off.name().is_none());
        assert!(off.load().await.unwrap().is_empty());
        assert_eq!(
            off.save(&[]).await.unwrap_err().to_string(),
            "saving watches is turned off (github.store = \"off\")"
        );
        // not in a cluster here
        let e = Store::new(WatchStore::Configmap, None).await.err().unwrap();
        assert!(
            e.to_string()
                .starts_with("github.store = \"configmap\" runs in a cluster only")
        );

        let dir = std::env::temp_dir().join(format!("homie-watch-err-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("github-watch.json")).unwrap();
        let store = Store::new(WatchStore::File, Some(&dir)).await.unwrap();
        let e = store.load().await.unwrap_err().to_string();
        assert!(e.contains("github-watch.json: "), "{e}");
        assert!(store.save(&[]).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Kubernetes API holding at most one ConfigMap in namespace `ns`;
    /// records the writes as "create" or "replace".
    async fn fake_api() -> (
        String,
        Arc<Mutex<Option<Value>>>,
        Arc<Mutex<Vec<&'static str>>>,
    ) {
        let cm: Arc<Mutex<Option<Value>>> = Arc::default();
        let writes: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let path = format!("/api/v1/namespaces/ns/configmaps/{CONFIGMAP}");
        let (get_cm, post_cm, put_cm) = (cm.clone(), cm.clone(), cm.clone());
        let (post_w, put_w) = (writes.clone(), writes.clone());
        let app = Router::new()
            .route(
                &path,
                get(move || async move {
                    match get_cm.lock().unwrap().clone() {
                        Some(v) => Json(v).into_response(),
                        None => (
                            StatusCode::NOT_FOUND,
                            Json(json!({"kind": "Status", "apiVersion": "v1", "metadata": {},
                                "status": "Failure", "reason": "NotFound", "code": 404,
                                "message": "configmaps \"homie-github-watch\" not found"})),
                        )
                            .into_response(),
                    }
                })
                .put(move |Json(mut v): Json<Value>| async move {
                    assert_eq!(v["metadata"]["resourceVersion"], "1");
                    v["metadata"]["resourceVersion"] = json!("2");
                    put_w.lock().unwrap().push("replace");
                    *put_cm.lock().unwrap() = Some(v.clone());
                    Json(v)
                }),
            )
            .route(
                "/api/v1/namespaces/ns/configmaps",
                post(move |Json(mut v): Json<Value>| async move {
                    v["metadata"]["resourceVersion"] = json!("1");
                    post_w.lock().unwrap().push("create");
                    *post_cm.lock().unwrap() = Some(v.clone());
                    (StatusCode::CREATED, Json(v))
                }),
            );
        (testkit::serve(app).await, cm, writes)
    }

    #[tokio::test]
    async fn configmap_against_fake_api() {
        let (url, cm, writes) = fake_api().await;
        let store = Store::ConfigMap(Api::namespaced(testkit::kube_client(&url), "ns"));
        assert_eq!(store.name(), Some("configmap"));
        assert!(store.load().await.unwrap().is_empty());
        let mut w = vec![Watch {
            repo: "o/r".into(),
            workflow: "ci.yml".into(),
        }];
        store.save(&w).await.unwrap();
        w.push(Watch {
            repo: "o/r".into(),
            workflow: "plan.yml".into(),
        });
        store.save(&w).await.unwrap();
        assert_eq!(*writes.lock().unwrap(), ["create", "replace"]);
        assert_eq!(store.load().await.unwrap(), w);
        let saved = cm.lock().unwrap().clone().unwrap();
        assert_eq!(saved["metadata"]["name"], CONFIGMAP);

        // a ConfigMap without the key holds no watches
        *cm.lock().unwrap() = Some(json!({"metadata": {"name": CONFIGMAP}, "data": {}}));
        assert!(store.load().await.unwrap().is_empty());
    }
}

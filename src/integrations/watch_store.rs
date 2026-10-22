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
            Self::File(_) => Some("file"),
            Self::ConfigMap(_) => Some("configmap"),
        }
    }

    pub async fn load(&self) -> Result<Vec<Watch>> {
        let json = match self {
            Self::Off => return Ok(Vec::new()),
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

    /// Against the current kubeconfig context, namespace default:
    /// `cargo test -- --ignored configmap_round_trip`
    #[tokio::test]
    #[ignore]
    async fn configmap_round_trip() {
        let client = kube::Client::try_default().await.unwrap();
        let api: Api<ConfigMap> = Api::namespaced(client, "default");
        let _ = api.delete(CONFIGMAP, &Default::default()).await;
        let store = Store::ConfigMap(api.clone());
        assert!(store.load().await.unwrap().is_empty());
        let mut w = vec![Watch {
            repo: "o/r".into(),
            workflow: "ci.yml".into(),
        }];
        store.save(&w).await.unwrap(); // create
        w.push(Watch {
            repo: "o/r".into(),
            workflow: "plan.yml".into(),
        });
        store.save(&w).await.unwrap(); // replace
        assert_eq!(store.load().await.unwrap(), w);
        api.delete(CONFIGMAP, &Default::default()).await.unwrap();
    }
}

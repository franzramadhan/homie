//! Kubernetes: nodes, workloads, unhealthy pods and warning events, with the
//! client identity chosen by `auth`: a kubeconfig context, the in-cluster
//! ServiceAccount, or the logged-in user's forwarded ID token.

use std::collections::HashMap;

use http::{HeaderValue, header::AUTHORIZATION};
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, StatefulSet},
    core::v1::{Event, Namespace, Node, Pod},
};
use kube::{
    Api, Client, Config,
    api::ListParams,
    config::{AuthInfo, KubeConfigOptions, Kubeconfig},
};
use serde::Serialize;

use crate::{
    Result,
    config::{self, KubeAuth},
};

pub struct Kube {
    pub name: String,
    pub quiet: bool,
    pub mode: KubeAuth,
    pub headlamp_url: Option<String>,
    headlamp_cluster: String,
    base: Config,
    shared: Option<Client>,
}

#[derive(Serialize)]
pub struct Overview {
    nodes: Vec<NodeRow>,
    namespaces: usize,
    workloads: Vec<Workload>,
    unhealthy_pods: Vec<PodRow>,
    events: Vec<EventRow>,
}

#[derive(Serialize)]
struct NodeRow {
    name: String,
    ready: bool,
    unschedulable: bool,
    version: String,
    roles: Vec<String>,
    cpu_allocatable: f64,
    cpu_requested: f64,
    mem_allocatable: f64,
    mem_requested: f64,
    link: Option<String>,
}

#[derive(Serialize)]
struct Workload {
    kind: &'static str,
    namespace: String,
    name: String,
    ready: i32,
    desired: i32,
    link: Option<String>,
}

#[derive(Serialize)]
struct PodRow {
    namespace: String,
    name: String,
    phase: String,
    reason: String,
    restarts: i32,
    node: String,
    link: Option<String>,
}

#[derive(Serialize)]
struct EventRow {
    namespace: String,
    object: String,
    reason: String,
    message: String,
    count: i32,
    last: String,
}

impl Kube {
    pub async fn new(name: &str, cfg: &config::Kubernetes) -> Result<Self> {
        let mode = cfg.auth_mode();
        let err = |e: &dyn std::fmt::Display| format!("kubernetes.{name}: {e}");
        let kubeconfig = |path: Option<&std::path::Path>| {
            let opts = KubeConfigOptions {
                context: cfg.context.clone(),
                ..Default::default()
            };
            let path = path.map(config::expand);
            async move {
                match path {
                    Some(p) => {
                        Config::from_custom_kubeconfig(Kubeconfig::read_from(&p)?, &opts).await
                    }
                    None => Config::from_kubeconfig(&opts).await,
                }
            }
        };
        let base = match mode {
            KubeAuth::Kubeconfig => kubeconfig(cfg.kubeconfig.as_deref())
                .await
                .map_err(|e| err(&e))?,
            KubeAuth::InCluster => Config::incluster().map_err(|e| err(&e))?,
            // the cluster address and CA come from the kubeconfig, or from the pod
            KubeAuth::ForwardToken if cfg.kubeconfig.is_some() => {
                kubeconfig(cfg.kubeconfig.as_deref())
                    .await
                    .map_err(|e| err(&e))?
            }
            KubeAuth::ForwardToken => Config::incluster().map_err(|e| err(&e))?,
        };
        let shared = match mode {
            KubeAuth::ForwardToken => None,
            _ => Some(Client::try_from(base.clone()).map_err(|e| err(&e))?),
        };
        Ok(Self {
            name: name.to_string(),
            quiet: cfg.quiet,
            mode,
            headlamp_url: cfg
                .headlamp_url
                .as_ref()
                .map(|u| u.trim_end_matches('/').to_string()),
            headlamp_cluster: cfg.headlamp_cluster.clone(),
            base,
            shared,
        })
    }

    pub fn forwards_token(&self) -> bool {
        self.mode == KubeAuth::ForwardToken
    }

    /// The client to act with: the shared one, or one carrying the user's token.
    // ponytail: builds a client per request in forward_token mode; cache per session if it shows up in profiles
    pub fn client(&self, id_token: Option<&str>) -> Result<Client> {
        if let Some(c) = &self.shared {
            return Ok(c.clone());
        }
        let token = id_token
            .ok_or("this cluster acts as the logged-in user, but there is no user session")?;
        let mut cfg = self.base.clone();
        cfg.auth_info = AuthInfo::default();
        cfg.headers.push((
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        ));
        Ok(Client::try_from(cfg)?)
    }

    /// Headlamp deep link, when `headlamp_url` is set.
    pub fn link(&self, plural: &str, namespace: Option<&str>, name: &str) -> Option<String> {
        let base = self.headlamp_url.as_ref()?;
        let c = &self.headlamp_cluster;
        Some(match namespace {
            Some(ns) => format!("{base}/c/{c}/{plural}/{ns}/{name}"),
            None => format!("{base}/c/{c}/{plural}/{name}"),
        })
    }

    pub async fn overview(&self, client: Client) -> Result<Overview> {
        let lp = ListParams::default();
        let warnings = ListParams::default().fields("type=Warning");
        let (nodes, pods, deploys, sets, daemons, events, namespaces) = (
            Api::<Node>::all(client.clone()),
            Api::<Pod>::all(client.clone()),
            Api::<Deployment>::all(client.clone()),
            Api::<StatefulSet>::all(client.clone()),
            Api::<DaemonSet>::all(client.clone()),
            Api::<Event>::all(client.clone()),
            Api::<Namespace>::all(client),
        );
        let (nodes, pods, deploys, sets, daemons, events, namespaces) = tokio::try_join!(
            nodes.list(&lp),
            pods.list(&lp),
            deploys.list(&lp),
            sets.list(&lp),
            daemons.list(&lp),
            events.list(&warnings),
            namespaces.list_metadata(&lp),
        )?;

        let now = jiff::Timestamp::now();
        let hour_ago = now - jiff::SignedDuration::from_hours(1);
        let mut requested: HashMap<String, (f64, f64)> = HashMap::new();
        let mut unhealthy_pods = Vec::new();
        for pod in &pods {
            let (Some(spec), Some(status)) = (&pod.spec, &pod.status) else {
                continue;
            };
            let phase = status.phase.clone().unwrap_or_default();
            if phase == "Succeeded" {
                // finished Job pods are history, not health
                continue;
            }
            if let (Some(node), "Running" | "Pending") = (&spec.node_name, phase.as_str()) {
                let r = requested.entry(node.clone()).or_default();
                for c in &spec.containers {
                    let req = c.resources.as_ref().and_then(|r| r.requests.as_ref());
                    r.0 += req.and_then(|m| m.get("cpu")).map_or(0.0, |q| cpu(&q.0));
                    r.1 += req
                        .and_then(|m| m.get("memory"))
                        .map_or(0.0, |q| bytes(&q.0));
                }
            }
            let containers = status.container_statuses.as_deref().unwrap_or_default();
            let ready = status
                .conditions
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True");
            let recent_restart = containers.iter().any(|cs| {
                cs.last_state
                    .as_ref()
                    .and_then(|s| s.terminated.as_ref())
                    .and_then(|t| t.finished_at.as_ref())
                    .is_some_and(|t| t.0 > hour_ago)
            });
            if phase == "Running" && ready && !recent_restart {
                continue;
            }
            let waiting = containers
                .iter()
                .find_map(|cs| cs.state.as_ref()?.waiting.as_ref()?.reason.clone());
            let ns = pod.metadata.namespace.clone().unwrap_or_default();
            let name = pod.metadata.name.clone().unwrap_or_default();
            unhealthy_pods.push(PodRow {
                link: self.link("pods", Some(&ns), &name),
                reason: waiting
                    .or_else(|| status.reason.clone())
                    .unwrap_or_else(|| {
                        if recent_restart {
                            "Restarted".into()
                        } else {
                            "NotReady".into()
                        }
                    }),
                restarts: containers.iter().map(|c| c.restart_count).sum(),
                node: spec.node_name.clone().unwrap_or_default(),
                namespace: ns,
                name,
                phase,
            });
        }

        let mut node_rows: Vec<NodeRow> = nodes
            .iter()
            .map(|n| {
                let name = n.metadata.name.clone().unwrap_or_default();
                let status = n.status.as_ref();
                let alloc = status.and_then(|s| s.allocatable.as_ref());
                let (cpu_req, mem_req) = requested.get(&name).copied().unwrap_or_default();
                NodeRow {
                    ready: status.and_then(|s| s.conditions.as_ref()).is_some_and(|c| {
                        c.iter().any(|c| c.type_ == "Ready" && c.status == "True")
                    }),
                    unschedulable: n
                        .spec
                        .as_ref()
                        .and_then(|s| s.unschedulable)
                        .unwrap_or(false),
                    version: status
                        .and_then(|s| s.node_info.as_ref())
                        .map(|i| i.kubelet_version.clone())
                        .unwrap_or_default(),
                    roles: n
                        .metadata
                        .labels
                        .iter()
                        .flatten()
                        .filter_map(|(k, _)| {
                            k.strip_prefix("node-role.kubernetes.io/").map(String::from)
                        })
                        .collect(),
                    cpu_allocatable: alloc.and_then(|a| a.get("cpu")).map_or(0.0, |q| cpu(&q.0)),
                    mem_allocatable: alloc
                        .and_then(|a| a.get("memory"))
                        .map_or(0.0, |q| bytes(&q.0)),
                    cpu_requested: cpu_req,
                    mem_requested: mem_req,
                    link: self.link("nodes", None, &name),
                    name,
                }
            })
            .collect();
        node_rows.sort_by(|a, b| a.name.cmp(&b.name));

        let mut workloads = Vec::new();
        let mut add = |kind,
                       plural,
                       meta: &kube::api::ObjectMeta,
                       ready: Option<i32>,
                       desired: Option<i32>| {
            let ns = meta.namespace.clone().unwrap_or_default();
            let name = meta.name.clone().unwrap_or_default();
            workloads.push(Workload {
                kind,
                link: self.link(plural, Some(&ns), &name),
                namespace: ns,
                name,
                ready: ready.unwrap_or(0),
                desired: desired.unwrap_or(1),
            });
        };
        for d in &deploys {
            add(
                "Deployment",
                "deployments",
                &d.metadata,
                d.status.as_ref().and_then(|s| s.ready_replicas),
                d.spec.as_ref().and_then(|s| s.replicas),
            );
        }
        for s in &sets {
            add(
                "StatefulSet",
                "statefulsets",
                &s.metadata,
                s.status.as_ref().and_then(|s| s.ready_replicas),
                s.spec.as_ref().and_then(|s| s.replicas),
            );
        }
        for d in &daemons {
            let st = d.status.as_ref();
            add(
                "DaemonSet",
                "daemonsets",
                &d.metadata,
                st.map(|s| s.number_ready),
                st.map(|s| s.desired_number_scheduled),
            );
        }
        workloads.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));

        let mut events: Vec<(jiff::Timestamp, EventRow)> = events
            .iter()
            .filter_map(|e| {
                let last = e
                    .last_timestamp
                    .as_ref()
                    .map(|t| t.0)
                    .or_else(|| e.event_time.as_ref().map(|t| t.0))
                    .or_else(|| e.metadata.creation_timestamp.as_ref().map(|t| t.0))?;
                (last > hour_ago).then(|| {
                    let o = &e.involved_object;
                    (
                        last,
                        EventRow {
                            namespace: e.metadata.namespace.clone().unwrap_or_default(),
                            object: format!(
                                "{}/{}",
                                o.kind.clone().unwrap_or_default(),
                                o.name.clone().unwrap_or_default()
                            ),
                            reason: e.reason.clone().unwrap_or_default(),
                            message: e.message.clone().unwrap_or_default(),
                            count: e.count.unwrap_or(1),
                            last: last.to_string(),
                        },
                    )
                })
            })
            .collect();
        events.sort_by_key(|e| std::cmp::Reverse(e.0));
        events.truncate(50);

        Ok(Overview {
            nodes: node_rows,
            namespaces: namespaces.items.len(),
            workloads,
            unhealthy_pods,
            events: events.into_iter().map(|(_, e)| e).collect(),
        })
    }

    pub async fn probe(&self) -> Result<String> {
        let client = match &self.shared {
            Some(c) => c.clone(),
            // no user token here: an anonymous client only proves the API answers
            None => {
                let mut cfg = self.base.clone();
                cfg.auth_info = AuthInfo::default();
                Client::try_from(cfg)?
            }
        };
        let version = client.apiserver_version().await;
        match (&self.shared, version) {
            (Some(c), Ok(v)) => {
                let ns = Api::<Namespace>::all(c.clone()).list_metadata(&ListParams::default().limit(1)).await;
                let access = if ns.is_ok() { "can list namespaces" } else { "cannot list namespaces" };
                Ok(format!("Kubernetes {}, {access}", v.git_version))
            }
            (None, Ok(v)) => Ok(format!("Kubernetes {}, user access is checked at login (forward_token)", v.git_version)),
            (None, Err(kube::Error::Api(s))) if s.code == 401 || s.code == 403 => {
                Ok("API answers (anonymous access refused); user access is checked at login (forward_token)".into())
            }
            (_, Err(e)) => Err(e.into()),
        }
    }
}

/// CPU quantity in cores: "250m", "2", "1500000n".
pub fn cpu(q: &str) -> f64 {
    let (num, scale) = match q.as_bytes().last() {
        Some(b'm') => (&q[..q.len() - 1], 1e-3),
        Some(b'u') => (&q[..q.len() - 1], 1e-6),
        Some(b'n') => (&q[..q.len() - 1], 1e-9),
        _ => (q, 1.0),
    };
    num.parse::<f64>().map_or(0.0, |n| n * scale)
}

/// Memory quantity in bytes: "128Mi", "1G", "512e6", "1048576".
pub fn bytes(q: &str) -> f64 {
    const UNITS: [(&str, f64); 12] = [
        ("Ki", 1024.0),
        ("Mi", 1048576.0),
        ("Gi", 1073741824.0),
        ("Ti", 1099511627776.0),
        ("Pi", 1125899906842624.0),
        ("Ei", 1152921504606846976.0),
        ("k", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("P", 1e15),
        ("E", 1e18),
    ];
    for (suffix, scale) in UNITS {
        if let Some(num) = q.strip_suffix(suffix) {
            return num.parse::<f64>().map_or(0.0, |n| n * scale);
        }
    }
    q.parse::<f64>().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities() {
        assert_eq!(cpu("250m"), 0.25);
        assert_eq!(cpu("2"), 2.0);
        assert_eq!(cpu("1500000n"), 0.0015);
        assert_eq!(bytes("128Mi"), 134217728.0);
        assert_eq!(bytes("1G"), 1e9);
        assert_eq!(bytes("512e6"), 512e6);
        assert_eq!(bytes("1048576"), 1048576.0);
        assert_eq!(bytes("garbage"), 0.0);
    }
}

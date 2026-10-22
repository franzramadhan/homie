//! Proxmox VE: cluster overview and guest power actions through the API,
//! with failover across the configured node endpoints.

use std::sync::atomic::{AtomicUsize, Ordering};

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Result, config,
    http::{self, Target},
    secret::SecretCell,
};

pub struct Proxmox {
    pub name: String,
    pub quiet: bool,
    pub ui_url: String,
    pub insecure: Option<&'static str>,
    endpoints: Vec<Target>,
    token: SecretCell,
    protected_tags: Vec<String>,
    preferred: AtomicUsize,
}

#[derive(Serialize)]
pub struct Overview {
    cluster: Cluster,
    nodes: Vec<Node>,
    guests: Vec<Guest>,
    storage: Vec<Storage>,
    ha: Vec<HaService>,
    failed_tasks: Vec<Task>,
    can_power: bool,
    ui_url: String,
}

#[derive(Serialize)]
struct Cluster {
    name: Option<String>,
    quorate: Option<bool>,
    nodes_online: usize,
    nodes_total: usize,
}

#[derive(Serialize)]
struct Node {
    name: String,
    online: bool,
    cpu: f64,
    maxcpu: u64,
    mem: u64,
    maxmem: u64,
    disk: u64,
    maxdisk: u64,
    uptime: u64,
}

#[derive(Serialize, Clone)]
pub struct Guest {
    pub vmid: u64,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub node: String,
    pub status: String,
    cpu: f64,
    maxcpu: u64,
    mem: u64,
    maxmem: u64,
    uptime: u64,
    pub tags: Vec<String>,
    pub protected: bool,
    template: bool,
}

#[derive(Serialize)]
struct Storage {
    storage: String,
    node: String,
    status: String,
    plugintype: String,
    shared: bool,
    disk: u64,
    maxdisk: u64,
}

#[derive(Serialize)]
struct HaService {
    sid: String,
    node: String,
    state: String,
}

#[derive(Serialize)]
struct Task {
    node: String,
    upid: String,
    #[serde(rename = "type")]
    kind: String,
    id: String,
    user: String,
    starttime: u64,
    status: String,
}

#[derive(Deserialize)]
pub struct PowerRequest {
    pub node: String,
    pub vmid: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub action: String,
    #[serde(default)]
    pub confirm: Option<String>,
}

#[derive(Serialize)]
pub struct TaskStatus {
    status: String,
    exitstatus: Option<String>,
}

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64()
        .or_else(|| v[k].as_f64().map(|f| f as u64))
        .or_else(|| v[k].as_str()?.parse().ok())
        .unwrap_or(0)
}
fn f(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}
fn flag(v: &Value, k: &str) -> bool {
    n(v, k) == 1 || v[k].as_bool() == Some(true)
}

/// Node names and UPIDs go into API paths: allow only what Proxmox produces.
fn safe(x: &str) -> bool {
    !x.is_empty()
        && x.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":@!._-".contains(c))
}

impl Proxmox {
    pub async fn new(name: &str, cfg: &config::Proxmox) -> Result<Self> {
        let mut endpoints = Vec::new();
        for (url, tls) in cfg.targets() {
            endpoints.push(
                http::target(&url, &tls)
                    .await
                    .map_err(|e| format!("proxmox.{name} {url}: {e}"))?,
            );
        }
        let insecure = endpoints.iter().find_map(|t| t.insecure);
        let ui_url = cfg
            .ui_url
            .clone()
            .unwrap_or_else(|| endpoints[0].base.as_str().trim_end_matches('/').to_string());
        Ok(Self {
            name: name.to_string(),
            quiet: cfg.quiet,
            ui_url,
            insecure,
            endpoints,
            token: SecretCell::new(&cfg.token)
                .await
                .map_err(|e| format!("proxmox.{name}.token: {e}"))?,
            protected_tags: cfg.protected_tags.clone(),
            preferred: AtomicUsize::new(0),
        })
    }

    /// Tries each endpoint, starting with the last one that answered. GETs move
    /// on after connect errors, timeouts and 5xx; POSTs only after connect
    /// errors, because a timed-out POST may already have run.
    async fn call(&self, post: bool, path: &str) -> Result<Value> {
        let mut retried_auth = false;
        loop {
            match self.call_once(post, path).await {
                Err(e) if http::is_unauthorized(&e) && !retried_auth => {
                    retried_auth = true;
                    self.token.reload().await?;
                }
                other => return other,
            }
        }
    }

    async fn call_once(&self, post: bool, path: &str) -> Result<Value> {
        let auth = format!("PVEAPIToken={}", self.token.get().await);
        let first = self.preferred.load(Ordering::Relaxed);
        let mut last_err: Option<crate::Error> = None;
        for i in 0..self.endpoints.len() {
            let idx = (first + i) % self.endpoints.len();
            let t = &self.endpoints[idx];
            let url = http::join(&t.base, &format!("/api2/json{path}"));
            let req = if post {
                t.client.post(url)
            } else {
                t.client.get(url)
            };
            match req.header("Authorization", &auth).send().await {
                Ok(resp) if resp.status().is_server_error() && !post => {
                    last_err = Some(http::ok(resp).await.unwrap_err());
                }
                Ok(resp) => {
                    let body: Value = http::ok(resp).await?.json().await?;
                    self.preferred.store(idx, Ordering::Relaxed);
                    return Ok(body["data"].clone());
                }
                Err(e) if e.is_connect() || (e.is_timeout() && !post) => last_err = Some(e.into()),
                Err(e) => return Err(e.into()),
            }
        }
        Err(last_err.unwrap_or_else(|| "no endpoints".into()))
    }

    async fn guests(&self) -> Result<Vec<Guest>> {
        let resources = self.call(false, "/cluster/resources?type=vm").await?;
        Ok(resources
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| self.guest(r))
            .collect())
    }

    fn guest(&self, r: &Value) -> Option<Guest> {
        let kind = s(r, "type");
        if kind != "qemu" && kind != "lxc" {
            return None;
        }
        let tags: Vec<String> = s(r, "tags")
            .split([';', ',', ' '])
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect();
        Some(Guest {
            vmid: n(r, "vmid"),
            name: s(r, "name"),
            protected: tags.iter().any(|t| self.protected_tags.contains(t)),
            kind,
            node: s(r, "node"),
            status: s(r, "status"),
            cpu: f(r, "cpu"),
            maxcpu: n(r, "maxcpu"),
            mem: n(r, "mem"),
            maxmem: n(r, "maxmem"),
            uptime: n(r, "uptime"),
            tags,
            template: flag(r, "template"),
        })
    }

    pub async fn overview(&self) -> Result<Overview> {
        let (status, resources, ha, perms) = tokio::join!(
            self.call(false, "/cluster/status"),
            self.call(false, "/cluster/resources"),
            self.call(false, "/cluster/ha/status/current"),
            self.call(false, "/access/permissions"),
        );
        let (status, resources) = (status?, resources?);
        let status = status.as_array().cloned().unwrap_or_default();
        let resources = resources.as_array().cloned().unwrap_or_default();

        let cluster_entry = status.iter().find(|e| s(e, "type") == "cluster");
        let node_entries: Vec<&Value> = status.iter().filter(|e| s(e, "type") == "node").collect();
        let cluster = Cluster {
            name: cluster_entry.map(|c| s(c, "name")),
            quorate: cluster_entry.map(|c| flag(c, "quorate")),
            nodes_online: node_entries.iter().filter(|e| flag(e, "online")).count(),
            nodes_total: node_entries.len(),
        };

        let mut nodes = Vec::new();
        let mut storage = Vec::new();
        let mut guests = Vec::new();
        for r in &resources {
            match s(r, "type").as_str() {
                "node" => nodes.push(Node {
                    name: s(r, "node"),
                    online: s(r, "status") == "online",
                    cpu: f(r, "cpu"),
                    maxcpu: n(r, "maxcpu"),
                    mem: n(r, "mem"),
                    maxmem: n(r, "maxmem"),
                    disk: n(r, "disk"),
                    maxdisk: n(r, "maxdisk"),
                    uptime: n(r, "uptime"),
                }),
                "storage" => storage.push(Storage {
                    storage: s(r, "storage"),
                    node: s(r, "node"),
                    status: s(r, "status"),
                    plugintype: s(r, "plugintype"),
                    shared: flag(r, "shared"),
                    disk: n(r, "disk"),
                    maxdisk: n(r, "maxdisk"),
                }),
                _ => guests.extend(self.guest(r)),
            }
        }
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        guests.sort_by_key(|g| g.vmid);

        // HA and permissions are optional extras: a token without access to
        // them still gets the rest of the overview.
        let ha = ha
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter(|e| s(e, "type") == "service")
            .map(|e| HaService {
                sid: s(e, "sid"),
                node: s(e, "node"),
                state: s(e, "state"),
            })
            .collect();
        let can_power = perms
            .ok()
            .and_then(|p| p.as_object().cloned())
            .is_some_and(|paths| {
                paths
                    .values()
                    .any(|privs| privs.get("VM.PowerMgmt").is_some())
            });

        let online: Vec<String> = nodes
            .iter()
            .filter(|n| n.online)
            .map(|n| n.name.clone())
            .collect();
        // last 24 h only: older failures are history, and the nightly power
        // schedule leaves "already running" errors every morning
        let since = jiff::Timestamp::now().as_second() - 86_400;
        let tasks = join_all(online.iter().filter(|n| safe(n)).map(|node| async move {
            let list = self
                .call(
                    false,
                    &format!("/nodes/{node}/tasks?errors=1&limit=10&since={since}"),
                )
                .await
                .ok()?;
            Some(
                list.as_array()?
                    .iter()
                    .map(|t| Task {
                        node: node.clone(),
                        upid: s(t, "upid"),
                        kind: s(t, "type"),
                        id: s(t, "id"),
                        user: s(t, "user"),
                        starttime: n(t, "starttime"),
                        status: s(t, "status"),
                    })
                    .collect::<Vec<_>>(),
            )
        }))
        .await;
        let mut failed_tasks: Vec<Task> = tasks.into_iter().flatten().flatten().collect();
        failed_tasks.sort_by_key(|t| std::cmp::Reverse(t.starttime));
        failed_tasks.truncate(20);

        Ok(Overview {
            cluster,
            nodes,
            guests,
            storage,
            ha,
            failed_tasks,
            can_power,
            ui_url: self.ui_url.clone(),
        })
    }

    /// Validates the request against the live guest list, then starts the
    /// action. Returns the task UPID.
    pub async fn power(&self, req: &PowerRequest) -> Result<String> {
        if !matches!(req.action.as_str(), "start" | "shutdown" | "reboot") {
            return Err(format!("unknown action {:?}", req.action).into());
        }
        let guests = self.guests().await?;
        let guest = guests
            .iter()
            .find(|g| g.vmid == req.vmid && g.kind == req.kind && g.node == req.node)
            .ok_or_else(|| format!("no {} {} on node {:?}", req.kind, req.vmid, req.node))?;
        if guest.protected && req.confirm.as_deref() != Some(guest.name.as_str()) {
            return Err(format!("{} is protected: type its name to confirm", guest.name).into());
        }
        if !safe(&guest.node) {
            return Err("unexpected node name".into());
        }
        let upid = self
            .call(
                true,
                &format!(
                    "/nodes/{}/{}/{}/status/{}",
                    guest.node, guest.kind, guest.vmid, req.action
                ),
            )
            .await?;
        upid.as_str()
            .map(String::from)
            .ok_or_else(|| "Proxmox returned no task id".into())
    }

    pub async fn task(&self, node: &str, upid: &str) -> Result<TaskStatus> {
        if !safe(node) || !safe(upid) || !upid.starts_with("UPID:") {
            return Err("invalid node or task id".into());
        }
        let t = self
            .call(false, &format!("/nodes/{node}/tasks/{upid}/status"))
            .await?;
        Ok(TaskStatus {
            status: s(&t, "status"),
            exitstatus: t["exitstatus"].as_str().map(String::from),
        })
    }

    pub async fn probe(&self) -> Result<String> {
        let version = self.call(false, "/version").await?;
        let perms = self.call(false, "/access/permissions").await?;
        let power = perms
            .as_object()
            .is_some_and(|p| p.values().any(|v| v.get("VM.PowerMgmt").is_some()));
        Ok(format!(
            "Proxmox VE {}, power actions {}",
            s(&version, "version"),
            if power {
                "enabled"
            } else {
                "off (no VM.PowerMgmt)"
            }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_values_are_restricted() {
        assert!(safe("pve-01"));
        assert!(safe(
            "UPID:pve-01:0000A1B2:00C3D4E5:6717F00D:qmstart:101:homie@pve!k0s:"
        ));
        assert!(!safe("pve-01/../../access"));
        assert!(!safe("a b"));
        assert!(!safe(""));
    }

    /// The first endpoint refuses connections; the second answers and is
    /// remembered. A wrong token is a 401, not a failover.
    #[tokio::test]
    async fn fails_over_and_sends_the_token() {
        use axum::{Json, Router, http::HeaderMap, routing::get};
        let app = Router::new().route(
            "/api2/json/version",
            get(|h: HeaderMap| async move {
                match h.get("authorization").and_then(|v| v.to_str().ok()) {
                    Some("PVEAPIToken=homie@pve!t=secret") => {
                        Ok(Json(serde_json::json!({"data": {"version": "9.2.21"}})))
                    }
                    _ => Err(axum::http::StatusCode::UNAUTHORIZED),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = |token: &str| -> config::Proxmox {
            serde_json::from_value(serde_json::json!({
                "endpoints": ["http://127.0.0.1:1", format!("http://{addr}")],
                "token": token,
            }))
            .unwrap()
        };
        let p = Proxmox::new("t", &cfg("homie@pve!t=secret")).await.unwrap();
        assert_eq!(p.insecure, Some("plain_http"));
        assert_eq!(
            p.call(false, "/version").await.unwrap()["version"],
            "9.2.21"
        );
        assert_eq!(p.preferred.load(Ordering::Relaxed), 1);

        let wrong = Proxmox::new("t", &cfg("homie@pve!t=wrong")).await.unwrap();
        let err = wrong.call(false, "/version").await.unwrap_err();
        assert!(http::is_unauthorized(&err), "{err}");
    }

    #[test]
    fn numbers_from_strings_and_floats() {
        let v = serde_json::json!({"a": "12", "b": 3.9, "c": 1, "t": "prod;homie-protected"});
        assert_eq!((n(&v, "a"), n(&v, "b"), n(&v, "missing")), (12, 3, 0));
        assert!(flag(&v, "c"));
    }
}

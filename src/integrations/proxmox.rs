//! Proxmox VE: cluster overview, guest power actions and consoles through
//! the API, with failover across the configured node endpoints.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use futures::{SinkExt, StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

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
    /// embedded consoles (RFC 0023) and how long one may stay open
    pub console: bool,
    pub console_max: Duration,
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
    can_console: bool,
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

#[derive(Deserialize)]
pub struct ConsoleRequest {
    pub node: String,
    pub vmid: u64,
    #[serde(rename = "type")]
    pub kind: String,
    /// `vnc` or `term`; none picks `term` for containers and VMs with serial0
    #[serde(default)]
    pub mode: Option<String>,
}

/// An opened Proxmox console. The ticket stays in Homie; only `password`
/// (a one-time VNC password for this console, `generate-password`) goes to
/// the browser, because noVNC answers the VNC challenge itself.
#[cfg_attr(test, derive(Default))]
pub struct Console {
    pub guest: String,
    pub mode: &'static str,
    /// a VM that can switch between its serial port and VNC
    pub switchable: bool,
    pub password: Option<String>,
    path: String,
    port: String,
    ticket: String,
    user: String,
}

pub trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}
pub type ConsoleStream = tokio_tungstenite::WebSocketStream<Box<dyn Io>>;

/// The console kind for a guest and the mode the user asked for.
fn console_mode(kind: &str, serial: bool, asked: Option<&str>) -> Result<&'static str> {
    match (kind, asked) {
        ("lxc", None | Some("term")) => Ok("term"),
        ("lxc", Some(_)) => Err("containers have a terminal only".into()),
        (_, Some("vnc")) => Ok("vnc"),
        (_, Some("term")) if serial => Ok("term"),
        (_, Some("term")) => Err("this VM has no serial0 port".into()),
        (_, None) => Ok(if serial { "term" } else { "vnc" }),
        (_, Some(m)) => Err(format!("unknown console mode {m:?}").into()),
    }
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

/// Why an action needs the guest's name typed, if it does. Stop is a hard
/// power-off, so it always needs it; protected guests need it for every action.
fn confirmation_needed(guest: &Guest, action: &str, confirm: Option<&str>) -> Option<String> {
    if confirm == Some(guest.name.as_str()) {
        return None;
    }
    if action == "stop" {
        Some(format!(
            "stop is a hard power-off: type {} to confirm",
            guest.name
        ))
    } else if guest.protected {
        Some(format!(
            "{} is protected: type its name to confirm",
            guest.name
        ))
    } else {
        None
    }
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
            console: cfg.console,
            console_max: Duration::from_secs(cfg.console_max_minutes * 60),
        })
    }

    /// Tries each endpoint, starting with the last one that answered. GETs move
    /// on after connect errors, timeouts and 5xx; POSTs only after connect
    /// errors, because a timed-out POST may already have run.
    async fn call(&self, post: bool, path: &str) -> Result<Value> {
        self.call_form(post, path, &[]).await
    }

    async fn call_form(&self, post: bool, path: &str, form: &[(&str, &str)]) -> Result<Value> {
        let mut retried_auth = false;
        loop {
            match self.call_once(post, path, form).await {
                Err(e) if http::is_unauthorized(&e) && !retried_auth => {
                    retried_auth = true;
                    self.token.reload().await?;
                }
                other => return other,
            }
        }
    }

    async fn call_once(&self, post: bool, path: &str, form: &[(&str, &str)]) -> Result<Value> {
        let auth = format!("PVEAPIToken={}", self.token.get().await);
        let first = self.preferred.load(Ordering::Relaxed);
        let mut last_err: Option<crate::Error> = None;
        for i in 0..self.endpoints.len() {
            let idx = (first + i) % self.endpoints.len();
            let t = &self.endpoints[idx];
            let url = http::join(&t.base, &format!("/api2/json{path}"));
            let req = if post {
                t.client.post(url).form(form)
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
        let perms = perms.ok().and_then(|p| p.as_object().cloned());
        let has = |privilege: &str| {
            perms
                .as_ref()
                .is_some_and(|paths| paths.values().any(|privs| privs.get(privilege).is_some()))
        };
        let can_power = has("VM.PowerMgmt");
        let can_console = self.console && has("VM.Console");

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
            can_console,
            ui_url: self.ui_url.clone(),
        })
    }

    /// Validates the request against the live guest list, then starts the
    /// action. Returns the task UPID.
    pub async fn power(&self, req: &PowerRequest) -> Result<String> {
        if !matches!(
            req.action.as_str(),
            "start" | "shutdown" | "reboot" | "stop"
        ) {
            return Err(format!("unknown action {:?}", req.action).into());
        }
        let guests = self.guests().await?;
        let guest = guests
            .iter()
            .find(|g| g.vmid == req.vmid && g.kind == req.kind && g.node == req.node)
            .ok_or_else(|| format!("no {} {} on node {:?}", req.kind, req.vmid, req.node))?;
        if let Some(why) = confirmation_needed(guest, &req.action, req.confirm.as_deref()) {
            return Err(why.into());
        }
        if !safe(&guest.node) {
            return Err("unexpected node name".into());
        }
        // one action at a time per guest: a second one only fails on the lock
        let active = self
            .call(
                false,
                &format!(
                    "/nodes/{}/tasks?vmid={}&source=active",
                    guest.node, guest.vmid
                ),
            )
            .await?;
        if let Some(t) = active.as_array().and_then(|a| a.first()) {
            return Err(format!(
                "{} already has a running task ({}); wait for it to finish",
                guest.name,
                s(t, "type")
            )
            .into());
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

    /// Opens a console on a running guest from the live guest list: a
    /// terminal for containers and VMs with serial0, VNC otherwise or when
    /// asked for.
    pub async fn console(&self, req: &ConsoleRequest) -> Result<Console> {
        let guests = self.guests().await?;
        let guest = guests
            .iter()
            .find(|g| g.vmid == req.vmid && g.kind == req.kind && g.node == req.node)
            .ok_or_else(|| format!("no {} {} on node {:?}", req.kind, req.vmid, req.node))?;
        if !safe(&guest.node) {
            return Err("unexpected node name".into());
        }
        if guest.status != "running" {
            return Err(format!("{} is not running", guest.name).into());
        }
        let base = format!("/nodes/{}/{}/{}", guest.node, guest.kind, guest.vmid);
        let serial = guest.kind == "qemu"
            && self
                .call(false, &format!("{base}/config"))
                .await?
                .get("serial0")
                .is_some();
        let mode = console_mode(&guest.kind, serial, req.mode.as_deref())?;
        let opened = match (mode, guest.kind.as_str()) {
            ("vnc", _) => {
                let form = [("websocket", "1"), ("generate-password", "1")];
                self.call_form(true, &format!("{base}/vncproxy"), &form)
                    .await?
            }
            (_, "lxc") => self.call(true, &format!("{base}/termproxy")).await?,
            _ => {
                self.call_form(true, &format!("{base}/termproxy"), &[("serial", "serial0")])
                    .await?
            }
        };
        Ok(Console {
            guest: guest.name.clone(),
            mode,
            switchable: serial,
            password: opened["password"].as_str().map(String::from),
            path: format!("{base}/vncwebsocket"),
            port: s(&opened, "port"),
            ticket: s(&opened, "ticket"),
            user: s(&opened, "user"),
        })
    }

    /// Connects to an opened console's WebSocket with the token and the
    /// ticket; a terminal also gets its login line here, so the browser
    /// only ever sees the console itself.
    pub async fn console_connect(&self, c: &Console) -> Result<ConsoleStream> {
        let t = &self.endpoints[self.preferred.load(Ordering::Relaxed)];
        let mut url = http::join(&t.base, &format!("/api2/json{}", c.path));
        url.query_pairs_mut()
            .append_pair("port", &c.port)
            .append_pair("vncticket", &c.ticket);
        let host = url.host_str().ok_or("URL has no host")?.to_string();
        let port = url.port_or_known_default().ok_or("URL has no port")?;
        let addr = match t.addr {
            Some(a) => a,
            None => tokio::net::lookup_host((host.as_str(), port))
                .await?
                .next()
                .ok_or_else(|| format!("{host} does not resolve"))?,
        };
        let tcp =
            tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(addr))
                .await
                .map_err(|_| format!("connecting to {host}: timed out"))??;
        let io: Box<dyn Io> = match &t.tls {
            Some(cfg) => {
                let name = rustls::pki_types::ServerName::try_from(
                    host.trim_matches(['[', ']']).to_string(),
                )?;
                Box::new(
                    tokio_rustls::TlsConnector::from(cfg.clone())
                        .connect(name, tcp)
                        .await?,
                )
            }
            None => Box::new(tcp),
        };
        let scheme = if t.tls.is_some() { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| "unexpected URL scheme")?;
        let mut req = url.as_str().into_client_request()?;
        let headers = req.headers_mut();
        headers.insert(
            "Authorization",
            format!("PVEAPIToken={}", self.token.get().await).parse()?,
        );
        headers.insert("Sec-WebSocket-Protocol", "binary".parse()?);
        let (mut ws, _) = tokio_tungstenite::client_async(req, io).await?;
        if c.mode == "term" {
            ws.send(Message::text(format!("{}:{}\n", c.user, c.ticket)))
                .await?;
            let reply = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
            if !matches!(reply, Ok(Some(Ok(ref m))) if m.clone().into_data().starts_with(b"OK")) {
                return Err("the terminal did not accept the console ticket".into());
            }
        }
        Ok(ws)
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

    /// A read for other modules (sparklines); same failover as the overview.
    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(false, path).await
    }

    pub async fn probe(&self) -> Result<String> {
        let version = self.call(false, "/version").await?;
        let perms = self.call(false, "/access/permissions").await?;
        let has = |privilege: &str| {
            perms
                .as_object()
                .is_some_and(|p| p.values().any(|v| v.get(privilege).is_some()))
        };
        // consoles are opt-in: mentioned only when configured
        let console = match (self.console, has("VM.Console")) {
            (false, _) => "",
            (true, true) => ", console on",
            (true, false) => ", console off (no VM.Console)",
        };
        Ok(format!(
            "Proxmox VE {}, power actions {}{console}",
            s(&version, "version"),
            if has("VM.PowerMgmt") {
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
    fn console_modes() {
        assert_eq!(console_mode("lxc", false, None).unwrap(), "term");
        assert!(console_mode("lxc", false, Some("vnc")).is_err());
        assert_eq!(console_mode("qemu", true, None).unwrap(), "term");
        assert_eq!(console_mode("qemu", false, None).unwrap(), "vnc");
        assert_eq!(console_mode("qemu", true, Some("vnc")).unwrap(), "vnc");
        assert!(console_mode("qemu", false, Some("term")).is_err());
        assert!(console_mode("qemu", true, Some("spice")).is_err());
    }

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
    fn confirmations() {
        let mut g = Guest {
            vmid: 1,
            name: "vm1".into(),
            kind: "qemu".into(),
            node: "pve".into(),
            status: "running".into(),
            cpu: 0.0,
            maxcpu: 1,
            mem: 0,
            maxmem: 0,
            uptime: 0,
            tags: vec![],
            protected: false,
            template: false,
        };
        assert!(confirmation_needed(&g, "shutdown", None).is_none());
        assert!(confirmation_needed(&g, "stop", None).is_some());
        assert!(confirmation_needed(&g, "stop", Some("vm1")).is_none());
        g.protected = true;
        assert!(confirmation_needed(&g, "reboot", Some("wrong")).is_some());
        assert!(confirmation_needed(&g, "reboot", Some("vm1")).is_none());
    }

    /// Keeps `confirmation_needed` equal to `proceeds` in spec/power.bend,
    /// whose laws (LAWS.bend) are proven by `bend PROOF.bend`.
    #[test]
    fn confirmations_match_spec() {
        for action in ["start", "shutdown", "reboot", "stop"] {
            for protected in [false, true] {
                for (confirm, typed) in [(None, false), (Some("wrong"), false), (Some("vm1"), true)]
                {
                    let g = Guest {
                        vmid: 1,
                        name: "vm1".into(),
                        kind: "qemu".into(),
                        node: "pve".into(),
                        status: "running".into(),
                        cpu: 0.0,
                        maxcpu: 1,
                        mem: 0,
                        maxmem: 0,
                        uptime: 0,
                        tags: vec![],
                        protected,
                        template: false,
                    };
                    let model = match action {
                        "stop" => typed,
                        _ => typed || !protected,
                    };
                    assert_eq!(
                        confirmation_needed(&g, action, confirm).is_none(),
                        model,
                        "{action} protected={protected} confirm={confirm:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn numbers_from_strings_and_floats() {
        let v = serde_json::json!({"a": "12", "b": 3.9, "c": 1, "t": "prod;homie-protected"});
        assert_eq!((n(&v, "a"), n(&v, "b"), n(&v, "missing")), (12, 3, 0));
        assert!(flag(&v, "c"));
    }

    use crate::testkit::{json, serve};
    use axum::{
        Json, Router,
        extract::{Path, Query},
        http::StatusCode,
        routing::{get, post},
    };
    use serde_json::json;
    use std::collections::HashMap;

    /// A two-node cluster: pve-01 online, pve-02 offline.
    fn cluster(power: bool) -> Router {
        let privs = if power {
            json!({"/": {"VM.PowerMgmt": 1, "Sys.Audit": 1}})
        } else {
            json!({"/": {"Sys.Audit": 1}})
        };
        let guest = |t: &str, id: u64, name: &str, node: &str, tags: &str| {
            json!({"type": t, "vmid": id, "name": name, "node": node, "status": "running",
                   "cpu": 0.5, "maxcpu": 2, "mem": 1024, "maxmem": "2048", "uptime": 60.0, "tags": tags})
        };
        json(
            "/api2/json/cluster/status",
            json!({"data": [
                {"type": "cluster", "name": "lab", "quorate": 1},
                {"type": "node", "name": "pve-01", "online": 1},
                {"type": "node", "name": "pve-02", "online": 0},
            ]}),
        )
        .merge(json(
            "/api2/json/cluster/resources",
            json!({"data": [
                {"type": "node", "node": "pve-02", "status": "offline"},
                {"type": "node", "node": "pve-01", "status": "online", "cpu": 0.1, "maxcpu": 8,
                 "mem": 1, "maxmem": 2, "disk": 3, "maxdisk": 4, "uptime": 5},
                {"type": "node", "node": "a b", "status": "online"},
                {"type": "storage", "storage": "local", "node": "pve-01", "status": "available",
                 "plugintype": "dir", "shared": 0, "disk": 10, "maxdisk": 100},
                {"type": "sdn", "sdn": "localnetwork"},
                guest("qemu", 101, "web", "pve-01", "prod;homie-protected"),
                guest("qemu", 100, "db", "pve-01", ""),
                guest("lxc", 102, "busy", "pve-01", "a,b c"),
                guest("lxc", 103, "noid", "pve-01", ""),
                guest("qemu", 104, "odd", "a b", ""),
            ]}),
        ))
        .merge(json(
            "/api2/json/cluster/ha/status/current",
            json!({"data": [
                {"type": "quorum", "node": "pve-01"},
                {"type": "service", "sid": "vm:101", "node": "pve-01", "state": "started"},
            ]}),
        ))
        .merge(json("/api2/json/access/permissions", json!({"data": privs})))
        .merge(json("/api2/json/version", json!({"data": {"version": "9.0.3"}})))
        .route(
            "/api2/json/nodes/{node}/tasks",
            get(|Path(node): Path<String>, Query(q): Query<HashMap<String, String>>| async move {
                if q.get("source").map(String::as_str) == Some("active") {
                    let busy = q["vmid"] == "102";
                    return Json(json!({"data": if busy { json!([{"type": "qmshutdown"}]) } else { json!([]) }}));
                }
                assert_eq!((q["errors"].as_str(), q["limit"].as_str()), ("1", "10"));
                assert!(q["since"].parse::<i64>().is_ok());
                let task = |start: u64| {
                    json!({"upid": format!("UPID:{node}:{start}"), "type": "qmstart", "id": 101,
                           "user": "root@pam", "starttime": start, "status": "error"})
                };
                Json(json!({"data": [task(10), task(20)]}))
            }),
        )
        .route(
            "/api2/json/nodes/{node}/{kind}/{vmid}/status/{action}",
            post(|Path((node, kind, vmid, action)): Path<(String, String, String, String)>| async move {
                if vmid == "103" {
                    return Json(json!({"data": null}));
                }
                Json(json!({"data": format!("UPID:{node}:{kind}:{vmid}:{action}")}))
            }),
        )
        .route(
            "/api2/json/nodes/{node}/tasks/{upid}/status",
            get(|Path((_, upid)): Path<(String, String)>| async move {
                Json(json!({"data": {"status": "stopped", "exitstatus": format!("OK {upid}")}}))
            }),
        )
    }

    async fn pve(endpoints: &[&str]) -> Proxmox {
        let cfg = serde_json::from_value(json!({
            "endpoints": endpoints, "token": "u@pve!t=x", "protected_tags": ["homie-protected"]
        }))
        .unwrap();
        Proxmox::new("lab", &cfg).await.unwrap()
    }

    fn failing() -> Router {
        Router::new().fallback(|| async { StatusCode::INTERNAL_SERVER_ERROR })
    }

    #[tokio::test]
    async fn overview_of_the_cluster() {
        let url = serve(cluster(true)).await;
        let p = pve(&[url.as_str()]).await;
        assert_eq!(p.ui_url, url);
        let o = p.overview().await.unwrap();

        assert_eq!(o.cluster.name.as_deref(), Some("lab"));
        assert_eq!(o.cluster.quorate, Some(true));
        assert_eq!((o.cluster.nodes_online, o.cluster.nodes_total), (1, 2));
        let nodes: Vec<(&str, bool)> = o
            .nodes
            .iter()
            .map(|n| (n.name.as_str(), n.online))
            .collect();
        assert_eq!(nodes, [("a b", true), ("pve-01", true), ("pve-02", false)]);
        let n1 = &o.nodes[1];
        assert_eq!(
            (n1.maxcpu, n1.mem, n1.maxmem, n1.disk, n1.maxdisk, n1.uptime),
            (8, 1, 2, 3, 4, 5)
        );
        assert_eq!(n1.cpu, 0.1);

        let ids: Vec<u64> = o.guests.iter().map(|g| g.vmid).collect();
        assert_eq!(ids, [100, 101, 102, 103, 104]);
        let web = &o.guests[1];
        assert!(web.protected && !web.template);
        assert_eq!(web.tags, ["prod", "homie-protected"]);
        assert_eq!((web.maxmem, web.uptime), (2048, 60));
        assert_eq!(o.guests[2].tags, ["a", "b", "c"]);
        assert!(!o.guests[0].protected);

        assert_eq!(o.storage.len(), 1);
        let st = &o.storage[0];
        assert_eq!(
            (st.storage.as_str(), st.plugintype.as_str(), st.shared),
            ("local", "dir", false)
        );
        assert_eq!((st.disk, st.maxdisk), (10, 100));

        assert_eq!(o.ha.len(), 1);
        assert_eq!(
            (o.ha[0].sid.as_str(), o.ha[0].state.as_str()),
            ("vm:101", "started")
        );
        assert!(o.can_power);

        // only the safe online node is asked, newest failure first
        let tasks: Vec<(&str, u64)> = o
            .failed_tasks
            .iter()
            .map(|t| (t.node.as_str(), t.starttime))
            .collect();
        assert_eq!(tasks, [("pve-01", 20), ("pve-01", 10)]);
        assert_eq!(o.failed_tasks[0].upid, "UPID:pve-01:20");
        assert_eq!(
            (
                o.failed_tasks[0].user.as_str(),
                o.failed_tasks[0].kind.as_str()
            ),
            ("root@pam", "qmstart")
        );
        assert_eq!(o.failed_tasks[0].id, "101");
    }

    /// HA, permissions and tasks are optional: their errors only empty them.
    #[tokio::test]
    async fn overview_without_extras() {
        let app = json("/api2/json/cluster/status", json!({"data": []})).merge(json(
            "/api2/json/cluster/resources",
            json!({"data": [{"type": "node", "node": "pve-01", "status": "online"}, {"type": "qemu", "vmid": 7}]}),
        ));
        let o = pve(&[serve(app).await.as_str()])
            .await
            .overview()
            .await
            .unwrap();
        assert_eq!(
            (o.cluster.name, o.cluster.quorate, o.cluster.nodes_total),
            (None, None, 0)
        );
        assert!(o.ha.is_empty() && o.failed_tasks.is_empty() && !o.can_power);
        assert_eq!(o.nodes.len(), 1);
        assert_eq!((o.guests[0].vmid, o.guests[0].name.as_str()), (7, ""));
        assert!(o.guests[0].tags.is_empty());

        let err = pve(&[serve(failing()).await.as_str()])
            .await
            .overview()
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("500"), "{err}");
    }

    fn req(vmid: u64, kind: &str, action: &str, confirm: Option<&str>) -> PowerRequest {
        PowerRequest {
            node: "pve-01".into(),
            vmid,
            kind: kind.into(),
            action: action.into(),
            confirm: confirm.map(String::from),
        }
    }

    #[tokio::test]
    async fn power_actions() {
        let p = pve(&[serve(cluster(true)).await.as_str()]).await;
        let p = &p;
        let err = |r: PowerRequest| async move { p.power(&r).await.err().unwrap().to_string() };

        assert_eq!(
            p.power(&req(100, "qemu", "start", None)).await.unwrap(),
            "UPID:pve-01:qemu:100:start"
        );
        assert_eq!(
            err(req(100, "qemu", "destroy", None)).await,
            "unknown action \"destroy\""
        );
        assert_eq!(
            err(req(100, "lxc", "start", None)).await,
            "no lxc 100 on node \"pve-01\""
        );
        assert!(
            err(req(100, "qemu", "stop", None))
                .await
                .contains("hard power-off")
        );
        assert_eq!(
            p.power(&req(100, "qemu", "stop", Some("db")))
                .await
                .unwrap(),
            "UPID:pve-01:qemu:100:stop"
        );
        assert!(
            err(req(101, "qemu", "reboot", None))
                .await
                .contains("web is protected")
        );
        assert_eq!(
            p.power(&req(101, "qemu", "reboot", Some("web")))
                .await
                .unwrap(),
            "UPID:pve-01:qemu:101:reboot"
        );
        assert_eq!(
            err(req(102, "lxc", "shutdown", None)).await,
            "busy already has a running task (qmshutdown); wait for it to finish"
        );
        assert_eq!(
            err(req(103, "lxc", "start", None)).await,
            "Proxmox returned no task id"
        );
        let mut odd = req(104, "qemu", "start", None);
        odd.node = "a b".into();
        assert_eq!(err(odd).await, "unexpected node name");
    }

    #[tokio::test]
    async fn task_status_and_probe() {
        let p = pve(&[serve(cluster(true)).await.as_str()]).await;
        let t = p.task("pve-01", "UPID:pve-01:1:qmstart:").await.unwrap();
        assert_eq!(t.status, "stopped");
        assert_eq!(t.exitstatus.as_deref(), Some("OK UPID:pve-01:1:qmstart:"));
        for (node, upid) in [
            ("pve-01", "not-a-upid"),
            ("../x", "UPID:a"),
            ("pve-01", "UPID:a/b"),
        ] {
            let err = p.task(node, upid).await.err().unwrap();
            assert_eq!(err.to_string(), "invalid node or task id");
        }
        assert_eq!(
            p.probe().await.unwrap(),
            "Proxmox VE 9.0.3, power actions enabled"
        );
        let p = pve(&[serve(cluster(false)).await.as_str()]).await;
        assert_eq!(
            p.probe().await.unwrap(),
            "Proxmox VE 9.0.3, power actions off (no VM.PowerMgmt)"
        );
    }

    /// GETs move past a 5xx endpoint; POSTs do not, a failed POST may have run.
    #[tokio::test]
    async fn failover_rules() {
        let (bad, good) = (serve(failing()).await, serve(cluster(true)).await);
        let p = pve(&[bad.as_str(), good.as_str()]).await;
        assert_eq!(p.get("/version").await.unwrap()["version"], "9.0.3");
        assert_eq!(p.preferred.load(Ordering::Relaxed), 1);

        let p = pve(&[bad.as_str(), good.as_str()]).await;
        let err = p
            .call(true, "/nodes/pve-01/qemu/100/status/start")
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("500"), "{err}");

        let err = pve(&[bad.as_str()])
            .await
            .get("/version")
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("500"), "{err}");

        let down = pve(&["http://127.0.0.1:1", "http://127.0.0.1:1"]).await;
        assert!(down.get("/version").await.is_err());
    }

    #[tokio::test]
    async fn config_errors_and_ui_url() {
        let cfg = |v: serde_json::Value| -> config::Proxmox { serde_json::from_value(v).unwrap() };
        let err = Proxmox::new("lab", &cfg(json!({"endpoints": ["nope"], "token": "t"})))
            .await
            .err()
            .unwrap();
        assert!(err.to_string().starts_with("proxmox.lab nope: "), "{err}");
        let err = Proxmox::new(
            "lab",
            &cfg(json!({"endpoints": ["http://127.0.0.1:1"], "token": {"env": "HOMIE_TEST_UNSET_PVE_TOKEN"}})),
        )
        .await
        .err()
        .unwrap();
        assert!(err.to_string().starts_with("proxmox.lab.token: "), "{err}");
        let p = Proxmox::new(
            "lab",
            &cfg(json!({"endpoints": ["http://127.0.0.1:1"], "token": "t",
                        "ui_url": "https://pve.example.com", "quiet": true})),
        )
        .await
        .unwrap();
        assert_eq!(
            (p.ui_url.as_str(), p.quiet, p.name.as_str()),
            ("https://pve.example.com", true, "lab")
        );
    }
}

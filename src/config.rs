//! Configuration: built-in defaults, then an optional TOML file, then
//! `HOMIE__<SECTION>__<KEY>` environment variables (highest precedence).

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use serde::{Deserialize, Serialize};

use crate::{Result, secret::Secret};

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: String,
    pub public_url: String,
    pub state_dir: Option<PathBuf>,
    pub refresh_seconds: u64,
    /// Push query results over server-sent events (RFC 0009)
    pub live_updates: bool,
    pub allow_unauthenticated: bool,
    pub quiet_hours: Option<QuietHours>,
    pub auth: Option<Auth>,
    pub local_login: Option<LocalLogin>,
    pub proxmox: BTreeMap<String, Proxmox>,
    pub kubernetes: BTreeMap<String, Kubernetes>,
    pub grafana: BTreeMap<String, Grafana>,
    pub openbao: BTreeMap<String, OpenBao>,
    pub security: Security,
    pub logs: Logs,
    pub tailscale: Option<Tailscale>,
    pub github: Option<GitHub>,
    /// read-only views for some users (RFC 0016)
    pub roles: BTreeMap<String, Role>,
    /// the passwordless "Try the demo" login (RFC 0016)
    pub demo: Option<Demo>,
    pub healthchecks: Option<Healthchecks>,
    /// certificate expiry from TLS handshakes (RFC 0018)
    pub certificates: Certificates,
    /// messages when an overview card changes status (RFC 0019)
    pub notify: Option<Notify>,
    /// what is out of date (RFC 0020)
    pub updates: Updates,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7777".into(),
            public_url: String::new(),
            state_dir: None,
            refresh_seconds: 15,
            live_updates: true,
            allow_unauthenticated: false,
            quiet_hours: None,
            auth: None,
            local_login: None,
            proxmox: BTreeMap::new(),
            kubernetes: BTreeMap::new(),
            grafana: BTreeMap::new(),
            openbao: BTreeMap::new(),
            security: Security::default(),
            logs: Logs::default(),
            tailscale: None,
            github: None,
            roles: BTreeMap::new(),
            demo: None,
            healthchecks: None,
            certificates: Certificates::default(),
            notify: None,
            updates: Updates::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    pub window: String,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub issuer: String,
    pub client_id: Secret,
    pub client_secret: Option<Secret>,
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    #[serde(default = "default_email_claim")]
    pub email_claim: String,
    #[serde(default)]
    pub groups_claim: Option<String>,
    #[serde(default)]
    pub allowed_emails: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    /// email to role name (RFC 0016); users not listed have full access
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
}

/// Pages a role can be given. The Overview is always there, with the cards
/// of the allowed pages only.
pub const PAGES: [&str; 10] = [
    "infra",
    "workloads",
    "gitops",
    "alerts",
    "logs",
    "security",
    "tailscale",
    "workflows",
    "services",
    "outage",
];

/// A read-only view (RFC 0016). Roles never run actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub pages: Vec<String>,
    /// explicit Kubernetes namespaces, exact names
    #[serde(default)]
    pub namespaces: Vec<String>,
    /// Proxmox guests are shown only with one of these tags
    #[serde(default)]
    pub proxmox_tags: Vec<String>,
    /// "namespace/name" of the ServiceAccount Homie impersonates for this
    /// role's Kubernetes reads
    #[serde(default)]
    pub kubernetes_service_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Demo {
    pub role: String,
    #[serde(default = "default_demo_sessions")]
    pub max_sessions: usize,
}

fn default_demo_sessions() -> usize {
    200
}

/// Built-in username and password login. Without `password_hash`, a random
/// password is generated at the first start and kept in `state_dir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLogin {
    #[serde(default = "default_username")]
    pub username: String,
    /// argon2 PHC string from `homie hash-password`
    #[serde(default)]
    pub password_hash: Option<Secret>,
    /// a role for this user (RFC 0016), e.g. on a public demo instance
    #[serde(default)]
    pub role: Option<String>,
}

fn default_username() -> String {
    "homie".into()
}

fn default_scopes() -> Vec<String> {
    ["openid", "email", "profile", "offline_access"]
        .map(String::from)
        .to_vec()
}

fn default_email_claim() -> String {
    "email".into()
}

/// TLS options shared by every HTTP integration.
#[derive(Debug, Clone)]
pub struct Tls {
    pub tls_verify: bool,
    pub ca_file: Option<PathBuf>,
    pub tls_server_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Endpoint {
    Url(String),
    Full {
        url: String,
        tls_verify: Option<bool>,
        ca_file: Option<PathBuf>,
        tls_server_name: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proxmox {
    pub endpoints: Vec<Endpoint>,
    pub token: Secret,
    #[serde(default = "yes")]
    pub tls_verify: bool,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub protected_tags: Vec<String>,
    #[serde(default)]
    pub ui_url: Option<String>,
    #[serde(default)]
    pub quiet: bool,
    /// embedded guest consoles (RFC 0023); the token also needs VM.Console
    #[serde(default)]
    pub console: bool,
    #[serde(default = "default_console_minutes")]
    pub console_max_minutes: u64,
}

fn default_console_minutes() -> u64 {
    60
}

impl Proxmox {
    /// Each endpoint with the section defaults filled in.
    pub fn targets(&self) -> Vec<(String, Tls)> {
        self.endpoints
            .iter()
            .map(|e| match e {
                Endpoint::Url(url) => (
                    url.clone(),
                    Tls {
                        tls_verify: self.tls_verify,
                        ca_file: self.ca_file.clone(),
                        tls_server_name: None,
                    },
                ),
                Endpoint::Full {
                    url,
                    tls_verify,
                    ca_file,
                    tls_server_name,
                } => (
                    url.clone(),
                    Tls {
                        tls_verify: tls_verify.unwrap_or(self.tls_verify),
                        ca_file: ca_file.clone().or_else(|| self.ca_file.clone()),
                        tls_server_name: tls_server_name.clone(),
                    },
                ),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubeAuth {
    Kubeconfig,
    InCluster,
    ForwardToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kubernetes {
    /// Default: `in_cluster` inside a pod, `kubeconfig` elsewhere.
    #[serde(default)]
    pub auth: Option<KubeAuth>,
    #[serde(default)]
    pub kubeconfig: Option<PathBuf>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub headlamp_url: Option<String>,
    /// `{cluster}` in link templates (Headlamp: "main" in-cluster)
    #[serde(default = "default_headlamp_cluster")]
    pub headlamp_cluster: String,
    /// Deep-link templates (RFC 0007); `headlamp_url` fills unset ones.
    #[serde(default)]
    pub links: LinkTemplates,
    /// Argo CD UI base URL for deep links to Applications (RFC 0005)
    #[serde(default)]
    pub argocd_url: Option<String>,
    /// CloudNativePG: hours after which the last backup counts as stale (RFC 0011)
    #[serde(default = "default_backup_age")]
    pub cnpg_max_backup_age_hours: f64,
    /// Node CPU and memory history for sparklines (RFC 0012)
    #[serde(default)]
    pub metrics: Option<NodeMetrics>,
    /// TLS hostnames found in this cluster, probed for expiry (RFC 0018)
    #[serde(default)]
    pub certificates: CertDiscovery,
    /// "renewal failing" from cert-manager Certificate conditions (RFC 0018)
    #[serde(default)]
    pub cert_manager: bool,
    /// the distribution behind the versions (RFC 0020): "auto", "none", a
    /// built-in name, or `{ github = "owner/repo" }`
    #[serde(default)]
    pub distro: Distro,
    /// where the Flux version label is read (RFC 0020)
    #[serde(default = "default_flux_namespace")]
    pub flux_namespace: String,
    #[serde(default)]
    pub quiet: bool,
}

fn default_flux_namespace() -> String {
    "flux-system".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Distro {
    Name(String),
    GitHub { github: String },
}

impl Default for Distro {
    fn default() -> Self {
        Distro::Name("auto".into())
    }
}

/// Pending updates (RFC 0020). Everything that calls a third party is off
/// until turned on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Updates {
    /// package list and reboot pending per Proxmox node
    pub proxmox: bool,
    pub upstream: Upstream,
    /// raises the GitHub API rate limit (60 an hour without)
    pub github_token: Option<Secret>,
    pub registries: Vec<Registry>,
    /// repositories of charts installed with plain `helm install`
    pub charts: Vec<ChartRepo>,
    /// chart (and image) globs never looked up
    pub ignore: Vec<String>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            proxmox: true,
            upstream: Upstream::default(),
            github_token: None,
            registries: Vec::new(),
            charts: Vec::new(),
            ignore: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Upstream {
    pub kubernetes: bool,
    pub distro: bool,
    pub flux: bool,
    pub charts: bool,
    /// running container images against their registries (RFC 0021)
    pub images: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub host: String,
    pub username: Secret,
    pub password: Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartRepo {
    pub name: String,
    pub repo: String,
}

/// Notifications (RFC 0019): status changes of the overview cards.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notify {
    #[serde(default = "default_notify_interval")]
    pub interval_seconds: u64,
    /// how long a new status holds before a message
    #[serde(default = "default_notify_hold")]
    pub hold_seconds: u64,
    #[serde(default = "default_min_tone")]
    pub min_tone: crate::status::Tone,
    /// per card; amber is the stage to act on for certificates and backups
    #[serde(default = "default_min_tone_per_card")]
    pub min_tone_per_card: BTreeMap<String, crate::status::Tone>,
    #[serde(default = "yes")]
    pub recovery: bool,
    /// card ids to evaluate; empty: all but `alerts` (Grafana notifies itself)
    #[serde(default)]
    pub cards: Vec<String>,
    /// send the card and its status only, no names
    #[serde(default)]
    pub redact: bool,
    /// "namespace/name" impersonated for forward_token clusters
    #[serde(default)]
    pub kubernetes_service_account: Option<String>,
    pub targets: Vec<NotifyTarget>,
}

fn default_notify_interval() -> u64 {
    60
}

fn default_notify_hold() -> u64 {
    180
}

fn default_min_tone() -> crate::status::Tone {
    crate::status::Tone::Bad
}

fn default_min_tone_per_card() -> BTreeMap<String, crate::status::Tone> {
    [
        ("certificates", crate::status::Tone::Warn),
        ("cnpg", crate::status::Tone::Warn),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyTarget {
    #[serde(rename = "type")]
    pub kind: NotifyKind,
    pub url: Secret,
    /// bearer token (an ntfy access token)
    #[serde(default)]
    pub token: Option<Secret>,
    #[serde(default)]
    pub headers: BTreeMap<String, Secret>,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    Ntfy,
    Slack,
    Webhook,
}

/// Certificate expiry (RFC 0018): thresholds and the targets to probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Certificates {
    /// amber for leaf and intermediate certificates
    pub warn_days: u32,
    /// amber for roots from a `ca_file`; bundled public roots never are
    pub root_warn_days: u32,
    /// how often a listed or discovered target is probed again
    pub interval_hours: u32,
    /// record the certificates of the integrations' own endpoints
    pub endpoints: bool,
    pub targets: Vec<CertTarget>,
}

impl Default for Certificates {
    fn default() -> Self {
        Self {
            warn_days: 14,
            root_warn_days: 90,
            interval_hours: 6,
            endpoints: true,
            targets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertTarget {
    /// sent as SNI and checked against the certificate's names
    pub host: String,
    #[serde(default = "default_tls_port")]
    pub port: u16,
    /// connect here instead of resolving `host`
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
}

fn default_tls_port() -> u16 {
    443
}

/// Where a cluster's TLS hostnames come from. Each source only yields
/// hostnames; they are probed like listed targets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CertDiscovery {
    pub discover: bool,
    pub sources: Vec<CertSource>,
    /// annotation keys read by the `annotations` source
    pub annotations: Vec<String>,
    /// `host:port` to connect to instead of resolving each hostname
    pub discover_address: Option<String>,
    /// extra roots for discovered hostnames (a private CA)
    pub ca_file: Option<PathBuf>,
}

impl Default for CertDiscovery {
    fn default() -> Self {
        Self {
            discover: false,
            sources: vec![
                CertSource::Ingress,
                CertSource::Gateway,
                CertSource::Annotations,
            ],
            annotations: vec!["external-dns.alpha.kubernetes.io/hostname".into()],
            discover_address: None,
            ca_file: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertSource {
    Ingress,
    Gateway,
    Annotations,
}

/// PromQL range queries, one series per node, for sparklines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeMetrics {
    pub grafana: String,
    pub datasource: String,
    pub cpu: String,
    pub memory: String,
    /// label holding the node name in both series
    #[serde(default = "default_node_label")]
    pub node_label: String,
}

fn default_node_label() -> String {
    "node".into()
}

/// URL templates with `{cluster}`, `{namespace}`, `{name}`, `{kind}`,
/// `{plural}`, and for Flux `{group}` (kustomize, helm, source).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LinkTemplates {
    pub node: Option<String>,
    pub pod: Option<String>,
    pub workload: Option<String>,
    pub flux: Option<String>,
}

fn default_backup_age() -> f64 {
    26.0
}

fn default_headlamp_cluster() -> String {
    "main".into()
}

impl Kubernetes {
    pub fn auth_mode(&self) -> KubeAuth {
        self.auth
            .unwrap_or(if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
                KubeAuth::InCluster
            } else {
                KubeAuth::Kubeconfig
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grafana {
    pub url: String,
    /// where browsers open Grafana when `url` is internal (a cluster Service)
    #[serde(default)]
    pub ui_url: Option<String>,
    pub token: Secret,
    #[serde(default = "yes")]
    pub tls_verify: bool,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub tls_server_name: Option<String>,
    #[serde(default)]
    pub quiet: bool,
    /// Alert names never listed, such as an always-firing heartbeat.
    #[serde(default = "default_ignore_alerts")]
    pub ignore_alerts: Vec<String>,
}

fn default_ignore_alerts() -> Vec<String> {
    vec!["Watchdog".into()]
}

/// Log search (RFC 0008): only these datasources can be queried.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Logs {
    pub datasources: Vec<LogSource>,
    pub max_rows: usize,
    /// live tail (RFC 0022): poll interval and how long a tail runs; 0 turns it off
    pub tail_seconds: u64,
    pub tail_max_minutes: u64,
}

impl Default for Logs {
    fn default() -> Self {
        Self {
            datasources: Vec::new(),
            max_rows: 1000,
            tail_seconds: 5,
            tail_max_minutes: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogSource {
    pub label: String,
    pub grafana: String,
    pub datasource: String,
    #[serde(rename = "type")]
    pub kind: LogKind,
    /// query for one pod, with {namespace} and {pod}
    #[serde(default)]
    pub pod_query: Option<String>,
    /// query for one node or host, with {node}
    #[serde(default)]
    pub node_query: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogKind {
    Victorialogs,
    Loki,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Security {
    pub kyverno: Option<KyvernoPreset>,
    pub tetragon: Option<LogPreset>,
    pub wazuh: Option<WazuhPreset>,
    pub debsecan: Option<MetricPreset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KyvernoPreset {
    pub kubernetes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogPreset {
    pub grafana: String,
    pub datasource: String,
    /// LogsQL stream filter that selects the tool's output. It depends on the
    /// log collector, so it is configurable.
    #[serde(default = "default_tetragon_filter")]
    pub filter: String,
}

fn default_tetragon_filter() -> String {
    "k8s.container.name:export-stdout".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WazuhPreset {
    pub grafana: String,
    pub datasource: String,
    #[serde(default = "default_wazuh_filter")]
    pub filter: String,
    #[serde(default = "default_wazuh_level")]
    pub min_level: u8,
}

fn default_wazuh_filter() -> String {
    "k8s.namespace.name:wazuh k8s.container.name:alerts".into()
}

fn default_wazuh_level() -> u8 {
    7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricPreset {
    pub grafana: String,
    pub datasource: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tailscale {
    #[serde(default = "default_tailnet")]
    pub tailnet: String,
    #[serde(default)]
    pub api_key: Option<Secret>,
    #[serde(default)]
    pub oauth_client_id: Option<Secret>,
    #[serde(default)]
    pub oauth_client_secret: Option<Secret>,
    #[serde(default = "default_hosts")]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub quiet_hosts: Vec<String>,
}

fn default_tailnet() -> String {
    "-".into()
}

fn default_hosts() -> Vec<String> {
    vec!["*".into()]
}

/// GitHub Actions workflow runs for a shared watch list (RFC 0014).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHub {
    #[serde(default)]
    pub app_id: Option<u64>,
    /// the GitHub App's PEM private key
    #[serde(default)]
    pub private_key: Option<Secret>,
    /// a fine-grained token instead of the app
    #[serde(default)]
    pub token: Option<Secret>,
    /// the app's user authorization, for the picking helper only
    #[serde(default)]
    pub client_id: Option<Secret>,
    #[serde(default)]
    pub client_secret: Option<Secret>,
    #[serde(default)]
    pub watch: Vec<Watch>,
    #[serde(default = "default_github_api")]
    pub api_url: String,
    #[serde(default)]
    pub quiet: bool,
    /// where the picker saves watches (RFC 0015)
    #[serde(default)]
    pub store: WatchStore,
    /// the app's install page, offered when a signed-in user sees no
    /// repositories (RFC 0017), e.g. https://github.com/apps/<slug>/installations/new
    #[serde(default)]
    pub install_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchStore {
    /// a ConfigMap when running in a cluster, else a file in state_dir, else off
    #[default]
    Auto,
    Configmap,
    File,
    /// paste-only, as in RFC 0014
    Off,
    /// per Homie session, read with the visitor's own GitHub token (RFC 0017)
    Session,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    /// owner/name
    pub repo: String,
    /// workflow file name (terraform-apply.yml) or numeric id
    pub workflow: String,
}

fn default_github_api() -> String {
    "https://api.github.com".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Healthchecks {
    #[serde(default = "default_healthchecks_url")]
    pub url: String,
    pub api_key: Secret,
    #[serde(default = "yes")]
    pub tls_verify: bool,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub tls_server_name: Option<String>,
    #[serde(default)]
    pub quiet: bool,
}

/// OpenBao or Vault, for its unauthenticated health (RFC 0011).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenBao {
    pub url: String,
    #[serde(default = "yes")]
    pub tls_verify: bool,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub tls_server_name: Option<String>,
    #[serde(default)]
    pub quiet: bool,
}

impl OpenBao {
    pub fn tls(&self) -> Tls {
        Tls {
            tls_verify: self.tls_verify,
            ca_file: self.ca_file.clone(),
            tls_server_name: self.tls_server_name.clone(),
        }
    }
}

impl Grafana {
    pub fn tls(&self) -> Tls {
        Tls {
            tls_verify: self.tls_verify,
            ca_file: self.ca_file.clone(),
            tls_server_name: self.tls_server_name.clone(),
        }
    }
}

impl Healthchecks {
    pub fn tls(&self) -> Tls {
        Tls {
            tls_verify: self.tls_verify,
            ca_file: self.ca_file.clone(),
            tls_server_name: self.tls_server_name.clone(),
        }
    }
}

fn default_healthchecks_url() -> String {
    "https://healthchecks.io".into()
}

/// `~/x` to `$HOME/x`. Other paths are returned as they are.
pub fn expand(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

pub fn load(explicit: Option<PathBuf>) -> Result<Config> {
    let explicit = explicit.or_else(|| std::env::var_os("HOMIE_CONFIG").map(PathBuf::from));
    let file = match explicit {
        Some(p) => {
            let p = expand(&p);
            if !p.exists() {
                return Err(format!("config file {} does not exist", p.display()).into());
            }
            Some(p)
        }
        None => [
            expand(Path::new("~/.config/homie/config.toml")),
            PathBuf::from("/etc/homie/config.toml"),
        ]
        .into_iter()
        .find(|p| p.exists()),
    };

    let mut fig = Figment::new();
    if let Some(p) = &file {
        fig = fig.merge(Toml::file_exact(p));
    }
    let mut cfg: Config = fig
        .merge(Env::prefixed("HOMIE__").split("__"))
        .extract()
        .map_err(|e| format!("config: {e}"))?;
    match &file {
        Some(p) => tracing::info!(msg = "config", file = %p.display()),
        None => tracing::info!(msg = "config", file = "none, environment only"),
    }
    cfg.state_dir = cfg.state_dir.as_deref().map(expand);
    validate(&cfg)?;
    Ok(cfg)
}

/// DNS-1123 label: Kubernetes namespace and ServiceAccount names.
fn valid_namespace(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_url(what: &str, url: &str) -> Result<url::Url> {
    let u = url::Url::parse(url).map_err(|e| format!("{what}: {url:?} is not a URL: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(format!("{what}: {url:?} must be http:// or https://").into());
    }
    Ok(u)
}

pub fn validate(cfg: &Config) -> Result<()> {
    let listen: SocketAddr = cfg
        .listen
        .parse()
        .map_err(|e| format!("listen: {:?} is not ip:port: {e}", cfg.listen))?;
    if cfg.refresh_seconds < 5 {
        return Err("refresh_seconds must be 5 or more".into());
    }
    for (name, r) in &cfg.roles {
        if let Some(p) = r.pages.iter().find(|p| !PAGES.contains(&p.as_str())) {
            return Err(
                format!("roles.{name}.pages: unknown page {p:?} (one of {PAGES:?})").into(),
            );
        }
        if let Some(n) = r.namespaces.iter().find(|n| !valid_namespace(n)) {
            return Err(format!(
                "roles.{name}.namespaces: {n:?} is not a namespace name (no globs)"
            )
            .into());
        }
        let kube_pages = r
            .pages
            .iter()
            .any(|p| matches!(p.as_str(), "workloads" | "gitops"));
        if kube_pages && !cfg.kubernetes.is_empty() {
            if r.namespaces.is_empty() {
                return Err(format!("roles.{name}: list the namespaces it may see").into());
            }
            if !r
                .kubernetes_service_account
                .as_deref()
                .and_then(|s| s.split_once('/'))
                .is_some_and(|(ns, sa)| valid_namespace(ns) && valid_namespace(sa))
            {
                return Err(format!(
                    "roles.{name}.kubernetes_service_account: set \"namespace/name\" of a read-only ServiceAccount"
                )
                .into());
            }
        }
    }
    let role_known = |what: &str, r: &str| -> Result<()> {
        if cfg.roles.contains_key(r) {
            Ok(())
        } else {
            Err(format!("{what}: no [roles.{r}]").into())
        }
    };
    if let Some(d) = &cfg.demo {
        role_known("demo.role", &d.role)?;
        if cfg.auth.is_none() && cfg.local_login.is_none() {
            return Err("demo: needs a login method ([auth] or [local_login])".into());
        }
    }
    for r in cfg.auth.iter().flat_map(|a| a.roles.values()) {
        role_known("auth.roles", r)?;
    }
    if let Some(r) = cfg.local_login.as_ref().and_then(|l| l.role.as_ref()) {
        role_known("local_login.role", r)?;
    }
    if let Some(q) = &cfg.quiet_hours {
        crate::quiet::Quiet::new(&q.window, q.timezone.as_deref())?;
    }

    let login = cfg.auth.is_some() || cfg.local_login.is_some();
    if !login && !listen.ip().is_loopback() && !cfg.allow_unauthenticated {
        return Err(format!(
            "listen {listen} is not loopback and no login is configured: anyone who can reach it \
             could use your tokens. Configure [local_login] or [auth], listen on 127.0.0.1, or set \
             allow_unauthenticated = true behind an auth proxy or a loopback-only port mapping"
        )
        .into());
    }
    if login {
        let u = check_url("public_url", &cfg.public_url)?;
        let localhost = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if u.scheme() != "https" && !localhost && cfg.auth.is_some() {
            return Err("public_url must be https:// when [auth] is configured".into());
        }
    }
    if let Some(l) = &cfg.local_login {
        if l.username.trim().is_empty() {
            return Err("local_login.username is empty".into());
        }
        if l.password_hash.is_none() && cfg.state_dir.is_none() {
            return Err(
                "local_login: set password_hash (from `homie hash-password`), or state_dir so a \
                 generated password can be kept there"
                    .into(),
            );
        }
    }
    if let Some(a) = &cfg.auth {
        check_url("auth.issuer", &a.issuer)?;
        if a.allowed_emails.is_empty() && a.allowed_groups.is_empty() {
            return Err("auth: set allowed_emails or allowed_groups, or nobody can log in".into());
        }
        if !a.allowed_groups.is_empty() && a.groups_claim.is_none() {
            return Err("auth.allowed_groups needs auth.groups_claim".into());
        }
    }

    if let Some(n) = &cfg.notify {
        if n.targets.is_empty() {
            return Err("notify.targets is empty".into());
        }
        if n.interval_seconds < 10 {
            return Err("notify.interval_seconds must be 10 or more".into());
        }
        use crate::status::Tone;
        let thresholds = std::iter::once(&n.min_tone).chain(n.min_tone_per_card.values());
        if thresholds
            .into_iter()
            .any(|t| !matches!(t, Tone::Warn | Tone::Bad))
        {
            return Err("notify: min_tone values are \"warn\" or \"bad\"".into());
        }
        if let Some(sa) = &n.kubernetes_service_account
            && !sa
                .split_once('/')
                .is_some_and(|(ns, name)| valid_namespace(ns) && valid_namespace(name))
        {
            return Err(
                "notify.kubernetes_service_account: set \"namespace/name\" of a read-only ServiceAccount".into(),
            );
        }
    }
    for r in &cfg.updates.charts {
        if !(r.repo.starts_with("oci://")
            || r.repo.starts_with("https://")
            || r.repo.starts_with("http://"))
        {
            return Err(format!(
                "updates.charts: repo {:?} must be oci://, https:// or http://",
                r.repo
            )
            .into());
        }
    }
    let c = &cfg.certificates;
    if c.interval_hours == 0 {
        return Err("certificates.interval_hours must be 1 or more".into());
    }
    for t in &c.targets {
        if t.host.trim().is_empty() || t.host.contains(['/', ':']) {
            return Err(format!(
                "certificates.targets: host {:?} must be a name, without scheme or port",
                t.host
            )
            .into());
        }
    }

    for (name, p) in &cfg.proxmox {
        if p.endpoints.is_empty() {
            return Err(format!("proxmox.{name}.endpoints is empty").into());
        }
        for (url, _) in p.targets() {
            check_url(&format!("proxmox.{name}.endpoints"), &url)?;
        }
        if let Some(u) = &p.ui_url {
            check_url(&format!("proxmox.{name}.ui_url"), u)?;
        }
    }
    for (name, k) in &cfg.kubernetes {
        if let Some(u) = &k.headlamp_url {
            check_url(&format!("kubernetes.{name}.headlamp_url"), u)?;
        }
        if let Some(u) = &k.argocd_url {
            check_url(&format!("kubernetes.{name}.argocd_url"), u)?;
        }
        if let Some(m) = &k.metrics
            && !cfg.grafana.contains_key(&m.grafana)
        {
            return Err(format!(
                "kubernetes.{name}.metrics.grafana: no [grafana.{}]",
                m.grafana
            )
            .into());
        }
        let l = &k.links;
        for (key, t) in [
            ("node", &l.node),
            ("pod", &l.pod),
            ("workload", &l.workload),
            ("flux", &l.flux),
        ] {
            if let Some(t) = t
                && !(t.starts_with("http://") || t.starts_with("https://"))
            {
                return Err(format!(
                    "kubernetes.{name}.links.{key} must start with http:// or https://"
                )
                .into());
            }
        }
        if let Some(a) = &k.certificates.discover_address
            && !a
                .rsplit_once(':')
                .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok())
        {
            return Err(format!(
                "kubernetes.{name}.certificates.discover_address: {a:?} must be host:port"
            )
            .into());
        }
        if k.auth_mode() == KubeAuth::ForwardToken && cfg.auth.is_none() {
            return Err(format!("kubernetes.{name}: auth = \"forward_token\" needs [auth]").into());
        }
    }
    for (name, b) in &cfg.openbao {
        check_url(&format!("openbao.{name}.url"), &b.url)?;
    }
    for (name, g) in &cfg.grafana {
        check_url(&format!("grafana.{name}.url"), &g.url)?;
        if let Some(u) = &g.ui_url {
            check_url(&format!("grafana.{name}.ui_url"), u)?;
        }
    }
    let s = &cfg.security;
    if let Some(k) = &s.kyverno
        && !cfg.kubernetes.contains_key(&k.kubernetes)
    {
        return Err(format!(
            "security.kyverno.kubernetes: no [kubernetes.{}]",
            k.kubernetes
        )
        .into());
    }
    for (preset, grafana) in [
        ("tetragon", s.tetragon.as_ref().map(|p| &p.grafana)),
        ("wazuh", s.wazuh.as_ref().map(|p| &p.grafana)),
        ("debsecan", s.debsecan.as_ref().map(|p| &p.grafana)),
    ] {
        if let Some(g) = grafana
            && !cfg.grafana.contains_key(g)
        {
            return Err(format!("security.{preset}.grafana: no [grafana.{g}]").into());
        }
    }
    if !(2..=60).contains(&cfg.logs.tail_seconds) {
        return Err("logs.tail_seconds must be 2 to 60".into());
    }
    let mut labels = std::collections::HashSet::new();
    for s in &cfg.logs.datasources {
        if !cfg.grafana.contains_key(&s.grafana) {
            return Err(
                format!("logs.datasources {:?}: no [grafana.{}]", s.label, s.grafana).into(),
            );
        }
        if !labels.insert(&s.label) {
            return Err(format!("logs.datasources: label {:?} used twice", s.label).into());
        }
    }
    if let Some(t) = &cfg.tailscale {
        let oauth = t.oauth_client_id.is_some() && t.oauth_client_secret.is_some();
        if t.api_key.is_none() && !oauth {
            return Err(
                "tailscale: set api_key, or oauth_client_id and oauth_client_secret".into(),
            );
        }
    }
    if let Some(g) = &cfg.github {
        if g.store == WatchStore::Session {
            // Homie never reads as the app: only the visitors' sign-in is needed
            if g.client_id.is_none() || g.client_secret.is_none() {
                return Err("github: store = \"session\" needs client_id and client_secret".into());
            }
            if !g.watch.is_empty() {
                return Err("github: store = \"session\" takes no [[github.watch]] (each visitor picks their own)".into());
            }
        } else if !(g.app_id.is_some() && g.private_key.is_some()) && g.token.is_none() {
            return Err("github: set app_id and private_key, or token".into());
        }
        check_url("github.api_url", &g.api_url)?;
        if let Some(u) = &g.install_url {
            check_url("github.install_url", u)?;
        }
        for w in &g.watch {
            if !crate::integrations::github::valid_repo(&w.repo) {
                return Err(format!("github.watch: {:?} must be owner/name", w.repo).into());
            }
            let ok = !w.workflow.is_empty()
                && w.workflow
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
            if !ok {
                return Err(format!(
                    "github.watch: workflow {:?} must be a file name or id",
                    w.workflow
                )
                .into());
            }
        }
    }
    if let Some(h) = &cfg.healthchecks {
        check_url("healthchecks.url", &h.url)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::result_large_err)] // figment's Jail closures return figment::Error
mod tests {
    use super::*;
    use figment::Jail;

    #[test]
    fn file_then_env_layering() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "homie.toml",
                r#"
                refresh_seconds = 30
                [proxmox.lab]
                endpoints = ["https://pve1:8006", { url = "https://10.0.0.2:8006", tls_server_name = "pve2.example" }]
                token = { env = "PVE_TOKEN" }
                tls_verify = false
                "#,
            )?;
            jail.set_env("HOMIE__REFRESH_SECONDS", "20");
            jail.set_env("HOMIE__GRAFANA__CLOUD__URL", "https://x.grafana.net");
            jail.set_env("HOMIE__GRAFANA__CLOUD__TOKEN", "glsa_x");
            let cfg = load(Some("homie.toml".into())).unwrap();
            assert_eq!(cfg.refresh_seconds, 20, "env wins over the file");
            let t = cfg.proxmox["lab"].targets();
            assert!(
                !t[0].1.tls_verify && !t[1].1.tls_verify,
                "section default applies"
            );
            assert_eq!(t[1].1.tls_server_name.as_deref(), Some("pve2.example"));
            assert!(matches!(cfg.grafana["cloud"].token, Secret::Plain(_)));
            Ok(())
        });
    }

    #[test]
    fn env_only_and_typos() {
        Jail::expect_with(|jail| {
            let home = jail.directory().display().to_string();
            jail.set_env("HOME", home);
            jail.set_env("HOMIE__LISTEN", "127.0.0.1:9000");
            assert_eq!(load(None).unwrap().listen, "127.0.0.1:9000");
            jail.set_env("HOMIE__LISTN", "x");
            assert!(load(None).is_err(), "unknown keys are rejected");
            Ok(())
        });
    }

    /// The shipped examples must stay loadable. reference-lab.toml keeps
    /// <placeholders> in URLs, so it is only parsed, not validated.
    #[test]
    fn examples_load() {
        for name in ["local", "server-oidc", "reference-lab"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("examples/{name}.toml"));
            let cfg: Config = Figment::new()
                .merge(Toml::file_exact(&path))
                .extract()
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            if name != "reference-lab" {
                validate(&cfg).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
    }

    #[test]
    fn github_session_store() {
        let gh = |v: serde_json::Value| -> GitHub { serde_json::from_value(v).unwrap() };
        let mut cfg = Config {
            allow_unauthenticated: true,
            ..Config::default()
        };
        cfg.github = Some(gh(serde_json::json!({
            "store": "session", "client_id": "Iv1.x", "client_secret": "s"
        })));
        assert!(validate(&cfg).is_ok(), "no app key needed");
        cfg.github = Some(gh(
            serde_json::json!({"store": "session", "client_id": "Iv1.x"}),
        ));
        assert!(validate(&cfg).is_err(), "needs the client secret");
        cfg.github = Some(gh(serde_json::json!({
            "store": "session", "client_id": "Iv1.x", "client_secret": "s",
            "watch": [{"repo": "o/r", "workflow": "ci.yml"}]
        })));
        assert!(validate(&cfg).is_err(), "no shared watches");
        cfg.github = Some(gh(serde_json::json!({
            "store": "session", "client_id": "Iv1.x", "client_secret": "s", "install_url": "javascript:x"
        })));
        assert!(validate(&cfg).is_err(), "install_url must be http(s)");
    }

    #[test]
    fn roles_and_demo() {
        let mut cfg = Config {
            allow_unauthenticated: true,
            ..Config::default()
        };
        let k: Kubernetes =
            serde_json::from_value(serde_json::json!({"auth": "kubeconfig"})).unwrap();
        cfg.kubernetes.insert("main".into(), k);
        let role = |v: serde_json::Value| -> Role { serde_json::from_value(v).unwrap() };
        cfg.roles.insert(
            "demo".into(),
            role(
                serde_json::json!({"pages": ["infra", "workloads"], "namespaces": ["apps"],
                "kubernetes_service_account": "homie/homie-demo"}),
            ),
        );
        assert!(validate(&cfg).is_ok());
        for (bad, why) in [
            (serde_json::json!({"pages": ["admin"]}), "unknown page"),
            (
                serde_json::json!({"pages": ["workloads"], "kubernetes_service_account": "homie/d"}),
                "no namespaces",
            ),
            (
                serde_json::json!({"pages": ["workloads"], "namespaces": ["apps-*"], "kubernetes_service_account": "homie/d"}),
                "glob",
            ),
            (
                serde_json::json!({"pages": ["gitops"], "namespaces": ["apps"]}),
                "no service account",
            ),
            (
                serde_json::json!({"pages": ["gitops"], "namespaces": ["apps"], "kubernetes_service_account": "d"}),
                "not ns/name",
            ),
        ] {
            cfg.roles.insert("bad".into(), role(bad));
            assert!(validate(&cfg).is_err(), "{why}");
        }
        cfg.roles.remove("bad");
        assert!(
            serde_json::from_value::<Role>(serde_json::json!({"pages": [], "actions": true}))
                .is_err(),
            "roles never act"
        );
        cfg.demo = Some(Demo {
            role: "nope".into(),
            max_sessions: 1,
        });
        assert!(validate(&cfg).is_err(), "unknown demo role");
        cfg.demo = Some(Demo {
            role: "demo".into(),
            max_sessions: 1,
        });
        assert!(validate(&cfg).is_err(), "demo without a login method");
    }

    #[test]
    fn loopback_guard_and_auth_rules() {
        let mut cfg = Config {
            listen: "0.0.0.0:7777".into(),
            ..Config::default()
        };
        assert!(validate(&cfg).is_err());
        cfg.allow_unauthenticated = true;
        assert!(validate(&cfg).is_ok());

        let auth: Auth = serde_json::from_value(serde_json::json!({
            "issuer": "https://idp.example", "client_id": "homie"
        }))
        .unwrap();
        cfg.auth = Some(auth);
        cfg.public_url = "https://homie.example".into();
        assert!(validate(&cfg).is_err(), "empty allowlists");
        cfg.auth.as_mut().unwrap().allowed_emails = vec!["me@example.com".into()];
        assert!(validate(&cfg).is_ok());
        cfg.public_url = "http://homie.example".into();
        assert!(validate(&cfg).is_err(), "plain http public_url with auth");

        // password login alone may listen anywhere and use plain http on a LAN
        let mut cfg = Config {
            listen: "0.0.0.0:7777".into(),
            ..Config::default()
        };
        cfg.public_url = "http://192.168.1.5:7777".into();
        cfg.local_login = Some(LocalLogin {
            username: "homie".into(),
            password_hash: None,
            role: None,
        });
        assert!(validate(&cfg).is_err(), "no hash and no state_dir");
        cfg.state_dir = Some("/tmp/homie".into());
        assert!(validate(&cfg).is_ok());

        // link targets must be http(s): they become href attributes in the UI
        let k: Kubernetes = serde_json::from_value(serde_json::json!({
            "auth": "kubeconfig", "headlamp_url": "javascript:alert(1)"
        }))
        .unwrap();
        cfg.kubernetes.insert("x".into(), k);
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn validation_errors_name_the_setting() {
        let check = |toml: &str| -> Result<()> {
            let cfg: Config = Figment::from(Toml::string(toml))
                .extract()
                .unwrap_or_else(|e| panic!("{e}\n{toml}"));
            validate(&cfg)
        };
        let oidc = "public_url = \"https://homie.example\"\n[auth]\nissuer = \"https://idp.example\"\nclient_id = \"homie\"\n";
        let grafana = "[grafana.g]\nurl = \"https://grafana.example\"\ntoken = \"t\"\n";
        let kube = "[kubernetes.k]\nauth = \"kubeconfig\"\n";
        for (toml, error) in [
            ("listen = \"nowhere\"".to_string(), "listen: \"nowhere\" is not ip:port"),
            ("refresh_seconds = 1".into(), "refresh_seconds must be 5 or more"),
            (
                "allow_unauthenticated = true\n[local_login]\npassword_hash = \"x\"\nrole = \"nope\"\n".into(),
                "local_login.role: no [roles.nope]",
            ),
            (
                format!("{oidc}allowed_emails = [\"a@example.com\"]\nroles = {{ \"a@example.com\" = \"nope\" }}\n"),
                "auth.roles: no [roles.nope]",
            ),
            (
                "[quiet_hours]\nwindow = \"soon\"\n".into(),
                "quiet_hours.window",
            ),
            (
                "public_url = \"homie\"\n[local_login]\npassword_hash = \"x\"\n".into(),
                "public_url: \"homie\" is not a URL",
            ),
            (
                "public_url = \"ftp://homie.example\"\n[local_login]\npassword_hash = \"x\"\n".into(),
                "must be http:// or https://",
            ),
            (
                "public_url = \"http://localhost\"\n[local_login]\nusername = \" \"\npassword_hash = \"x\"\n".into(),
                "local_login.username is empty",
            ),
            (
                format!("{oidc}allowed_groups = [\"admins\"]\n"),
                "auth.allowed_groups needs auth.groups_claim",
            ),
            (
                "[proxmox.p]\nendpoints = []\ntoken = \"t\"\n".into(),
                "proxmox.p.endpoints is empty",
            ),
            (
                "[proxmox.p]\nendpoints = [\"pve:8006\"]\ntoken = \"t\"\n".into(),
                "proxmox.p.endpoints",
            ),
            (
                "[proxmox.p]\nendpoints = [\"https://pve\"]\ntoken = \"t\"\nui_url = \"x\"\n".into(),
                "proxmox.p.ui_url",
            ),
            (
                format!("{kube}argocd_url = \"x\"\n"),
                "kubernetes.k.argocd_url",
            ),
            (
                format!("{kube}metrics = {{ grafana = \"g\", datasource = \"d\", cpu = \"c\", memory = \"m\" }}\n"),
                "kubernetes.k.metrics.grafana: no [grafana.g]",
            ),
            (
                format!("{kube}links = {{ pod = \"ftp://x\" }}\n"),
                "kubernetes.k.links.pod must start with http",
            ),
            (
                "[kubernetes.k]\nauth = \"forward_token\"\n".into(),
                "auth = \"forward_token\" needs [auth]",
            ),
            ("[openbao.b]\nurl = \"x\"\n".into(), "openbao.b.url"),
            ("[grafana.g]\nurl = \"x\"\ntoken = \"t\"\n".into(), "grafana.g.url"),
            (
                format!("{grafana}ui_url = \"x\"\n"),
                "grafana.g.ui_url",
            ),
            (
                "[security.kyverno]\nkubernetes = \"k\"\n".into(),
                "security.kyverno.kubernetes: no [kubernetes.k]",
            ),
            (
                "[security.wazuh]\ngrafana = \"g\"\ndatasource = \"d\"\n".into(),
                "security.wazuh.grafana: no [grafana.g]",
            ),
            (
                "[[logs.datasources]]\nlabel = \"L\"\ngrafana = \"g\"\ndatasource = \"d\"\ntype = \"loki\"\n".into(),
                "logs.datasources \"L\": no [grafana.g]",
            ),
            (
                format!("{grafana}[[logs.datasources]]\nlabel = \"L\"\ngrafana = \"g\"\ndatasource = \"d\"\ntype = \"loki\"\n\
                         [[logs.datasources]]\nlabel = \"L\"\ngrafana = \"g\"\ndatasource = \"e\"\ntype = \"victorialogs\"\n"),
                "label \"L\" used twice",
            ),
            (
                "[logs]\ntail_seconds = 1\n".into(),
                "logs.tail_seconds must be 2 to 60",
            ),
            (
                "[tailscale]\noauth_client_id = \"id\"\n".into(),
                "tailscale: set api_key",
            ),
            ("[github]\napp_id = 1\n".into(), "github: set app_id and private_key, or token"),
            (
                "[github]\ntoken = \"t\"\napi_url = \"x\"\n".into(),
                "github.api_url",
            ),
            (
                "[github]\ntoken = \"t\"\nwatch = [{ repo = \"nope\", workflow = \"ci.yml\" }]\n".into(),
                "github.watch: \"nope\" must be owner/name",
            ),
            (
                "[github]\ntoken = \"t\"\nwatch = [{ repo = \"a/b\", workflow = \"../x\" }]\n".into(),
                "must be a file name or id",
            ),
            (
                "[healthchecks]\nurl = \"x\"\napi_key = \"k\"\n".into(),
                "healthchecks.url",
            ),
        ] {
            let e = check(&toml).expect_err(&toml).to_string();
            assert!(e.contains(error), "{e:?} lacks {error:?}");
        }
        // and a config using all of them passes
        check(&format!(
            "{oidc}allowed_emails = [\"a@example.com\"]\n{grafana}ui_url = \"https://grafana.example\"\n\
             [openbao.b]\nurl = \"https://bao.example\"\n\
             [healthchecks]\napi_key = \"k\"\n\
             [tailscale]\napi_key = \"k\"\n\
             [github]\ntoken = \"t\"\nwatch = [{{ repo = \"a/b\", workflow = \"ci.yml\" }}]\n\
             [quiet_hours]\nwindow = \"23:00-06:00\"\n"
        ))
        .unwrap();
    }

    #[test]
    fn missing_config_file_and_home_expansion() {
        let e = load(Some("/nonexistent/homie.toml".into())).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
        assert_eq!(expand(Path::new("/etc/x")), PathBuf::from("/etc/x"));
        Jail::expect_with(|jail| {
            let home = jail.directory().to_path_buf();
            jail.set_env("HOME", home.display());
            jail.create_dir(".config/homie")?;
            jail.create_file(
                ".config/homie/config.toml",
                "refresh_seconds = 42\nstate_dir = \"~/state\"",
            )?;
            let cfg = load(None).unwrap();
            assert_eq!(cfg.refresh_seconds, 42, "the default location is read");
            assert_eq!(cfg.state_dir, Some(home.join("state")));
            jail.set_env("HOMIE_CONFIG", "~/.config/homie/config.toml");
            assert_eq!(load(None).unwrap().refresh_seconds, 42);
            Ok(())
        });
    }
}

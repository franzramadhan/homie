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
    pub allow_unauthenticated: bool,
    pub quiet_hours: Option<QuietHours>,
    pub auth: Option<Auth>,
    pub proxmox: BTreeMap<String, Proxmox>,
    pub kubernetes: BTreeMap<String, Kubernetes>,
    pub grafana: BTreeMap<String, Grafana>,
    pub security: Security,
    pub tailscale: Option<Tailscale>,
    pub healthchecks: Option<Healthchecks>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7777".into(),
            public_url: String::new(),
            state_dir: None,
            refresh_seconds: 15,
            allow_unauthenticated: false,
            quiet_hours: None,
            auth: None,
            proxmox: BTreeMap::new(),
            kubernetes: BTreeMap::new(),
            grafana: BTreeMap::new(),
            security: Security::default(),
            tailscale: None,
            healthchecks: None,
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
    #[serde(default = "default_headlamp_cluster")]
    pub headlamp_cluster: String,
    #[serde(default)]
    pub quiet: bool,
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
    pub token: Secret,
    #[serde(default = "yes")]
    pub tls_verify: bool,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub tls_server_name: Option<String>,
    #[serde(default)]
    pub quiet: bool,
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
    if let Some(q) = &cfg.quiet_hours {
        crate::quiet::Quiet::new(&q.window, q.timezone.as_deref())?;
    }

    match &cfg.auth {
        None if !listen.ip().is_loopback() && !cfg.allow_unauthenticated => {
            return Err(format!(
                "listen {listen} is not loopback and [auth] is not configured: anyone who can reach it \
                 could use your tokens. Configure [auth], listen on 127.0.0.1, or set \
                 allow_unauthenticated = true behind an auth proxy or a loopback-only port mapping"
            )
            .into());
        }
        None => {}
        Some(a) => {
            let u = check_url("public_url", &cfg.public_url)?;
            let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            if u.scheme() != "https" && !local {
                return Err("public_url must be https:// when [auth] is configured".into());
            }
            check_url("auth.issuer", &a.issuer)?;
            if a.allowed_emails.is_empty() && a.allowed_groups.is_empty() {
                return Err(
                    "auth: set allowed_emails or allowed_groups, or nobody can log in".into(),
                );
            }
            if !a.allowed_groups.is_empty() && a.groups_claim.is_none() {
                return Err("auth.allowed_groups needs auth.groups_claim".into());
            }
        }
    }

    for (name, p) in &cfg.proxmox {
        if p.endpoints.is_empty() {
            return Err(format!("proxmox.{name}.endpoints is empty").into());
        }
        for (url, _) in p.targets() {
            check_url(&format!("proxmox.{name}.endpoints"), &url)?;
        }
    }
    for (name, k) in &cfg.kubernetes {
        if k.auth_mode() == KubeAuth::ForwardToken && cfg.auth.is_none() {
            return Err(format!("kubernetes.{name}: auth = \"forward_token\" needs [auth]").into());
        }
    }
    for (name, g) in &cfg.grafana {
        check_url(&format!("grafana.{name}.url"), &g.url)?;
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
    if let Some(t) = &cfg.tailscale {
        let oauth = t.oauth_client_id.is_some() && t.oauth_client_secret.is_some();
        if t.api_key.is_none() && !oauth {
            return Err(
                "tailscale: set api_key, or oauth_client_id and oauth_client_secret".into(),
            );
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
    }
}

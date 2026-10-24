//! Every configured integration, built once at start, and the named queries
//! the API serves from them.

pub mod argocd;
pub mod certs;
pub mod flux;
pub mod github;
pub mod grafana;
pub mod helm;
pub mod kubernetes;
pub mod links;
pub mod logs;
pub mod outage;
pub mod proxmox;
pub mod security;
pub mod services;
pub mod sparks;
pub mod updates;
pub mod watch_store;

use std::{collections::BTreeMap, time::Duration};

use serde_json::Value;

use crate::{
    Result,
    cache::{Cache, Envelope},
    config::{Config, Security},
    quiet::Quiet,
};

pub struct Lab {
    pub proxmox: BTreeMap<String, proxmox::Proxmox>,
    pub kubernetes: BTreeMap<String, kubernetes::Kube>,
    pub grafana: BTreeMap<String, grafana::Grafana>,
    pub openbao: BTreeMap<String, services::OpenBao>,
    pub security: Security,
    pub logs: crate::config::Logs,
    pub tailscale: Option<outage::Tailscale>,
    pub github: Option<github::GitHub>,
    pub healthchecks: Option<outage::Healthchecks>,
    pub quiet: Option<Quiet>,
    pub roles: BTreeMap<String, std::sync::Arc<crate::config::Role>>,
    pub certificates: crate::config::Certificates,
    pub updates: crate::config::Updates,
}

/// The caller of a query: who they are and, in forward_token mode, the ID
/// token to act with.
#[derive(Clone, Default)]
pub struct Caller {
    pub email: Option<String>,
    pub id_token: Option<String>,
    /// a read-only role (RFC 0016); `None` is full access
    pub role: Option<RoleRef>,
    /// the Homie session id, for per-session state (RFC 0017)
    pub sid: Option<String>,
    /// a ServiceAccount to impersonate for Kubernetes reads without a user
    /// (the notifier, RFC 0019)
    pub service_account: Option<String>,
}

#[derive(Clone)]
pub struct RoleRef {
    pub name: String,
    pub role: std::sync::Arc<crate::config::Role>,
}

impl Lab {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let mut lab = Lab {
            proxmox: BTreeMap::new(),
            kubernetes: BTreeMap::new(),
            grafana: BTreeMap::new(),
            openbao: BTreeMap::new(),
            security: cfg.security.clone(),
            logs: cfg.logs.clone(),
            tailscale: None,
            github: None,
            healthchecks: None,
            quiet: cfg
                .quiet_hours
                .as_ref()
                .map(|q| Quiet::new(&q.window, q.timezone.as_deref()))
                .transpose()?,
            roles: cfg
                .roles
                .iter()
                .map(|(k, v)| (k.clone(), std::sync::Arc::new(v.clone())))
                .collect(),
            certificates: cfg.certificates.clone(),
            updates: cfg.updates.clone(),
        };
        for (name, c) in &cfg.proxmox {
            lab.proxmox
                .insert(name.clone(), proxmox::Proxmox::new(name, c).await?);
        }
        for (name, c) in &cfg.kubernetes {
            lab.kubernetes
                .insert(name.clone(), kubernetes::Kube::new(name, c).await?);
        }
        for (name, c) in &cfg.grafana {
            lab.grafana
                .insert(name.clone(), grafana::Grafana::new(name, c).await?);
        }
        for (name, c) in &cfg.openbao {
            lab.openbao
                .insert(name.clone(), services::OpenBao::new(name, c).await?);
        }
        if let Some(t) = &cfg.tailscale {
            lab.tailscale = Some(
                outage::Tailscale::new(t)
                    .await
                    .map_err(|e| format!("tailscale: {e}"))?,
            );
        }
        if let Some(g) = &cfg.github {
            lab.github = Some(github::GitHub::new(g, cfg.state_dir.as_deref()).await?);
        }
        if let Some(h) = &cfg.healthchecks {
            lab.healthchecks = Some(outage::Healthchecks::new(h).await?);
        }
        for (kind, name, reason) in lab.insecure() {
            tracing::warn!(
                msg = "insecure",
                integration = format!("{kind}.{name}"),
                reason
            );
        }
        Ok(lab)
    }

    pub fn insecure(&self) -> Vec<(&'static str, &str, &'static str)> {
        let p = self
            .proxmox
            .values()
            .filter_map(|x| Some(("proxmox", x.name.as_str(), x.insecure?)));
        let g = self
            .grafana
            .values()
            .filter_map(|x| Some(("grafana", x.name.as_str(), x.insecure?)));
        p.chain(g).collect()
    }

    /// Clusters that discovery or cert-manager read for certificates.
    fn cert_clusters(&self) -> impl Iterator<Item = &kubernetes::Kube> {
        self.kubernetes
            .values()
            .filter(|k| k.certificates.discover || k.cert_manager)
    }

    /// Anything to show on the certificates card (RFC 0018).
    pub fn certificates_on(&self) -> bool {
        let c = &self.certificates;
        c.endpoints || !c.targets.is_empty() || self.cert_clusters().next().is_some()
    }

    /// Anything to show on the updates card (RFC 0020).
    pub fn updates_on(&self) -> bool {
        (self.updates.proxmox && !self.proxmox.is_empty()) || !self.kubernetes.is_empty()
    }

    pub fn quiet_active(&self) -> bool {
        self.quiet.as_ref().is_some_and(Quiet::active)
    }

    fn grafana(&self, name: &str) -> Result<&grafana::Grafana> {
        self.grafana
            .get(name)
            .ok_or_else(|| format!("no grafana {name:?}").into())
    }

    fn kube(&self, name: &str) -> Result<&kubernetes::Kube> {
        self.kubernetes
            .get(name)
            .ok_or_else(|| format!("no kubernetes {name:?}").into())
    }

    /// Serves the named query `kind/name` through the cache. Results fetched
    /// with a user's token are cached per user.
    pub async fn query(
        &self,
        cache: &Cache,
        kind: &str,
        name: &str,
        caller: &Caller,
    ) -> Option<Envelope> {
        // RFC 0017: each session's own workflows, read with its own token;
        // the data is the visitor's own, so a role's filter does not apply
        if let Some(gh) = self
            .github
            .as_ref()
            .filter(|g| kind == "github" && g.session_mode())
        {
            if caller
                .role
                .as_ref()
                .is_some_and(|r| !crate::roles::allows(&r.role, kind))
            {
                return None;
            }
            let sid = caller.sid.clone().unwrap_or_default();
            let key = format!("github:_:session:{sid}");
            let mut env = cache
                .get(&key, Duration::from_secs(60), false, || async {
                    Ok(serde_json::to_value(gh.session_workflows(&sid).await?)?)
                })
                .await;
            crate::status::apply(kind, &mut env);
            return Some(env);
        }
        if let Some(r) = &caller.role
            && !crate::roles::allows(&r.role, kind)
        {
            return None;
        }
        let quiet_now = self.quiet_active();
        let secs = |s| Duration::from_secs(s);
        let (ttl, quiet, per_user) = match kind {
            "pve" => (secs(10), self.proxmox.get(name)?.quiet, false),
            "spark-pve" => (secs(60), self.proxmox.get(name)?.quiet, false),
            "spark-k8s" => {
                let k = self.kubernetes.get(name)?;
                k.metrics.as_ref()?;
                (secs(60), k.quiet, false)
            }
            "k8s" | "flux" => {
                let k = self.kubernetes.get(name)?;
                (secs(10), k.quiet, k.forwards_token())
            }
            "helm" => {
                let k = self.kubernetes.get(name)?;
                (secs(60), k.quiet, k.forwards_token())
            }
            "alerts" => (secs(30), self.grafana.get(name)?.quiet, false),
            "openbao" => (secs(30), self.openbao.get(name)?.quiet, false),
            "cnpg" | "argocd" => {
                let k = self.kubernetes.get(name)?;
                (
                    secs(if kind == "argocd" { 10 } else { 30 }),
                    k.quiet,
                    k.forwards_token(),
                )
            }
            "security" => {
                let (quiet, per_user) = self.preset_owner(name)?;
                (secs(60), quiet, per_user)
            }
            "tailscale" => (secs(30), false, false),
            "github" => (secs(60), self.github.as_ref()?.quiet, false),
            "healthchecks" => (secs(60), self.healthchecks.as_ref()?.quiet, false),
            // probes keep their own interval; this bounds discovery reads
            // and how soon newly seen endpoints show
            "certs" if self.certificates_on() => (
                secs(60),
                false,
                self.cert_clusters().any(|k| k.forwards_token()),
            ),
            "updates" if self.updates_on() => (
                secs(300),
                false,
                self.kubernetes.values().any(|k| k.forwards_token()),
            ),
            _ => return None,
        };
        let key = match (&caller.role, per_user, &caller.email) {
            // a role reads with its own identity: its own cache entries
            (Some(r), _, _) => format!("{kind}:{name}:role:{}", r.name),
            (None, true, Some(email)) => format!("{kind}:{name}:{email}"),
            _ => format!("{kind}:{name}"),
        };
        let mut env = cache
            .get(&key, ttl, quiet && quiet_now, || {
                self.fetch(kind, name, caller, quiet_now)
            })
            .await;
        if let Some(r) = &caller.role {
            // guest sparklines follow the guests the role sees
            let pve = match kind {
                "spark-pve" => Box::pin(self.query(cache, "pve", name, caller))
                    .await
                    .and_then(|e| e.data),
                _ => None,
            };
            env.data = env
                .data
                .map(|d| crate::roles::filter(&r.role, kind, d, pve.as_ref()));
            // errors name the endpoint that failed, often by address
            env.error = env.error.map(|e| crate::roles::redact_ips(&e));
        }
        // after the role filter: the tone is of what the caller sees
        crate::status::apply(kind, &mut env);
        Some(env)
    }

    /// The Kubernetes client for a caller: a role's impersonated
    /// ServiceAccount, else the shared client or the user's token.
    fn kube_client(&self, k: &kubernetes::Kube, caller: &Caller) -> Result<kube::Client> {
        match &caller.role {
            Some(r) => k.impersonating(
                r.role
                    .kubernetes_service_account
                    .as_deref()
                    .ok_or("this role has no kubernetes_service_account")?,
            ),
            None => match &caller.service_account {
                Some(sa) => k.impersonating(sa),
                None => k.client(caller.id_token.as_deref()),
            },
        }
    }

    /// The quiet flag and per-user caching of the integration behind a preset.
    fn preset_owner(&self, preset: &str) -> Option<(bool, bool)> {
        let s = &self.security;
        let g = |name: &str| self.grafana.get(name).map(|g| (g.quiet, false));
        match preset {
            "kyverno" => self
                .kubernetes
                .get(&s.kyverno.as_ref()?.kubernetes)
                .map(|k| (k.quiet, k.forwards_token())),
            "tetragon" => g(&s.tetragon.as_ref()?.grafana),
            "wazuh" => g(&s.wazuh.as_ref()?.grafana),
            "debsecan" => g(&s.debsecan.as_ref()?.grafana),
            _ => None,
        }
    }

    async fn fetch(
        &self,
        kind: &str,
        name: &str,
        caller: &Caller,
        quiet_now: bool,
    ) -> Result<Value> {
        let token = caller.id_token.as_deref();
        let v = match kind {
            "pve" => serde_json::to_value(self.proxmox[name].overview().await?)?,
            "spark-pve" => serde_json::to_value(sparks::proxmox(&self.proxmox[name]).await?)?,
            "spark-k8s" => {
                let m = self
                    .kube(name)?
                    .metrics
                    .as_ref()
                    .ok_or("no metrics configured")?;
                serde_json::to_value(sparks::kubernetes(self.grafana(&m.grafana)?, m).await?)?
            }
            "k8s" => {
                let k = self.kube(name)?;
                let namespaces = caller.role.as_ref().map(|r| r.role.namespaces.as_slice());
                serde_json::to_value(k.overview(self.kube_client(k, caller)?, namespaces).await?)?
            }
            "helm" => serde_json::to_value(helm::releases(self.kube(name)?.client(token)?).await?)?,
            "flux" => {
                let k = self.kube(name)?;
                serde_json::to_value(
                    flux::list_linked(
                        self.kube_client(k, caller)?,
                        &k.links,
                        caller.role.is_some(),
                    )
                    .await?,
                )?
            }
            "alerts" => serde_json::to_value(self.grafana(name)?.alerts().await?)?,
            "openbao" => {
                let b = self.openbao.get(name).ok_or("no such openbao")?;
                serde_json::to_value(b.health().await?)?
            }
            "argocd" => {
                let k = self.kube(name)?;
                serde_json::to_value(
                    argocd::list(self.kube_client(k, caller)?, k.argocd_url.as_deref()).await?,
                )?
            }
            "cnpg" => {
                let k = self.kube(name)?;
                serde_json::to_value(
                    services::cnpg(self.kube_client(k, caller)?, k.cnpg_max_backup_age_hours)
                        .await?,
                )?
            }
            "security" => {
                let s = &self.security;
                let table = match name {
                    "kyverno" => {
                        let p = s.kyverno.as_ref().ok_or("kyverno is not configured")?;
                        let k = self.kube(&p.kubernetes)?;
                        security::kyverno(self.kube_client(k, caller)?).await?
                    }
                    "tetragon" => {
                        let p = s.tetragon.as_ref().ok_or("tetragon is not configured")?;
                        security::tetragon(self.grafana(&p.grafana)?, p).await?
                    }
                    "wazuh" => {
                        let p = s.wazuh.as_ref().ok_or("wazuh is not configured")?;
                        security::wazuh(self.grafana(&p.grafana)?, p).await?
                    }
                    "debsecan" => {
                        let p = s.debsecan.as_ref().ok_or("debsecan is not configured")?;
                        security::debsecan(self.grafana(&p.grafana)?, p).await?
                    }
                    _ => return Err(format!("unknown preset {name:?}").into()),
                };
                serde_json::to_value(table)?
            }
            "github" => serde_json::to_value(
                self.github
                    .as_ref()
                    .ok_or("github is not configured")?
                    .workflows()
                    .await?,
            )?,
            "tailscale" => serde_json::to_value(
                self.tailscale
                    .as_ref()
                    .ok_or("tailscale is not configured")?
                    .devices(quiet_now)
                    .await?,
            )?,
            "healthchecks" => serde_json::to_value(
                self.healthchecks
                    .as_ref()
                    .ok_or("healthchecks is not configured")?
                    .checks()
                    .await?,
            )?,
            "certs" => {
                let clusters = self
                    .cert_clusters()
                    .map(|k| (k, self.kube_client(k, caller)))
                    .collect();
                serde_json::to_value(certs::collect(&self.certificates, clusters).await)?
            }
            "updates" => {
                let u = &self.updates;
                let mut r = updates::Report::default();
                let gh = match &u.github_token {
                    Some(s) => Some(s.resolve().await?),
                    None => None,
                };
                if u.proxmox {
                    for p in self.proxmox.values() {
                        match updates::proxmox(p).await {
                            Ok(n) => r.proxmox.extend(n),
                            Err(e) => r.errors.push(format!("{}: {e}", p.name)),
                        }
                    }
                }
                for k in self.kubernetes.values() {
                    let client = match self.kube_client(k, caller) {
                        Ok(c) => c,
                        Err(e) => {
                            r.errors.push(format!("{}: {e}", k.name));
                            continue;
                        }
                    };
                    match updates::cluster(k, client.clone(), u, gh.as_deref(), &mut r.errors).await
                    {
                        Ok(c) => r.kubernetes.push(c),
                        Err(e) => r.errors.push(format!("{}: {e}", k.name)),
                    }
                    match updates::charts(&k.name, client, u).await {
                        Ok(c) => r.charts.extend(c),
                        Err(e) => r.errors.push(format!("{}: charts: {e}", k.name)),
                    }
                }
                if u.upstream.charts {
                    let mut creds = std::collections::BTreeMap::new();
                    for reg in &u.registries {
                        creds.insert(
                            reg.host.clone(),
                            (reg.username.resolve().await?, reg.password.resolve().await?),
                        );
                    }
                    updates::chart_latest(&mut r.charts, &creds, &mut r.errors).await;
                }
                serde_json::to_value(r)?
            }
            _ => return Err(format!("unknown query {kind:?}").into()),
        };
        Ok(v)
    }

    /// Configured security presets, in display order.
    pub fn presets(&self) -> Vec<&'static str> {
        let s = &self.security;
        [
            ("kyverno", s.kyverno.is_some()),
            ("tetragon", s.tetragon.is_some()),
            ("wazuh", s.wazuh.is_some()),
            ("debsecan", s.debsecan.is_some()),
        ]
        .into_iter()
        .filter_map(|(n, on)| on.then_some(n))
        .collect()
    }
}

impl Lab {
    /// Runs a log search against a configured source (RFC 0008).
    pub async fn search_logs(&self, req: &logs::SearchRequest) -> Result<impl serde::Serialize> {
        let src = self
            .logs
            .datasources
            .iter()
            .find(|s| s.label == req.source)
            .ok_or_else(|| format!("no log source {:?}", req.source))?;
        let limit = logs::validate(req, self.logs.max_rows)?;
        logs::search(self.grafana(&src.grafana)?, src, req, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::harness::{backend, config, kubeconfig};

    fn role(pages: &[&str], service_account: Option<&str>) -> Caller {
        Caller {
            email: Some("viewer@example.com".into()),
            id_token: None,
            sid: None,
            service_account: None,
            role: Some(RoleRef {
                name: "viewer".into(),
                role: std::sync::Arc::new(crate::config::Role {
                    pages: pages.iter().map(|p| p.to_string()).collect(),
                    namespaces: vec!["apps".into()],
                    proxmox_tags: vec!["demo".into()],
                    kubernetes_service_account: service_account.map(String::from),
                }),
            }),
        }
    }

    #[tokio::test]
    async fn queries_per_caller() {
        let backend = backend().await;
        let kc = kubeconfig(&backend);
        let lab = Lab::new(&config(&format!(
            "[kubernetes.main]\nauth = \"kubeconfig\"\nkubeconfig = \"{}\"\n\
             [proxmox.lab]\nendpoints = [\"{backend}\"]\ntoken = \"t\"\n",
            kc.display()
        )))
        .await
        .unwrap();
        let cache = Cache::new(None).unwrap();
        let anyone = Caller::default();

        assert!(
            lab.query(&cache, "spark-k8s", "main", &anyone)
                .await
                .is_none(),
            "no metrics configured"
        );
        assert!(lab.query(&cache, "alerts", "main", &anyone).await.is_none());

        let no_sa = role(&["workloads"], None);
        let env = lab.query(&cache, "k8s", "main", &no_sa).await.unwrap();
        assert_eq!(
            env.error.as_deref(),
            Some("this role has no kubernetes_service_account")
        );
        assert!(
            lab.query(&cache, "pve", "lab", &no_sa).await.is_none(),
            "infra is not among the role's pages"
        );

        // a role's sparklines follow the guests it sees
        let infra = role(&["infra"], None);
        let sparks = lab.query(&cache, "spark-pve", "lab", &infra).await.unwrap();
        assert!(sparks.error.is_none(), "{:?}", sparks.error);
        let pve = lab.query(&cache, "pve", "lab", &infra).await.unwrap();
        assert_eq!(pve.data.unwrap()["guests"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn fetch_errors_name_what_is_missing() {
        let lab = Lab::new(&config(
            "[quiet_hours]\nwindow = \"00:00-23:59\"\ntimezone = \"UTC\"\n\
             [tailscale]\napi_key = \"tskey-x\"\n",
        ))
        .await
        .unwrap();
        assert!(lab.tailscale.is_some());
        assert_eq!(
            lab.quiet_active(),
            lab.quiet.as_ref().unwrap().active(),
            "quiet hours come from the config"
        );
        assert!(lab.presets().is_empty());
        assert!(lab.insecure().is_empty());
        let anyone = Caller::default();
        for (kind, name, error) in [
            ("bogus", "x", "unknown query \"bogus\""),
            ("security", "none", "unknown preset \"none\""),
            ("security", "kyverno", "kyverno is not configured"),
            ("security", "tetragon", "tetragon is not configured"),
            ("security", "wazuh", "wazuh is not configured"),
            ("security", "debsecan", "debsecan is not configured"),
            ("github", "main", "github is not configured"),
            ("healthchecks", "main", "healthchecks is not configured"),
            ("alerts", "none", "no grafana \"none\""),
            ("k8s", "none", "no kubernetes \"none\""),
            ("openbao", "none", "no such openbao"),
        ] {
            let e = lab.fetch(kind, name, &anyone, false).await.unwrap_err();
            assert_eq!(e.to_string(), error, "{kind}/{name}");
        }
        assert!(lab.preset_owner("none").is_none());
        let req = logs::SearchRequest {
            source: "none".into(),
            query: "x".into(),
            minutes: 5,
            limit: None,
        };
        assert!(lab.search_logs(&req).await.is_err());
    }

    #[tokio::test]
    async fn construction_errors_are_reported() {
        let cfg = |toml: &str| -> Config {
            figment::Figment::from(
                <figment::providers::Toml as figment::providers::Format>::string(toml),
            )
            .extract()
            .unwrap()
        };
        for (toml, error) in [
            (
                "[proxmox.lab]\nendpoints = [\"https://pve.example.com\"]\ntoken = { env = \"HOMIE_TEST_UNSET\" }\n",
                "proxmox.lab.token",
            ),
            (
                "[kubernetes.main]\nauth = \"kubeconfig\"\nkubeconfig = \"/nonexistent/kubeconfig\"\n",
                "kubernetes.main",
            ),
            (
                "[tailscale]\napi_key = { env = \"HOMIE_TEST_UNSET\" }\n",
                "tailscale",
            ),
            ("[quiet_hours]\nwindow = \"later\"\n", "quiet_hours.window"),
        ] {
            let e = Lab::new(&cfg(toml)).await.err().unwrap().to_string();
            assert!(e.contains(error), "{e}");
        }
    }
}

//! Every configured integration, built once at start, and the named queries
//! the API serves from them.

pub mod argocd;
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
}

/// The caller of a query: who they are and, in forward_token mode, the ID
/// token to act with.
#[derive(Clone, Default)]
pub struct Caller {
    pub email: Option<String>,
    pub id_token: Option<String>,
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
            _ => return None,
        };
        let key = match (per_user, &caller.email) {
            (true, Some(email)) => format!("{kind}:{name}:{email}"),
            _ => format!("{kind}:{name}"),
        };
        Some(
            cache
                .get(&key, ttl, quiet && quiet_now, || {
                    self.fetch(kind, name, caller, quiet_now)
                })
                .await,
        )
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
                serde_json::to_value(k.overview(k.client(token)?).await?)?
            }
            "helm" => serde_json::to_value(helm::releases(self.kube(name)?.client(token)?).await?)?,
            "flux" => {
                let k = self.kube(name)?;
                serde_json::to_value(flux::list_linked(k.client(token)?, &k.links).await?)?
            }
            "alerts" => serde_json::to_value(self.grafana(name)?.alerts().await?)?,
            "openbao" => {
                let b = self.openbao.get(name).ok_or("no such openbao")?;
                serde_json::to_value(b.health().await?)?
            }
            "argocd" => {
                let k = self.kube(name)?;
                serde_json::to_value(
                    argocd::list(k.client(token)?, k.argocd_url.as_deref()).await?,
                )?
            }
            "cnpg" => {
                let k = self.kube(name)?;
                serde_json::to_value(
                    services::cnpg(k.client(token)?, k.cnpg_max_backup_age_hours).await?,
                )?
            }
            "security" => {
                let s = &self.security;
                let table = match name {
                    "kyverno" => {
                        let p = s.kyverno.as_ref().ok_or("kyverno is not configured")?;
                        security::kyverno(self.kube(&p.kubernetes)?.client(token)?).await?
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

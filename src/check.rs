//! `homie check`: print the effective config with secrets redacted, then test
//! every integration and print one line each.

use std::time::Duration;

use futures::future::join_all;

use crate::{Result, auth::Auth, config::Config, integrations::Lab};

type Probe<'a> = std::pin::Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

pub async fn run(cfg: Config) -> Result<()> {
    println!("{}\n", serde_json::to_string_pretty(&cfg)?);

    let lab = Lab::new(&cfg).await?;
    let auth = Auth::new(&cfg).await;

    let cache = crate::cache::Cache::new(None)?;
    let anonymous = crate::integrations::Caller::default();
    let mut probes: Vec<(String, Probe)> = Vec::new();
    match &auth {
        Ok(Some(a)) => {
            let detail = a.probe();
            probes.push(("auth".into(), Box::pin(async move { Ok(detail) })));
        }
        Err(e) => {
            let e = e.to_string();
            probes.push(("auth".into(), Box::pin(async move { Err(e.into()) })));
        }
        Ok(None) => {}
    }
    for (n, p) in &lab.proxmox {
        probes.push((format!("proxmox.{n}"), Box::pin(p.probe())));
    }
    for (n, k) in &lab.kubernetes {
        probes.push((format!("kubernetes.{n}"), Box::pin(k.probe())));
    }
    for (n, g) in &lab.grafana {
        probes.push((format!("grafana.{n}"), Box::pin(g.probe())));
    }
    for preset in lab.presets() {
        let (lab, cache, anonymous) = (&lab, &cache, &anonymous);
        probes.push((
            format!("security.{preset}"),
            Box::pin(async move {
                let env = lab
                    .query(cache, "security", preset, anonymous)
                    .await
                    .ok_or("not configured")?;
                match (env.error, env.data) {
                    (Some(e), _) => Err(e.into()),
                    (None, Some(d)) => Ok(format!(
                        "{} rows",
                        d["rows"].as_array().map_or(0, |r| r.len())
                    )),
                    (None, None) => Err("no data".into()),
                }
            }),
        ));
    }
    if let Some(t) = &lab.tailscale {
        probes.push((
            "tailscale".into(),
            Box::pin(async move {
                let d = t.devices(false).await?;
                let watched = d.iter().filter(|x| x.is_watched()).count();
                Ok(format!("{} devices, {watched} watched (hosts)", d.len()))
            }),
        ));
    }
    if let Some(h) = &lab.healthchecks {
        probes.push((
            "healthchecks".into(),
            Box::pin(async move { Ok(format!("{} checks", h.checks().await?.len())) }),
        ));
    }

    let names: Vec<String> = probes.iter().map(|(n, _)| n.clone()).collect();
    let results = join_all(probes.into_iter().map(|(_, p)| async move {
        tokio::time::timeout(Duration::from_secs(20), p)
            .await
            .unwrap_or_else(|_| Err("timed out after 20 s".into()))
    }))
    .await;

    let width = names.iter().map(String::len).max().unwrap_or(0);
    let mut failed = 0;
    for (name, r) in names.iter().zip(results) {
        match r {
            Ok(detail) => println!("ok    {name:width$}  {detail}"),
            Err(e) => {
                failed += 1;
                println!("FAIL  {name:width$}  {e}");
            }
        }
    }
    for (kind, name, reason) in lab.insecure() {
        println!("warn  {:width$}  {reason}", format!("{kind}.{name}"));
    }
    if names.is_empty() {
        println!("nothing configured: add a [proxmox.*], [kubernetes.*] or [grafana.*] section");
    }
    if failed > 0 {
        return Err(format!("{failed} of {} checks failed", names.len()).into());
    }
    Ok(())
}

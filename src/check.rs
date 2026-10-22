//! `homie check`: print the effective config with secrets redacted, then test
//! every integration and print one line each.

use std::time::Duration;

use futures::future::join_all;

use crate::{Result, auth::Auth, config::Config, integrations::Lab};

type Probe<'a> = std::pin::Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

pub async fn run(cfg: Config) -> Result<()> {
    report(cfg, &mut std::io::stdout()).await
}

/// `run`, printing to `out`.
async fn report(cfg: Config, out: &mut impl std::io::Write) -> Result<()> {
    writeln!(out, "{}\n", serde_json::to_string_pretty(&cfg)?)?;

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
    for (n, b) in &lab.openbao {
        probes.push((
            format!("openbao.{n}"),
            Box::pin(async move {
                let h = serde_json::to_value(b.health().await?)?;
                Ok(format!(
                    "{} {}",
                    h["state"].as_str().unwrap_or("?"),
                    h["version"].as_str().unwrap_or("")
                ))
            }),
        ));
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
    if let Some(g) = &lab.github {
        probes.push((
            "github".into(),
            Box::pin(async move {
                let flows = g.workflows().await?;
                let broken: Vec<String> = flows
                    .iter()
                    .filter_map(|w| Some(format!("{}: {}", w.label(), w.err()?)))
                    .collect();
                if broken.is_empty() {
                    Ok(format!("{} watched workflows readable", flows.len()))
                } else {
                    Err(broken.join("; ").into())
                }
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
            Ok(detail) => writeln!(out, "ok    {name:width$}  {detail}")?,
            Err(e) => {
                failed += 1;
                writeln!(out, "FAIL  {name:width$}  {e}")?;
            }
        }
    }
    for (kind, name, reason) in lab.insecure() {
        writeln!(out, "warn  {:width$}  {reason}", format!("{kind}.{name}"))?;
    }
    if names.is_empty() {
        writeln!(
            out,
            "nothing configured: add a [proxmox.*], [kubernetes.*] or [grafana.*] section"
        )?;
    }
    if failed > 0 {
        return Err(format!("{failed} of {} checks failed", names.len()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::harness::{backend, config, lab_sections, password_hash};

    async fn check(toml: &str) -> (String, Result<()>) {
        let mut out = Vec::new();
        let r = report(config(toml), &mut out).await;
        (String::from_utf8(out).unwrap(), r)
    }

    fn line<'a>(out: &'a str, start: &str) -> &'a str {
        out.lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no line {start:?} in\n{out}"))
    }

    #[tokio::test]
    async fn every_integration_answers() {
        let backend = backend().await;
        let (out, r) = check(&format!(
            "public_url = \"http://localhost:7777\"\n[local_login]\npassword_hash = \"{}\"\n\
             [github]\ntoken = \"ghp_x\"\napi_url = \"{backend}\"\nstore = \"off\"\n\
             watch = [{{ repo = \"acme/app\", workflow = \"ci.yml\" }}]\n{}",
            password_hash(),
            lab_sections(&backend)
        ))
        .await;
        assert!(r.is_ok(), "{out}");
        assert!(out.contains("<redacted>"), "secrets are redacted");
        assert!(!out.contains("glsa_x"));
        for (name, detail) in [
            ("auth", "password login for \"homie\""),
            ("proxmox.lab", "Proxmox VE 8.2.4, power actions enabled"),
            ("kubernetes.main", "Kubernetes v1.32.0, can list namespaces"),
            ("grafana.mon", "Grafana 11.2.0, token accepted"),
            ("openbao.vault", "active 2.1.0"),
            ("security.kyverno", "0 rows"),
            ("security.debsecan", "0 rows"),
            ("github", "1 watched workflows readable"),
            ("healthchecks", "0 checks"),
        ] {
            let l = line(&out, &format!("ok    {name} "));
            assert!(l.ends_with(detail), "{l}");
        }
        line(&out, "warn  proxmox.lab");
        line(&out, "warn  grafana.mon");
    }

    #[tokio::test]
    async fn failures_are_counted() {
        let backend = backend().await;
        let (out, r) = check(&format!(
            "public_url = \"http://localhost:7777\"\n\
             [auth]\nissuer = \"http://127.0.0.1:9\"\nclient_id = \"homie\"\nallowed_emails = [\"me@example.com\"]\n\
             [github]\ntoken = \"ghp_x\"\napi_url = \"{backend}\"\nstore = \"off\"\n\
             watch = [{{ repo = \"acme/app\", workflow = \"ci.yml\" }}, {{ repo = \"acme/app\", workflow = \"gone.yml\" }}]\n\
             [openbao.vault]\nurl = \"{backend}\"\n\
             [grafana.gone]\nurl = \"{backend}/missing\"\ntoken = \"t\"\n\
             [security.tetragon]\ngrafana = \"gone\"\ndatasource = \"vlogs\"\n"
        ))
        .await;
        assert_eq!(r.unwrap_err().to_string(), "4 of 5 checks failed");
        assert!(line(&out, "FAIL  grafana.gone").contains("404"), "{out}");
        assert!(
            line(&out, "FAIL  security.tetragon").contains("404"),
            "{out}"
        );
        assert!(line(&out, "FAIL  auth").contains("OIDC discovery"), "{out}");
        assert!(
            line(&out, "FAIL  github").contains("acme/app/gone.yml: HTTP 404"),
            "{out}"
        );
        line(&out, "ok    openbao.vault");
    }

    #[tokio::test]
    async fn nothing_configured() {
        let (out, r) = check("").await;
        assert!(r.is_ok());
        assert!(out.contains("nothing configured"), "{out}");
    }
}

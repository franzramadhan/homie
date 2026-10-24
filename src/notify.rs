//! Notifications (RFC 0019): a background loop runs the overview queries,
//! and a message goes out when a card's status changes and holds.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;

use crate::{
    Result,
    config::{Notify, NotifyKind, NotifyTarget},
    http,
    integrations::Caller,
    server::Shared,
    status::Tone,
};

/// One card's progress: the status last notified (or the silent baseline),
/// a new status waiting out the hold time, and whether quiet hours hide it.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub notified: Tone,
    pending: Option<(Tone, Instant)>,
    quiet: bool,
}

impl State {
    pub fn new(notified: Tone) -> Self {
        Self {
            notified,
            pending: None,
            quiet: false,
        }
    }
}

/// A change worth a message: from, to.
pub type Change = (Tone, Tone);

/// Feeds one round's status. The first status is a silent baseline, so is
/// the first one after quiet hours; a new status must hold for `hold`.
pub fn step(
    state: &mut Option<State>,
    tone: Tone,
    now: Instant,
    hold: Duration,
    min: Tone,
    recovery: bool,
) -> Option<Change> {
    let Some(s) = state else {
        *state = Some(if tone == Tone::Quiet {
            State {
                quiet: true,
                ..State::new(Tone::Ok)
            }
        } else {
            State::new(tone)
        });
        return None;
    };
    if tone == Tone::Quiet {
        s.quiet = true;
        s.pending = None;
        return None;
    }
    if s.quiet {
        // the lab coming back in the morning is not news
        *s = State::new(tone);
        return None;
    }
    if tone == s.notified {
        s.pending = None;
        return None;
    }
    match s.pending {
        Some((t, since)) if t == tone => {
            if now.duration_since(since) < hold {
                return None;
            }
        }
        _ => {
            s.pending = Some((tone, now));
            if !hold.is_zero() {
                return None;
            }
        }
    }
    let from = s.notified;
    *s = State::new(tone);
    let relevant = from.max(tone) >= min;
    (relevant && (tone > from || recovery)).then_some((from, tone))
}

/// The card's id in config (`cards`, `min_tone_per_card`).
pub fn card_id(kind: &str) -> &str {
    match kind {
        "certs" => "certificates",
        k => k,
    }
}

fn label(kind: &str, name: &str) -> String {
    let title = match kind {
        "pve" => "Proxmox",
        "k8s" => "Kubernetes",
        "flux" => "Flux",
        "argocd" => "Argo CD",
        "alerts" => "Alerts",
        "security" => "Security",
        "openbao" => "OpenBao",
        "cnpg" => "PostgreSQL",
        "tailscale" => return "Tailscale".into(),
        "github" => return "Workflows".into(),
        "healthchecks" => return "healthchecks".into(),
        "certs" => return "Certificates".into(),
        k => k,
    };
    format!("{title} {name}")
}

fn page(kind: &str) -> &'static str {
    match kind {
        "pve" | "certs" => "/infra",
        "k8s" => "/workloads",
        "flux" | "argocd" => "/gitops",
        "alerts" => "/observability",
        "security" => "/security",
        "tailscale" => "/tailscale",
        "github" => "/workflows",
        _ => "/",
    }
}

fn word(t: Tone) -> &'static str {
    match t {
        Tone::Ok => "ok",
        Tone::Warn => "warning",
        Tone::Bad => "failing",
        Tone::Quiet => "scheduled off",
    }
}

/// What a message says, before it is shaped for a target.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub card: String,
    pub name: String,
    pub title: String,
    pub from: Tone,
    pub to: Tone,
    pub reasons: Vec<String>,
    pub url: String,
}

pub fn message(
    kind: &str,
    name: &str,
    change: Change,
    reasons: &[String],
    public_url: &str,
    redact: bool,
) -> Message {
    let (from, to) = change;
    Message {
        card: card_id(kind).to_string(),
        name: if redact {
            String::new()
        } else {
            name.to_string()
        },
        title: format!(
            "{}: {} (was {})",
            if redact {
                label(kind, "").trim().to_string()
            } else {
                label(kind, name)
            },
            word(to),
            word(from)
        ),
        from,
        to,
        reasons: if redact { Vec::new() } else { reasons.to_vec() },
        url: format!("{}{}", public_url.trim_end_matches('/'), page(kind)),
    }
}

/// A target's request: extra headers and the body.
pub fn shape(kind: NotifyKind, m: &Message) -> (Vec<(&'static str, String)>, String, bool) {
    match kind {
        NotifyKind::Ntfy => {
            let (priority, tags) = match (m.to, m.to < m.from) {
                (_, true) => ("2", "white_check_mark"),
                (Tone::Bad, _) => ("4", "rotating_light"),
                _ => ("3", "warning"),
            };
            let mut headers = vec![
                ("Title", m.title.clone()),
                ("Priority", priority.into()),
                ("Tags", tags.into()),
            ];
            if m.url.starts_with("http") {
                headers.push(("Click", m.url.clone()));
            }
            let body = if m.reasons.is_empty() {
                m.title.clone()
            } else {
                m.reasons.join("\n")
            };
            (headers, body, false)
        }
        NotifyKind::Slack => {
            let mut text = format!("*Homie*: {}", m.title);
            for r in &m.reasons {
                text.push_str(&format!("\n• {r}"));
            }
            if m.url.starts_with("http") {
                text.push_str(&format!("\n<{}|open Homie>", m.url));
            }
            (Vec::new(), json!({ "text": text }).to_string(), true)
        }
        NotifyKind::Webhook => {
            let body = json!({
                "card": m.card, "name": m.name, "title": m.title,
                "from": m.from, "to": m.to, "reasons": m.reasons, "url": m.url,
                "at": jiff::Timestamp::now().to_string(),
            });
            (Vec::new(), body.to_string(), true)
        }
    }
}

pub struct Target {
    kind: NotifyKind,
    http: http::Target,
    token: Option<String>,
    headers: Vec<(String, String)>,
}

impl Target {
    pub async fn new(t: &NotifyTarget) -> Result<Self> {
        let url = t.url.resolve().await?;
        let tls = crate::config::Tls {
            tls_verify: true,
            ca_file: t.ca_file.clone(),
            tls_server_name: None,
        };
        let mut headers = Vec::new();
        for (k, v) in &t.headers {
            headers.push((k.clone(), v.resolve().await?));
        }
        Ok(Self {
            kind: t.kind,
            http: http::target(&url, &tls).await?,
            token: match &t.token {
                Some(s) => Some(s.resolve().await?),
                None => None,
            },
            headers,
        })
    }

    pub async fn send(&self, m: &Message) -> Result<()> {
        let (extra, body, is_json) = shape(self.kind, m);
        let mut req = self.http.client.post(self.http.base.clone()).body(body);
        if is_json {
            req = req.header("Content-Type", "application/json");
        }
        for (k, v) in extra {
            req = req.header(k, v);
        }
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        http::ok(req.send().await?).await?;
        Ok(())
    }
}

fn kind_name(t: &Target) -> &'static str {
    match t.kind {
        NotifyKind::Ntfy => "ntfy",
        NotifyKind::Slack => "slack",
        NotifyKind::Webhook => "webhook",
    }
}

async fn send_all(targets: &[Target], m: &Message) {
    for t in targets {
        match t.send(m).await {
            Ok(()) => {
                tracing::info!(msg = "notify", card = %m.card, name = %m.name, target = kind_name(t), from = word(m.from), to = word(m.to), result = "ok")
            }
            Err(e) => {
                tracing::warn!(msg = "notify", card = %m.card, name = %m.name, target = kind_name(t), error = %e, result = "error")
            }
        }
    }
}

pub async fn targets(n: &Notify) -> Result<Vec<Target>> {
    let mut out = Vec::new();
    for (i, t) in n.targets.iter().enumerate() {
        out.push(
            Target::new(t)
                .await
                .map_err(|e| format!("notify.targets[{i}]: {e}"))?,
        );
    }
    Ok(out)
}

/// The overview queries to evaluate, as `(kind, name)`. Kubernetes cards of
/// a forward_token cluster need a ServiceAccount to read as.
pub fn cards(lab: &crate::integrations::Lab, n: &Notify) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, n: &str| out.push((k.to_string(), n.to_string()));
    let kube_ok = |k: &crate::integrations::kubernetes::Kube| {
        !k.forwards_token() || n.kubernetes_service_account.is_some()
    };
    for name in lab.proxmox.keys() {
        push("pve", name);
    }
    for (name, k) in &lab.kubernetes {
        if kube_ok(k) {
            for kind in ["k8s", "flux", "argocd", "cnpg"] {
                push(kind, name);
            }
        } else {
            tracing::warn!(msg = "notify skipped", card = "kubernetes", name = %name, reason = "forward_token without notify.kubernetes_service_account");
        }
    }
    for name in lab.grafana.keys() {
        push("alerts", name);
    }
    for name in lab.openbao.keys() {
        push("openbao", name);
    }
    for p in lab.presets() {
        let kyverno_kube = (p == "kyverno")
            .then(|| {
                lab.security
                    .kyverno
                    .as_ref()
                    .and_then(|k| lab.kubernetes.get(&k.kubernetes))
            })
            .flatten();
        if kyverno_kube.is_none_or(kube_ok) {
            push("security", p);
        }
    }
    if lab.tailscale.is_some() {
        push("tailscale", "_");
    }
    if lab.github.as_ref().is_some_and(|g| !g.session_mode()) {
        push("github", "_");
    }
    if lab.healthchecks.is_some() {
        push("healthchecks", "_");
    }
    if lab.certificates_on() {
        push("certs", "_");
    }
    out.retain(|(k, _)| {
        let id = card_id(k);
        if n.cards.is_empty() {
            id != "alerts"
        } else {
            n.cards.iter().any(|c| c == id)
        }
    });
    out
}

fn state_file(dir: Option<&Path>) -> Option<PathBuf> {
    dir.map(|d| d.join("notify.json"))
}

fn load(path: Option<&Path>) -> BTreeMap<String, Option<State>> {
    let Some(p) = path else {
        return BTreeMap::new();
    };
    let tones: BTreeMap<String, Tone> = std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    tones
        .into_iter()
        .map(|(k, t)| (k, Some(State::new(t))))
        .collect()
}

fn save(path: Option<&Path>, states: &BTreeMap<String, Option<State>>) {
    let Some(p) = path else { return };
    let tones: BTreeMap<_, _> = states
        .iter()
        .filter_map(|(k, s)| s.as_ref().filter(|s| !s.quiet).map(|s| (k, s.notified)))
        .collect();
    if let Err(e) = serde_json::to_vec(&tones)
        .map_err(crate::Error::from)
        .and_then(|b| Ok(std::fs::write(p, b)?))
    {
        tracing::warn!(msg = "notify state not saved", error = %e);
    }
}

/// The evaluator: every `interval_seconds`, each card's status through the
/// shared cache; messages for changes that held.
pub async fn run(state: Shared) {
    let Some(n) = state.cfg.notify.clone() else {
        return;
    };
    let targets = match targets(&n).await {
        Ok(t) => Arc::new(t),
        Err(e) => {
            tracing::error!(msg = "notify disabled", error = %e);
            return;
        }
    };
    let caller = Caller {
        email: Some("homie-notify".into()),
        service_account: n.kubernetes_service_account.clone(),
        ..Default::default()
    };
    let cards = cards(&state.lab, &n);
    tracing::info!(msg = "notify", cards = cards.len(), targets = targets.len());
    let path = state_file(state.cfg.state_dir.as_deref());
    let mut states = load(path.as_deref());
    let hold = Duration::from_secs(n.hold_seconds);
    let mut tick = tokio::time::interval(Duration::from_secs(n.interval_seconds));
    loop {
        tick.tick().await;
        let mut changed = false;
        for (kind, name) in &cards {
            let Some(env) = state.lab.query(&state.cache, kind, name, &caller).await else {
                continue;
            };
            let Some(tone) = env.tone else { continue };
            let key = format!("{kind}/{name}");
            let min = n
                .min_tone_per_card
                .get(card_id(kind))
                .copied()
                .unwrap_or(n.min_tone);
            let s = states.entry(key).or_default();
            let before = s.as_ref().map(|s| s.notified);
            if let Some(change) = step(s, tone, Instant::now(), hold, min, n.recovery) {
                let m = message(
                    kind,
                    name,
                    change,
                    &env.reasons,
                    &state.cfg.public_url,
                    n.redact,
                );
                send_all(&targets, &m).await;
            }
            changed |= before != s.as_ref().map(|s| s.notified);
        }
        if changed {
            save(path.as_deref(), &states);
        }
    }
}

/// `homie notify-test`: one message to every target.
pub async fn test(cfg: &crate::config::Config) -> Result<()> {
    let n = cfg.notify.as_ref().ok_or("no [notify] section")?;
    let targets = targets(n).await?;
    let m = Message {
        card: "test".into(),
        name: String::new(),
        title: "Homie: test message (notify-test)".into(),
        from: Tone::Ok,
        to: Tone::Warn,
        reasons: vec!["sent by homie notify-test".into()],
        url: format!("{}/", cfg.public_url.trim_end_matches('/')),
    };
    let mut failed = 0;
    for t in &targets {
        match t.send(&m).await {
            Ok(()) => println!("ok    {}", kind_name(t)),
            Err(e) => {
                failed += 1;
                println!("FAIL  {}: {e}", kind_name(t));
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} of {} targets failed", targets.len()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_secs(180);

    struct Run {
        s: Option<State>,
        t0: Instant,
        min: Tone,
        recovery: bool,
    }

    impl Run {
        fn new(min: Tone) -> Self {
            Self {
                s: None,
                t0: Instant::now(),
                min,
                recovery: true,
            }
        }
        fn at(&mut self, secs: u64, tone: Tone) -> Option<Change> {
            step(
                &mut self.s,
                tone,
                self.t0 + Duration::from_secs(secs),
                HOLD,
                self.min,
                self.recovery,
            )
        }
    }

    #[test]
    fn first_status_is_a_silent_baseline() {
        let mut r = Run::new(Tone::Bad);
        assert_eq!(r.at(0, Tone::Bad), None);
        assert_eq!(r.at(600, Tone::Bad), None);
    }

    #[test]
    fn a_change_must_hold() {
        let mut r = Run::new(Tone::Bad);
        r.at(0, Tone::Ok);
        assert_eq!(r.at(60, Tone::Bad), None);
        assert_eq!(r.at(120, Tone::Bad), None);
        assert_eq!(r.at(240, Tone::Bad), Some((Tone::Ok, Tone::Bad)));
        assert_eq!(r.at(300, Tone::Bad), None);
        // a flap shorter than the hold says nothing
        assert_eq!(r.at(360, Tone::Ok), None);
        assert_eq!(r.at(420, Tone::Bad), None);
        assert_eq!(r.at(700, Tone::Bad), None);
        assert_eq!(r.at(760, Tone::Ok), None);
        assert_eq!(r.at(950, Tone::Ok), Some((Tone::Bad, Tone::Ok)));
    }

    #[test]
    fn thresholds_and_recovery() {
        // amber below a bad threshold is silent both ways
        let mut r = Run::new(Tone::Bad);
        r.at(0, Tone::Ok);
        r.at(1, Tone::Warn);
        assert_eq!(r.at(200, Tone::Warn), None);
        r.at(201, Tone::Ok);
        assert_eq!(r.at(400, Tone::Ok), None);
        // with a warn threshold it is news
        let mut r = Run::new(Tone::Warn);
        r.at(0, Tone::Ok);
        r.at(1, Tone::Warn);
        assert_eq!(r.at(200, Tone::Warn), Some((Tone::Ok, Tone::Warn)));
        // no recovery messages when turned off
        let mut r = Run::new(Tone::Bad);
        r.recovery = false;
        r.at(0, Tone::Bad);
        r.at(1, Tone::Ok);
        assert_eq!(r.at(200, Tone::Ok), None);
        assert_eq!(r.s.as_ref().unwrap().notified, Tone::Ok);
    }

    #[test]
    fn quiet_hours_end_silently() {
        let mut r = Run::new(Tone::Bad);
        r.at(0, Tone::Ok);
        assert_eq!(r.at(60, Tone::Quiet), None);
        assert_eq!(r.at(400, Tone::Quiet), None);
        // the first status after the window is the new baseline
        assert_eq!(r.at(500, Tone::Bad), None);
        assert_eq!(r.at(900, Tone::Bad), None);
        assert_eq!(r.s.as_ref().unwrap().notified, Tone::Bad);
        // starting inside quiet hours too
        let mut r = Run::new(Tone::Bad);
        r.at(0, Tone::Quiet);
        assert_eq!(r.at(500, Tone::Bad), None);
    }

    #[test]
    fn zero_hold_sends_at_once() {
        let mut s = None;
        let t = Instant::now();
        step(&mut s, Tone::Ok, t, Duration::ZERO, Tone::Bad, true);
        assert_eq!(
            step(&mut s, Tone::Bad, t, Duration::ZERO, Tone::Bad, true),
            Some((Tone::Ok, Tone::Bad))
        );
    }

    #[test]
    fn messages_and_shapes() {
        let reasons = vec!["sealed".to_string()];
        let m = message(
            "openbao",
            "lab",
            (Tone::Ok, Tone::Bad),
            &reasons,
            "https://homie.example/",
            false,
        );
        assert_eq!(m.title, "OpenBao lab: failing (was ok)");
        assert_eq!(m.url, "https://homie.example/");
        assert_eq!(
            message("certs", "_", (Tone::Ok, Tone::Warn), &reasons, "", false).card,
            "certificates"
        );
        let r = message(
            "pve",
            "lab",
            (Tone::Ok, Tone::Bad),
            &reasons,
            "https://h",
            true,
        );
        assert_eq!(
            (r.title.as_str(), r.name.as_str(), r.reasons.len()),
            ("Proxmox: failing (was ok)", "", 0)
        );

        let (h, body, json) = shape(NotifyKind::Ntfy, &m);
        assert!(!json);
        assert_eq!(body, "sealed");
        assert!(h.contains(&("Priority", "4".into())));
        assert!(h.contains(&("Click", "https://homie.example/".into())));
        let back = message(
            "openbao",
            "lab",
            (Tone::Bad, Tone::Ok),
            &[],
            "https://h",
            false,
        );
        assert!(
            shape(NotifyKind::Ntfy, &back)
                .0
                .contains(&("Tags", "white_check_mark".into()))
        );

        let (_, body, json) = shape(NotifyKind::Slack, &m);
        assert!(json);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["text"],
            "*Homie*: OpenBao lab: failing (was ok)\n• sealed\n<https://homie.example/|open Homie>"
        );

        let (_, body, _) = shape(NotifyKind::Webhook, &m);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (v["card"].as_str(), v["from"].as_str(), v["to"].as_str()),
            (Some("openbao"), Some("ok"), Some("bad"))
        );
    }

    #[test]
    fn state_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("homie-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = state_file(Some(&dir));
        let mut states = BTreeMap::new();
        states.insert("pve/lab".to_string(), Some(State::new(Tone::Bad)));
        states.insert(
            "k8s/x".to_string(),
            Some(State {
                quiet: true,
                ..State::new(Tone::Ok)
            }),
        );
        save(path.as_deref(), &states);
        let back = load(path.as_deref());
        assert_eq!(back.len(), 1);
        assert_eq!(back["pve/lab"].as_ref().unwrap().notified, Tone::Bad);
        assert!(load(None).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn targets_post_to_their_url() {
        use axum::{Router, extract::State as S, http::HeaderMap, routing::post};
        use std::sync::Mutex;
        type Seen = Arc<Mutex<Vec<(HeaderMap, String)>>>;
        let seen: Seen = Default::default();
        let app = Router::new()
            .route(
                "/topic",
                post(|S(s): S<Seen>, h: HeaderMap, b: String| async move {
                    s.lock().unwrap().push((h, b));
                }),
            )
            .with_state(seen.clone());
        let url = crate::testkit::serve(app).await;
        let cfg: NotifyTarget = serde_json::from_value(json!({
            "type": "ntfy", "url": format!("{url}/topic"), "token": "tk", "headers": {"X-Extra": "1"}
        }))
        .unwrap();
        let t = Target::new(&cfg).await.unwrap();
        let m = message(
            "pve",
            "lab",
            (Tone::Ok, Tone::Bad),
            &["2/3 nodes online".into()],
            "https://h",
            false,
        );
        t.send(&m).await.unwrap();
        let (h, b) = seen.lock().unwrap()[0].clone();
        assert_eq!(b, "2/3 nodes online");
        assert_eq!(h["authorization"], "Bearer tk");
        assert_eq!(h["title"], "Proxmox lab: failing (was ok)");
        assert_eq!(h["x-extra"], "1");
        // a failing target is an error
        let bad: NotifyTarget =
            serde_json::from_value(json!({"type": "webhook", "url": format!("{url}/missing")}))
                .unwrap();
        assert!(Target::new(&bad).await.unwrap().send(&m).await.is_err());
    }
}

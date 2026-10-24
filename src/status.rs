//! Card status (RFC 0019): the tone of each overview card and the reasons
//! behind it, computed from a query's data. The UI shows this tone, so the
//! page and anything else that reads it (notifications) cannot disagree.

use serde::Serialize;
use serde_json::Value;

use crate::cache::Envelope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Ok,
    Warn,
    Bad,
    Quiet,
}

/// Sets `tone` and `reasons` on the envelope of a query that has a card.
pub fn apply(kind: &str, env: &mut Envelope) {
    apply_at(kind, env, jiff::Timestamp::now());
}

fn apply_at(kind: &str, env: &mut Envelope, now: jiff::Timestamp) {
    let Some(summary) = env.data.as_ref().map(|d| summarize(kind, d, now)) else {
        if has_card(kind) {
            env.tone = Some(if env.quiet { Tone::Quiet } else { Tone::Bad });
        }
        return;
    };
    let Some((tone, reasons)) = summary else {
        return;
    };
    // a failing source with no data is the headline (above); stale data
    // still counts but is flagged
    env.tone = Some(if env.error.is_some() && env.quiet {
        Tone::Quiet
    } else if env.stale && tone == Tone::Ok {
        Tone::Warn
    } else {
        tone
    });
    env.reasons = reasons;
}

fn has_card(kind: &str) -> bool {
    summarize(kind, &Value::Null, jiff::Timestamp::UNIX_EPOCH).is_some()
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_default()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn names<'a>(items: impl Iterator<Item = &'a Value>) -> String {
    items
        .map(|i| text(&i["name"]))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The tone and the problems, or `None` for a query without a card.
fn summarize(kind: &str, d: &Value, now: jiff::Timestamp) -> Option<(Tone, Vec<String>)> {
    let mut reasons = Vec::new();
    let tone = match kind {
        "pve" => {
            let c = &d["cluster"];
            let (online, total) = (num(&c["nodes_online"]), num(&c["nodes_total"]));
            let quorate = c["quorate"].as_bool() != Some(false);
            let full: Vec<_> = arr(&d["storage"])
                .iter()
                .filter(|s| num(&s["maxdisk"]) > 0.0 && num(&s["disk"]) / num(&s["maxdisk"]) > 0.85)
                .map(|s| format!("{}@{}", text(&s["storage"]), text(&s["node"])))
                .collect();
            let failed = arr(&d["failed_tasks"]).len();
            if online < total {
                reasons.push(format!("{online}/{total} nodes online"));
            }
            if !quorate {
                reasons.push("not quorate".into());
            }
            if !full.is_empty() {
                reasons.push(format!("storage above 85%: {}", full.join(", ")));
            }
            if failed > 0 {
                reasons.push(format!("{failed} recent failed tasks"));
            }
            if online < total || !quorate {
                Tone::Bad
            } else if !full.is_empty() || failed > 0 {
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        "k8s" => {
            let nodes = arr(&d["nodes"]);
            let ready = nodes.iter().filter(|n| n["ready"] == true).count();
            let unhealthy = arr(&d["unhealthy_pods"]).len();
            let degraded = arr(&d["workloads"])
                .iter()
                .filter(|w| num(&w["ready"]) < num(&w["desired"]))
                .count();
            if ready < nodes.len() {
                reasons.push(format!("{ready}/{} nodes ready", nodes.len()));
            }
            if unhealthy > 0 {
                reasons.push(format!("{unhealthy} unhealthy pods"));
            }
            if degraded > 0 {
                reasons.push(format!("{degraded} degraded workloads"));
            }
            if ready < nodes.len() {
                Tone::Bad
            } else if unhealthy > 0 || degraded > 0 {
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        "flux" => {
            let items = arr(d);
            let failing: Vec<_> = items.iter().filter(|i| i["ready"] == "False").collect();
            let suspended = items.iter().filter(|i| i["suspended"] == true).count();
            if !failing.is_empty() {
                reasons.push(format!("failing: {}", names(failing.iter().copied())));
            }
            if suspended > 0 {
                reasons.push(format!("{suspended} suspended"));
            }
            if !failing.is_empty() {
                Tone::Bad
            } else if suspended > 0 {
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        "argocd" => {
            let apps = arr(d);
            let bad: Vec<_> = apps
                .iter()
                .filter(|a| a["health"] == "Degraded" || a["health"] == "Missing")
                .collect();
            let out: Vec<_> = apps.iter().filter(|a| a["sync"] == "OutOfSync").collect();
            if !bad.is_empty() {
                reasons.push(format!("degraded: {}", names(bad.iter().copied())));
            }
            if !out.is_empty() {
                reasons.push(format!("out of sync: {}", names(out.iter().copied())));
            }
            if !bad.is_empty() {
                Tone::Bad
            } else if !out.is_empty() {
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        "alerts" => {
            let alerts = arr(&d["alerts"]);
            let firing: Vec<_> = alerts.iter().filter(|a| a["state"] == "firing").collect();
            if !firing.is_empty() {
                reasons.push(format!("firing: {}", names(firing.iter().copied())));
            }
            if alerts.len() > firing.len() {
                reasons.push(format!(
                    "{} pending or erroring",
                    alerts.len() - firing.len()
                ));
            }
            if !firing.is_empty() {
                Tone::Bad
            } else if !alerts.is_empty() {
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        "security" => {
            let total = num(&d["total"]);
            if total > 0.0 {
                reasons.push(format!("{total} {}", text(&d["total_label"])));
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        "tailscale" => {
            // only watched hosts (config `hosts`) count toward health
            let loud: Vec<_> = arr(d)
                .iter()
                .filter(|x| x["watched"] == true && x["online"] != true && x["quiet"] != true)
                .collect();
            if loud.is_empty() {
                Tone::Ok
            } else {
                reasons.push(format!("offline: {}", names(loud.into_iter())));
                Tone::Bad
            }
        }
        "openbao" => {
            if d["sealed"] == true {
                reasons.push("sealed".into());
            } else if d["initialized"] != true {
                reasons.push("not initialized".into());
            } else if text(&d["state"]).starts_with("HTTP") {
                reasons.push(text(&d["state"]).into());
            }
            if reasons.is_empty() {
                Tone::Ok
            } else {
                Tone::Bad
            }
        }
        "cnpg" => {
            let mut tone = Tone::Ok;
            for c in arr(d) {
                let name = format!("{}/{}", text(&c["namespace"]), text(&c["name"]));
                if c["healthy"] != true {
                    reasons.push(format!("{name}: unhealthy"));
                    tone = Tone::Bad;
                }
                if c["backup_stale"] == true {
                    reasons.push(format!("{name}: backup stale"));
                    tone = tone.max(Tone::Warn);
                }
                if c["archiving"] != true {
                    reasons.push(format!("{name}: WAL archiving off"));
                    tone = tone.max(Tone::Warn);
                }
            }
            tone
        }
        "github" => {
            let mut tone = Tone::Ok;
            for w in arr(d) {
                let name = text(&w["name"]);
                match workflow_health(w, now) {
                    _ if !w["error"].is_null() => {
                        reasons.push(format!("failed: {name}"));
                        tone = Tone::Bad;
                    }
                    Health::Failed => {
                        reasons.push(format!("failed: {name}"));
                        tone = Tone::Bad;
                    }
                    Health::Stuck => {
                        reasons.push(format!("queued over 10 min: {name}"));
                        tone = tone.max(Tone::Warn);
                    }
                    Health::Ok | Health::Unknown => {}
                }
            }
            tone
        }
        "healthchecks" => {
            let checks = arr(d);
            let down: Vec<_> = checks.iter().filter(|c| c["status"] == "down").collect();
            let late: Vec<_> = checks.iter().filter(|c| c["status"] == "grace").collect();
            if !down.is_empty() {
                reasons.push(format!("down: {}", names(down.iter().copied())));
            }
            if !late.is_empty() {
                reasons.push(format!("late: {}", names(late.iter().copied())));
            }
            if !down.is_empty() {
                Tone::Bad
            } else if !late.is_empty() {
                Tone::Warn
            } else {
                Tone::Ok
            }
        }
        _ => return None,
    };
    Some((tone, reasons))
}

#[derive(Debug, PartialEq)]
enum Health {
    Failed,
    Stuck,
    Ok,
    Unknown,
}

/// The run that decides health: the newest completed one, and whether a
/// run waits too long. Mirrors `workflowHealth` in `web/src/api.ts`.
fn workflow_health(w: &Value, now: jiff::Timestamp) -> Health {
    let runs = arr(&w["runs"]);
    let queued = runs
        .iter()
        .find(|r| matches!(text(&r["status"]), "queued" | "waiting" | "pending"));
    if let Some(r) = queued
        && text(&r["started"])
            .parse::<jiff::Timestamp>()
            .is_ok_and(|t| now.duration_since(t).as_secs() > 600)
    {
        return Health::Stuck;
    }
    let last = runs.iter().find(|r| {
        r["status"] == "completed" && !matches!(text(&r["conclusion"]), "skipped" | "cancelled")
    });
    match last {
        None => Health::Unknown,
        Some(r) if r["conclusion"] == "success" => Health::Ok,
        Some(_) => Health::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> jiff::Timestamp {
        "2026-10-24T12:00:00Z".parse().unwrap()
    }

    fn tone(kind: &str, d: Value) -> (Tone, Vec<String>) {
        summarize(kind, &d, now()).unwrap()
    }

    fn env(data: Option<Value>, stale: bool, quiet: bool, error: Option<&str>) -> Envelope {
        Envelope {
            data,
            fetched_at: None,
            stale,
            quiet,
            error: error.map(Into::into),
            tone: None,
            reasons: Vec::new(),
        }
    }

    #[test]
    fn pve() {
        let ok = json!({"cluster": {"quorate": true, "nodes_online": 3, "nodes_total": 3},
                        "storage": [{"storage": "local", "node": "n1", "disk": 10, "maxdisk": 100}],
                        "failed_tasks": []});
        assert_eq!(tone("pve", ok), (Tone::Ok, vec![]));
        let full = json!({"cluster": {"quorate": true, "nodes_online": 3, "nodes_total": 3},
                          "storage": [{"storage": "local", "node": "n1", "disk": 90, "maxdisk": 100},
                                      {"storage": "empty", "node": "n1", "disk": 0, "maxdisk": 0}],
                          "failed_tasks": [{}]});
        assert_eq!(
            tone("pve", full),
            (
                Tone::Warn,
                vec![
                    "storage above 85%: local@n1".into(),
                    "1 recent failed tasks".into()
                ]
            )
        );
        let down = json!({"cluster": {"quorate": false, "nodes_online": 2, "nodes_total": 3}});
        assert_eq!(
            tone("pve", down),
            (
                Tone::Bad,
                vec!["2/3 nodes online".into(), "not quorate".into()]
            )
        );
        // a single node has no cluster: quorate is null
        let single = json!({"cluster": {"quorate": null, "nodes_online": 1, "nodes_total": 1}});
        assert_eq!(tone("pve", single).0, Tone::Ok);
    }

    #[test]
    fn k8s() {
        let d = |ready: bool, pods: Value, w_ready: u32| {
            json!({"nodes": [{"ready": true}, {"ready": ready}], "unhealthy_pods": pods,
                   "workloads": [{"ready": w_ready, "desired": 2}]})
        };
        assert_eq!(tone("k8s", d(true, json!([]), 2)).0, Tone::Ok);
        assert_eq!(tone("k8s", d(true, json!([{}]), 2)).0, Tone::Warn);
        assert_eq!(
            tone("k8s", d(true, json!([]), 1)),
            (Tone::Warn, vec!["1 degraded workloads".into()])
        );
        assert_eq!(
            tone("k8s", d(false, json!([]), 2)),
            (Tone::Bad, vec!["1/2 nodes ready".into()])
        );
    }

    #[test]
    fn flux_and_argocd() {
        assert_eq!(tone("flux", json!([])).0, Tone::Ok);
        assert_eq!(
            tone(
                "flux",
                json!([{"name": "a", "ready": "True", "suspended": true}])
            ),
            (Tone::Warn, vec!["1 suspended".into()])
        );
        assert_eq!(
            tone(
                "flux",
                json!([{"name": "a", "ready": "False"}, {"name": "b", "ready": "Unknown"}])
            ),
            (Tone::Bad, vec!["failing: a".into()])
        );
        assert_eq!(
            tone(
                "argocd",
                json!([{"name": "a", "health": "Healthy", "sync": "OutOfSync"}])
            )
            .0,
            Tone::Warn
        );
        assert_eq!(
            tone(
                "argocd",
                json!([{"name": "a", "health": "Missing", "sync": "Synced"}])
            )
            .0,
            Tone::Bad
        );
    }

    #[test]
    fn alerts_security_openbao() {
        assert_eq!(tone("alerts", json!({"alerts": []})).0, Tone::Ok);
        assert_eq!(
            tone("alerts", json!({"alerts": [{"state": "pending"}]})).0,
            Tone::Warn
        );
        assert_eq!(
            tone(
                "alerts",
                json!({"alerts": [{"name": "Down", "state": "firing"}]})
            ),
            (Tone::Bad, vec!["firing: Down".into()])
        );
        assert_eq!(tone("security", json!({"total": 0})).0, Tone::Ok);
        assert_eq!(
            tone("security", json!({"total": 3, "total_label": "violations"})),
            (Tone::Warn, vec!["3 violations".into()])
        );
        let bao =
            |sealed, init, state| json!({"sealed": sealed, "initialized": init, "state": state});
        assert_eq!(tone("openbao", bao(false, true, "active")).0, Tone::Ok);
        assert_eq!(
            tone("openbao", bao(true, true, "active")),
            (Tone::Bad, vec!["sealed".into()])
        );
        assert_eq!(tone("openbao", bao(false, false, "active")).0, Tone::Bad);
        assert_eq!(tone("openbao", bao(false, true, "HTTP 503")).0, Tone::Bad);
    }

    #[test]
    fn tailscale_counts_watched_loud_hosts_only() {
        let d = json!([
            {"name": "a", "watched": true, "online": true},
            {"name": "b", "watched": true, "online": false, "quiet": true},
            {"name": "c", "watched": false, "online": false},
        ]);
        assert_eq!(tone("tailscale", d).0, Tone::Ok);
        let d = json!([{"name": "a", "watched": true, "online": false, "quiet": false}]);
        assert_eq!(tone("tailscale", d), (Tone::Bad, vec!["offline: a".into()]));
    }

    #[test]
    fn cnpg() {
        let c = |healthy, stale, archiving| json!([{"namespace": "db", "name": "pg", "healthy": healthy, "backup_stale": stale, "archiving": archiving}]);
        assert_eq!(tone("cnpg", c(true, false, true)).0, Tone::Ok);
        assert_eq!(
            tone("cnpg", c(true, true, true)),
            (Tone::Warn, vec!["db/pg: backup stale".into()])
        );
        assert_eq!(tone("cnpg", c(true, false, false)).0, Tone::Warn);
        assert_eq!(tone("cnpg", c(false, true, true)).0, Tone::Bad);
    }

    #[test]
    fn github_workflows() {
        let run = |status: &str, conclusion: Value, started: &str| json!({"status": status, "conclusion": conclusion, "started": started});
        let w = |runs: Vec<Value>, error: Value| json!([{"name": "plan", "runs": runs, "error": error}]);
        let ok = run("completed", json!("success"), "2026-10-24T11:00:00Z");
        assert_eq!(tone("github", w(vec![ok.clone()], Value::Null)).0, Tone::Ok);
        // skipped and cancelled runs do not decide health
        let skipped = run("completed", json!("skipped"), "2026-10-24T11:30:00Z");
        let failed = run("completed", json!("failure"), "2026-10-24T10:00:00Z");
        assert_eq!(
            tone("github", w(vec![skipped, failed], Value::Null)),
            (Tone::Bad, vec!["failed: plan".into()])
        );
        let queued_old = run("queued", Value::Null, "2026-10-24T11:49:00Z");
        assert_eq!(
            tone("github", w(vec![queued_old, ok.clone()], Value::Null)),
            (Tone::Warn, vec!["queued over 10 min: plan".into()])
        );
        let queued_new = run("queued", Value::Null, "2026-10-24T11:55:00Z");
        assert_eq!(
            tone("github", w(vec![queued_new, ok], Value::Null)).0,
            Tone::Ok
        );
        assert_eq!(tone("github", w(vec![], json!("not found"))).0, Tone::Bad);
        assert_eq!(tone("github", w(vec![], Value::Null)).0, Tone::Ok);
    }

    #[test]
    fn healthchecks() {
        let d = json!([{"name": "a", "status": "up"}, {"name": "b", "status": "grace"}]);
        assert_eq!(
            tone("healthchecks", d),
            (Tone::Warn, vec!["late: b".into()])
        );
        let d = json!([{"name": "a", "status": "down"}, {"name": "b", "status": "grace"}]);
        assert_eq!(
            tone("healthchecks", d),
            (Tone::Bad, vec!["down: a".into(), "late: b".into()])
        );
    }

    #[test]
    fn envelope_state_combines_with_the_summary() {
        let ok = || Some(json!({"total": 0}));
        let warn = || Some(json!({"total": 1, "total_label": "x"}));
        let cases = [
            (env(ok(), false, false, None), Some(Tone::Ok)),
            // stale data that looks fine is still flagged
            (env(ok(), true, false, Some("timeout")), Some(Tone::Warn)),
            (env(warn(), true, false, Some("timeout")), Some(Tone::Warn)),
            (env(None, false, false, Some("timeout")), Some(Tone::Bad)),
            (env(None, false, true, Some("timeout")), Some(Tone::Quiet)),
            (env(ok(), true, true, Some("timeout")), Some(Tone::Quiet)),
        ];
        for (mut e, want) in cases {
            apply_at("security", &mut e, now());
            assert_eq!(e.tone, want, "{e:?}");
        }
        // queries without a card get no tone
        let mut e = env(Some(json!({})), false, false, None);
        apply_at("helm", &mut e, now());
        assert_eq!(e.tone, None);
        let mut e = env(None, false, false, Some("x"));
        apply_at("spark-pve", &mut e, now());
        assert_eq!(e.tone, None);
    }
}

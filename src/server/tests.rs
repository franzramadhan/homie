//! End-to-end tests through the real router, against fake backends.

use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};

use super::harness::{Homie, backend, lab_sections, oidc_login, password_hash, provider, temp_dir};

/// No login, a public_url to compare Host headers against, every integration.
async fn open_homie() -> Homie {
    let backend = backend().await;
    Homie::start(|_| {
        format!(
            "public_url = \"http://homie.example\"\nallow_unauthenticated = true\nrefresh_seconds = 5\n{}",
            lab_sections(&backend)
        )
    })
    .await
}

fn local_login(role: Option<&str>) -> String {
    let role = role
        .map(|r| format!("role = \"{r}\"\n"))
        .unwrap_or_default();
    format!(
        "[local_login]\npassword_hash = \"{}\"\n{role}",
        password_hash()
    )
}

const VIEWER: &str = r#"
[roles.viewer]
pages = ["infra", "workloads"]
namespaces = ["apps"]
proxmox_tags = ["demo"]
kubernetes_service_account = "homie/viewer"
"#;

/// Reads the event stream until `done` holds for what arrived, or `secs` pass.
async fn read_stream(
    resp: &mut reqwest::Response,
    secs: u64,
    done: impl Fn(&str) -> bool,
) -> (String, bool) {
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, resp.chunk()).await {
            Ok(Ok(Some(c))) => {
                seen.push_str(&String::from_utf8_lossy(&c));
                if done(&seen) {
                    return (seen, false);
                }
            }
            // the stream ended
            Ok(_) => return (seen, true),
            Err(_) => panic!("stream timed out; got {seen}"),
        }
    }
}

#[tokio::test]
async fn host_checks_and_security_headers() {
    let h = open_homie().await;
    let host = |name: &'static str| h.req(Method::GET, "/healthz").header("host", name);
    let evil = h.send(host("evil.example")).await;
    assert_eq!(evil.status, 421, "DNS rebinding is refused");
    assert_eq!(evil.json["error"], "unexpected Host header");
    for ok in ["homie.example", "localhost:7777", "[::1]:7777", "127.0.0.1"] {
        assert_eq!(h.send(host(ok)).await.status, 200, "{ok}");
    }
    let r = h.get("/healthz").await;
    assert_eq!(r.text, "ok");
    assert_eq!(r.headers["x-frame-options"], "DENY");
    assert_eq!(r.headers["x-content-type-options"], "nosniff");
    assert!(
        r.headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );

    // without a public_url, allow_unauthenticated trusts the proxy in front
    let any = Homie::start(|_| "allow_unauthenticated = true".into()).await;
    let r = any
        .send(
            any.req(Method::GET, "/healthz")
                .header("host", "proxy.example"),
        )
        .await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn writes_need_the_header_and_a_same_site_origin() {
    let h = open_homie().await;
    let body = json!({"kind": "Kustomization", "namespace": "apps", "name": "web"});
    let path = "/api/act/flux/main/reconcile";
    let r = h.send(h.bare(Method::POST, path).json(&body)).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (403, Some("missing X-Homie header"))
    );
    let from = |origin: &'static str| {
        h.req(Method::POST, path)
            .header("origin", origin)
            .json(&body)
    };
    let r = h.send(from("http://evil.example")).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (403, Some("cross-origin request"))
    );
    for origin in ["http://homie.example", "http://localhost:7777"] {
        let r = h.send(from(origin)).await;
        assert_eq!(r.status, 200, "{origin}: {}", r.text);
    }
}

/// Keeps `guard::write_denied` equal to `write_ok` in spec/guard.bend, whose
/// laws (LAWS.bend) are proven by `bend PROOF.bend`: with and without login.
#[tokio::test]
async fn writes_match_spec() {
    let open = open_homie().await;
    let login = Homie::start(|_| {
        format!(
            "public_url = \"http://homie.example\"\n{}",
            local_login(None)
        )
    })
    .await;
    // (Origin header, the model's Origin), per instance
    // without login a loopback origin counts by host, any port; the
    // public_url host needs its scheme and port too
    let same_open = [
        "http://homie.example",
        "http://localhost:7777",
        "http://localhost:8080",
        "http://127.0.0.1",
    ];
    let same_login = ["http://homie.example"];
    let foreign = [
        "https://homie.example",
        "http://homie.example:8443",
        "http://evil.example",
        "http://homie.example.evil.example",
        "null",
    ];
    let foreign_login = ["http://localhost:7777"];
    for (h, same, extra) in [
        (&open, &same_open[..], &[][..]),
        (&login, &same_login[..], &foreign_login[..]),
    ] {
        let origins = std::iter::once((None, "Absent"))
            .chain(same.iter().map(|o| (Some(*o), "Same")))
            .chain(foreign.iter().chain(extra).map(|o| (Some(*o), "Foreign")));
        for (origin, class) in origins {
            for x_homie in [false, true] {
                let mut headers = axum::http::HeaderMap::new();
                if x_homie {
                    headers.insert("x-homie", "1".parse().unwrap());
                }
                if let Some(o) = origin {
                    headers.insert("origin", o.parse().unwrap());
                }
                assert_eq!(
                    crate::server::guard::write_denied(&h.state, &headers).is_none(),
                    x_homie && class != "Foreign",
                    "{origin:?} x_homie={x_homie} login={}",
                    h.state.auth.is_some()
                );
            }
        }
    }
}

#[tokio::test]
async fn meta_and_queries_with_full_access() {
    let h = open_homie().await;
    let m = h.get("/api/meta").await.json;
    assert_eq!(m["auth"], false);
    assert_eq!(m["user"], Value::Null);
    assert_eq!(m["role"], Value::Null);
    assert_eq!(m["refresh_seconds"], 5);
    assert_eq!(m["proxmox"][0]["name"], "lab");
    assert!(
        m["proxmox"][0]["ui_url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1")
    );
    assert_eq!(
        m["kubernetes"][0]["headlamp_url"],
        "https://headlamp.example.com"
    );
    assert_eq!(m["kubernetes"][0]["metrics"], true);
    assert_eq!(m["kubernetes"][0]["forward_token"], false);
    assert_eq!(m["grafana"][0]["name"], "mon");
    assert_eq!(m["openbao"][0]["name"], "vault");
    assert_eq!(
        m["security"],
        json!(["kyverno", "tetragon", "wazuh", "debsecan"])
    );
    assert_eq!(m["logs"][0]["label"], "Loki");
    assert_eq!(m["tailscale"], false);
    assert_eq!(m["github"], Value::Null);
    assert!(m["healthchecks"]["url"].is_string());

    let pve = h.get("/api/q/pve/lab").await;
    assert_eq!(pve.status, 200);
    assert_eq!(pve.json["error"], Value::Null);
    assert_eq!(pve.json["data"]["guests"].as_array().unwrap().len(), 2);
    assert_eq!(pve.json["data"]["cluster"]["name"], "lab");

    for q in [
        "spark-pve/lab",
        "spark-k8s/main",
        "k8s/main",
        "helm/main",
        "flux/main",
        "argocd/main",
        "cnpg/main",
        "alerts/mon",
        "openbao/vault",
        "security/kyverno",
        "security/tetragon",
        "security/wazuh",
        "security/debsecan",
        "healthchecks/hc",
    ] {
        let r = h.get(&format!("/api/q/{q}")).await;
        assert_eq!(r.status, 200, "{q}");
        assert_eq!(r.json["error"], Value::Null, "{q}: {}", r.text);
        assert!(!r.json["data"].is_null(), "{q}");
    }
    // configured but failing: an envelope with the error
    let ts = h.get("/api/q/tailscale/main").await;
    assert_eq!(ts.status, 200);
    assert_eq!(ts.json["error"], "tailscale is not configured");

    for q in [
        "pve/missing",
        "nope/x",
        "security/none",
        "github/main",
        "openbao/missing",
    ] {
        let r = h.get(&format!("/api/q/{q}")).await;
        assert_eq!(r.status, 404, "{q}");
        assert_eq!(r.json["error"], format!("no query {q}"));
    }
}

#[tokio::test]
async fn actions_against_fake_backends() {
    let h = open_homie().await;
    let power =
        |action: &str| json!({"node": "pve1", "vmid": 101, "type": "qemu", "action": action});
    let r = h.post("/api/act/pve/lab/power", power("start")).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let upid = r.json["upid"].as_str().unwrap().to_string();
    assert!(upid.starts_with("UPID:pve1"));
    let r = h.post("/api/act/pve/lab/power", power("stop")).await;
    assert_eq!(r.status, 400);
    assert!(r.json["error"].as_str().unwrap().contains("hard power-off"));
    let r = h.post("/api/act/pve/nope/power", power("start")).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (404, Some("no such Proxmox"))
    );

    let task = |pve: &str, upid: &str| {
        h.req(Method::GET, &format!("/api/act/pve/{pve}/task"))
            .query(&[("node", "pve1"), ("upid", upid)])
    };
    let r = h.send(task("lab", &upid)).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.json, json!({"status": "stopped", "exitstatus": "OK"}));
    assert_eq!(h.send(task("lab", "nope")).await.status, 502);
    assert_eq!(h.send(task("nope", &upid)).await.status, 404);

    let flux = json!({"kind": "Kustomization", "namespace": "apps", "name": "web"});
    let r = h.post("/api/act/flux/main/reconcile", flux.clone()).await;
    assert_eq!(r.json["reconciled"], "Kustomization/apps/web");
    let r = h
        .post(
            "/api/act/flux/main/reconcile",
            json!({"kind": "Pod", "namespace": "apps", "name": "web"}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        h.post("/api/act/flux/nope/reconcile", flux).await.status,
        404
    );

    let app = json!({"namespace": "argocd", "name": "guestbook"});
    let r = h.post("/api/act/argocd/main/refresh", app.clone()).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(r.json["refreshed"].as_str().unwrap().contains("guestbook"));
    let r = h
        .post(
            "/api/act/argocd/main/refresh",
            json!({"namespace": "Bad Name", "name": "x"}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        h.post("/api/act/argocd/nope/refresh", app).await.status,
        404
    );

    let r = h
        .post(
            "/api/logs",
            json!({"source": "Loki", "query": "{app=\"web\"}", "minutes": 15}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.json["rows"], json!([]));
    let r = h
        .post(
            "/api/logs",
            json!({"source": "Loki", "query": "x", "minutes": 0}),
        )
        .await;
    assert_eq!(r.status, 400);
}

#[tokio::test]
async fn login_routes_without_a_login_method() {
    let h = open_homie().await;
    assert_eq!(
        h.get("/auth/options").await.json,
        json!({"oidc": false, "local": false, "demo": false})
    );
    let r = h
        .post("/auth/local", json!({"username": "a", "password": "b"}))
        .await;
    assert_eq!(r.status, 404);
    assert_eq!(h.post("/auth/demo", json!({})).await.status, 404);
    let r = h.get("/auth/login").await;
    assert_eq!(r.status, 303);
    assert_eq!(r.headers["location"], "/login");
    let r = h.get("/auth/callback?code=a&state=b").await;
    assert_eq!(r.status, 303);
    assert_eq!(r.headers["location"], "/");
    assert_eq!(h.post("/auth/logout", json!({})).await.status, 204);
}

#[tokio::test]
async fn password_login_session_and_logout() {
    let h = Homie::start(|url| format!("public_url = \"{url}\"\n{}", local_login(None))).await;
    assert_eq!(
        h.get("/auth/options").await.json,
        json!({"oidc": false, "local": true, "demo": false})
    );
    let r = h.get("/api/meta").await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (401, Some("login required"))
    );
    // with a login, the Host header is not checked
    let r = h
        .send(
            h.req(Method::GET, "/healthz")
                .header("host", "elsewhere.example"),
        )
        .await;
    assert_eq!(r.status, 200);

    let creds = json!({"username": "homie", "password": super::harness::PASSWORD});
    let r = h
        .send(h.bare(Method::POST, "/auth/local").json(&creds))
        .await;
    assert_eq!(r.status, 403, "no X-Homie header");
    let r = h
        .send(
            h.req(Method::POST, "/auth/local")
                .header("origin", "https://evil.example")
                .json(&creds),
        )
        .await;
    assert_eq!(r.status, 403, "cross-origin");
    let r = h
        .post(
            "/auth/local",
            json!({"username": "homie", "password": "wrong"}),
        )
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json["error"], "wrong username or password");

    let r = h
        .send(
            h.req(Method::POST, "/auth/local")
                .header("origin", h.url.clone())
                .json(&creds),
        )
        .await;
    assert_eq!(r.status, 204);
    let set = r.headers["set-cookie"].to_str().unwrap();
    assert!(set.starts_with("__Host-homie="), "{set}");
    assert!(set.contains("HttpOnly") && set.contains("Secure") && set.contains("Max-Age=43200"));
    let m = h.get("/api/meta").await.json;
    assert_eq!(
        (m["auth"].as_bool(), m["user"].as_str()),
        (Some(true), Some("homie"))
    );
    assert_eq!(m["role"], Value::Null);

    // OIDC routes when only password login exists
    assert_eq!(h.get("/auth/login").await.headers["location"], "/login");
    let r = h.get("/auth/callback?code=a&state=b").await;
    assert_eq!(r.status, 403);
    assert!(
        r.text.contains("OIDC login is not configured"),
        "{}",
        r.text
    );
    let r = h
        .get("/auth/callback?error=access_denied&error_description=nope")
        .await;
    assert!(
        r.text.starts_with("Login failed: access_denied nope"),
        "{}",
        r.text
    );
    let r = h.get("/auth/callback").await;
    assert!(r.text.contains("missing code or state"));
    let r = h.post("/auth/demo", json!({})).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (404, Some("the demo is not configured"))
    );
    let r = h.send(h.bare(Method::POST, "/auth/demo")).await;
    assert_eq!(r.status, 403);

    let session = h.cookie();
    h.set_cookie(Some("__Host-homie=forged".into()));
    assert_eq!(h.get("/api/meta").await.status, 401);
    h.set_cookie(session);

    let r = h.send(h.bare(Method::POST, "/auth/logout")).await;
    assert_eq!(r.status, 403);
    let r = h.post("/auth/logout", json!({})).await;
    assert_eq!(r.status, 204);
    assert!(
        r.headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(h.get("/api/meta").await.status, 401);
}

#[tokio::test]
async fn roles_see_their_pages_only() {
    let backend = backend().await;
    let h = Homie::start(|url| {
        format!(
            "public_url = \"{url}\"\n{}[demo]\nrole = \"viewer\"\nmax_sessions = 1\n{VIEWER}{}",
            local_login(Some("viewer")),
            lab_sections(&backend)
        )
    })
    .await;
    assert_eq!(h.get("/auth/options").await.json["demo"], true);
    let r = h.post("/auth/demo", json!({})).await;
    assert_eq!(r.status, 204);
    assert!(
        r.headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=3600")
    );
    let m = h.get("/api/meta").await.json;
    assert_eq!(
        (m["user"].as_str(), m["role"].as_str()),
        (Some("demo"), Some("viewer"))
    );
    assert_eq!(m["pages"], json!(["infra", "workloads"]));
    assert_eq!(
        m["proxmox"][0]["ui_url"],
        Value::Null,
        "no links to other UIs"
    );
    assert_eq!(m["kubernetes"][0]["headlamp_url"], Value::Null);
    assert_eq!(m["kubernetes"][0]["argocd_url"], Value::Null);
    for empty in ["grafana", "openbao", "logs", "security"] {
        assert_eq!(m[empty], json!([]), "{empty}");
    }
    assert_eq!(m["healthchecks"], Value::Null);

    let pve = h.get("/api/q/pve/lab").await.json;
    let guests: Vec<_> = pve["data"]["guests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["vmid"].as_u64().unwrap())
        .collect();
    assert_eq!(guests, [101], "only guests with the role's tags");
    assert_eq!(h.get("/api/q/spark-pve/lab").await.status, 200);
    let k8s = h.get("/api/q/k8s/main").await;
    assert_eq!(
        (k8s.status, &k8s.json["error"]),
        (200, &Value::Null),
        "{}",
        k8s.text
    );

    for q in ["flux/main", "alerts/mon", "helm/main", "argocd/main"] {
        let r = h.get(&format!("/api/q/{q}")).await;
        assert_eq!(r.status, 403, "{q}");
    }
    let r = h
        .post(
            "/api/act/pve/lab/power",
            json!({"node": "pve1", "vmid": 101, "type": "qemu", "action": "start"}),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (403, Some("not available in this view"))
    );
    let r = h
        .post(
            "/api/logs",
            json!({"source": "Loki", "query": "x", "minutes": 5}),
        )
        .await;
    assert_eq!(r.status, 403);

    // roles stream with the refresh tick only
    let mut resp = h
        .req(Method::GET, "/api/stream")
        .query(&[("q", "pve/lab,alerts/mon")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let (seen, _) = read_stream(&mut resp, 15, |s| s.contains("pve/lab")).await;
    assert!(!seen.contains("alerts/mon"), "{seen}");
    drop(resp);

    // one demo session at most: a second one ends the first
    let first = h.cookie();
    assert_eq!(h.post("/auth/demo", json!({})).await.status, 204);
    let second = h.cookie();
    h.set_cookie(first);
    assert_eq!(h.get("/api/meta").await.status, 401);
    h.set_cookie(second);
    assert_eq!(h.get("/api/meta").await.status, 200);

    h.set_cookie(None);
    assert_eq!(h.login().await.status, 204);
    let m = h.get("/api/meta").await.json;
    assert_eq!(
        (m["user"].as_str(), m["role"].as_str()),
        (Some("homie"), Some("viewer"))
    );
}

#[tokio::test]
async fn a_role_missing_from_the_lab_fails_closed() {
    let h = Homie::start_with(
        |url| {
            format!(
                "public_url = \"{url}\"\n{}[demo]\nrole = \"viewer\"\n{VIEWER}",
                local_login(None)
            )
        },
        |state| state.lab.roles.clear(),
    )
    .await;
    assert_eq!(h.post("/auth/demo", json!({})).await.status, 204);
    let r = h.get("/api/meta").await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (403, Some("unknown role"))
    );
}

/// Keeps the session check in `guard::session` equal to `access` in
/// spec/sessions.bend, whose laws (LAWS.bend) are proven by `bend PROOF.bend`.
#[tokio::test]
async fn session_access_matches_spec() {
    let toml = |url: &str| {
        format!(
            "public_url = \"{url}\"\n{}[demo]\nrole = \"viewer\"\n{VIEWER}",
            local_login(None)
        )
    };
    let lab = Homie::start(toml).await;
    let gone = Homie::start_with(toml, |state| state.lab.roles.clear()).await;
    #[derive(Debug, PartialEq)]
    enum Access {
        Full,
        Scoped,
        Refused,
    }
    let model = |logged_in: bool, role: &str| match (logged_in, role) {
        (false, _) => Access::Refused,
        (true, "NoRole") => Access::Full,
        (true, "Known") => Access::Scoped,
        _ => Access::Refused,
    };
    let seen = |r: super::harness::Answer| match (r.status, r.json["role"].as_str()) {
        (200, None) => Access::Full,
        (200, Some(_)) => Access::Scoped,
        (401 | 403, _) => Access::Refused,
        (s, _) => panic!("unexpected {s}: {}", r.text),
    };
    for (h, role, login) in [
        (&lab, "NoRole", "/auth/local"),
        (&lab, "Known", "/auth/demo"),
        (&gone, "NoRole", "/auth/local"),
        (&gone, "Gone", "/auth/demo"),
    ] {
        h.set_cookie(None);
        assert_eq!(
            seen(h.get("/api/meta").await),
            model(false, role),
            "{role} logged out"
        );
        let status = if login == "/auth/local" {
            h.login().await.status
        } else {
            h.post(login, json!({})).await.status
        };
        assert_eq!(status, 204, "{role} login");
        assert_eq!(seen(h.get("/api/meta").await), model(true, role), "{role}");
    }
}

#[tokio::test]
async fn oidc_login_allowlists_and_roles() {
    let backend = backend().await;
    let op = provider(json!({"email": "Me@Example.com"})).await;
    let kc = super::harness::kubeconfig(&backend);
    let h = Homie::start(|url| {
        format!(
            "public_url = \"{url}\"\n{}\n[roles.viewer]\npages = [\"infra\"]\n\
             [kubernetes.user]\nauth = \"forward_token\"\nkubeconfig = \"{}\"\n{}",
            op.section(),
            kc.display(),
            local_login(None)
        )
    })
    .await;
    assert_eq!(
        h.get("/auth/options").await.json,
        json!({"oidc": true, "local": true, "demo": false})
    );

    let r = oidc_login(&h).await;
    assert_eq!(r.status, 303, "{}", r.text);
    assert_eq!(r.headers["location"], "/");
    let m = h.get("/api/meta").await.json;
    assert_eq!(
        m["user"], "me@example.com",
        "emails are compared lowercased"
    );
    assert_eq!(m["kubernetes"][0]["forward_token"], true);
    // the user's own token reaches the cluster
    let k8s = h.get("/api/q/k8s/user").await;
    assert_eq!(
        (k8s.status, &k8s.json["error"]),
        (200, &Value::Null),
        "{}",
        k8s.text
    );
    let r = h
        .post(
            "/api/act/flux/user/reconcile",
            json!({"kind": "Kustomization", "namespace": "apps", "name": "web"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);

    // allowed through a group
    h.set_cookie(None);
    op.set(|s| s.claims = json!({"email": "ops@example.com", "groups": ["admins"]}));
    assert_eq!(oidc_login(&h).await.status, 303);
    assert_eq!(h.get("/api/meta").await.json["user"], "ops@example.com");

    // neither listed nor in a group
    h.set_cookie(None);
    op.set(|s| s.claims = json!({"email": "no@example.com", "groups": ["users"]}));
    let r = oidc_login(&h).await;
    assert_eq!(r.status, 403);
    assert!(
        r.text.contains("no@example.com is not allowed"),
        "{}",
        r.text
    );
    assert_eq!(h.get("/api/meta").await.status, 401);

    // a role by email
    op.set(|s| s.claims = json!({"email": "viewer@example.com"}));
    assert_eq!(oidc_login(&h).await.status, 303);
    assert_eq!(h.get("/api/meta").await.json["role"], "viewer");

    // no ID token in the answer
    h.set_cookie(None);
    op.set(|s| s.id_token = false);
    let r = oidc_login(&h).await;
    assert!(r.text.contains("returned no ID token"), "{}", r.text);
    op.set(|s| s.id_token = true);

    // an unknown state, and a code the provider refuses
    let r = h.get("/auth/callback?code=good&state=forged").await;
    assert!(r.text.contains("login expired or unknown"), "{}", r.text);
    let start = h.get("/auth/login").await;
    let authorize = url::Url::parse(start.headers["location"].to_str().unwrap()).unwrap();
    let state = authorize
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    assert!(
        authorize
            .query_pairs()
            .any(|(k, v)| k == "scope" && v.contains("offline_access"))
    );
    let r = h
        .get(&format!("/auth/callback?code=bad&state={state}"))
        .await;
    assert!(r.text.contains("token exchange"), "{}", r.text);

    // a password session has no ID token to act with on a forward_token cluster
    assert_eq!(h.login().await.status, 204);
    for (path, body) in [
        (
            "/api/act/flux/user/reconcile",
            json!({"kind": "Kustomization", "namespace": "apps", "name": "web"}),
        ),
        (
            "/api/act/argocd/user/refresh",
            json!({"namespace": "argocd", "name": "guestbook"}),
        ),
    ] {
        let r = h.post(path, body).await;
        assert_eq!(r.status, 400, "{path}");
        assert!(
            r.json["error"]
                .as_str()
                .unwrap()
                .contains("no user session"),
            "{}",
            r.text
        );
    }
}

#[tokio::test]
async fn oidc_sessions_refresh_the_id_token() {
    let op = provider(json!({"email": "me@example.com"})).await;
    let h = Homie::start(|url| {
        format!(
            "public_url = \"{url}\"\n{}\n[roles.viewer]\npages = [\"infra\"]\n",
            op.section()
        )
    })
    .await;
    // tokens close to expiry are refreshed on the next request
    op.set(|s| s.exp_in = 30);
    assert_eq!(oidc_login(&h).await.status, 303);
    assert_eq!(h.get("/api/meta").await.status, 200);
    assert_eq!(op.state.lock().unwrap().refreshes, 1);
    // a failed refresh keeps the session while the token is still valid
    op.set(|s| s.refresh_ok = false);
    assert_eq!(h.get("/api/meta").await.status, 200);
    op.set(|s| {
        s.refresh_ok = true;
        s.id_token = false;
    });
    assert_eq!(h.get("/api/meta").await.status, 200);
    assert_eq!(op.state.lock().unwrap().refreshes, 2);
}

#[tokio::test]
async fn live_stream_sends_changed_envelopes() {
    let h = open_homie().await;
    let mut resp = h
        .req(Method::GET, "/api/stream")
        .query(&[("q", "pve/lab,k8s/main,flux/main,argocd/main,bogus/x")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    assert_eq!(resp.headers()["x-accel-buffering"], "no");
    let (seen, _) = read_stream(&mut resp, 15, |s| {
        ["pve/lab", "k8s/main", "flux/main", "argocd/main"]
            .iter()
            .all(|k| s.contains(&format!("\"key\":\"{k}\"")))
    })
    .await;
    assert!(!seen.contains("bogus/x"));
    // an invalidated query is sent again; unchanged ones are not
    h.state.cache.invalidate("pve:");
    let (again, _) = read_stream(&mut resp, 15, |s| s.contains("pve/lab")).await;
    assert!(!again.contains("argocd/main"), "{again}");
    drop(resp);

    // missing CRDs were remembered: a later stream does not watch them again
    let mut resp = h
        .req(Method::GET, "/api/stream")
        .query(&[("q", "flux/main,k8s/missing")])
        .send()
        .await
        .unwrap();
    let (seen, _) = read_stream(&mut resp, 15, |s| s.contains("flux/main")).await;
    assert!(!seen.contains("k8s/missing"));

    let off = Homie::start(|_| "live_updates = false".into()).await;
    let r = off.get("/api/stream?q=pve/lab").await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn live_stream_ends_with_the_session() {
    let h = Homie::start(|url| {
        format!(
            "public_url = \"{url}\"\nrefresh_seconds = 5\n{}",
            local_login(None)
        )
    })
    .await;
    assert_eq!(h.login().await.status, 204);
    let mut resp = h
        .req(Method::GET, "/api/stream?q=pve/lab")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(h.post("/auth/logout", json!({})).await.status, 204);
    h.state.cache.invalidate("wake");
    let (_, ended) = read_stream(&mut resp, 15, |_| false).await;
    assert!(ended);
}

#[tokio::test]
async fn github_picker() {
    let backend = backend().await;
    let dir = temp_dir("picker");
    let h = Homie::start(|_| {
        format!(
            "allow_unauthenticated = true\nstate_dir = \"{}\"\n[github]\ntoken = \"ghp_x\"\n\
             client_id = \"Iv1.x\"\nclient_secret = \"s\"\napi_url = \"{backend}\"\nstore = \"file\"\n",
            dir.display()
        )
    })
    .await;
    let save = |repo: &str, flows: &[&str]| json!({"repo": repo, "workflows": flows});
    let r = h.get("/api/github/repos").await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (428, Some("connect GitHub first"))
    );
    assert_eq!(
        h.get("/api/github/workflows?repo=acme/app").await.status,
        428
    );
    assert_eq!(
        h.post("/api/github/watch", save("acme/app", &["ci.yml"]))
            .await
            .status,
        428
    );

    let r = h.get("/github/login").await;
    assert_eq!(r.status, 303);
    let to = r.headers["location"].to_str().unwrap();
    assert!(
        to.starts_with("https://github.com/login/oauth/authorize?client_id=Iv1.x"),
        "{to}"
    );
    assert!(
        to.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A7777%2Fgithub%2Fcallback"),
        "{to}"
    );
    assert_eq!(h.get("/github/callback?code=x").await.status, 400);
    let r = h.get("/github/callback?code=x&state=forged").await;
    assert_eq!(r.status, 403);

    // as if the sign-in had finished
    let sign_in = |token: &str| {
        h.state
            .lab
            .github
            .as_ref()
            .unwrap()
            .set_user_token("local", token.into())
    };
    sign_in("good");
    assert_eq!(h.get("/api/github/repos").await.json, json!(["acme/app"]));
    let w = h.get("/api/github/workflows?repo=acme/app").await.json;
    assert_eq!(
        w,
        json!([{"name": "CI", "file": "ci.yml", "state": "active"}])
    );
    assert_eq!(h.get("/api/github/workflows?repo=bad").await.status, 400);
    let r = h
        .post("/api/github/watch", save("acme/app", &["nope.yml"]))
        .await;
    assert_eq!(r.status, 400);
    assert!(r.json["error"].as_str().unwrap().contains("nope.yml"));
    assert_eq!(
        h.post("/api/github/watch", save("bad", &[])).await.status,
        400
    );
    let r = h
        .post("/api/github/watch", save("acme/app", &["ci.yml"]))
        .await;
    assert_eq!(
        (r.status, &r.json),
        (200, &json!({"watch": 1})),
        "{}",
        r.text
    );
    assert!(dir.join("github-watch.json").exists());
    let q = h.get("/api/q/github/main").await.json;
    assert_eq!(q["data"][0]["repo"], "acme/app");
    assert_eq!(q["data"][0]["source"], "saved");
    let m = h.get("/api/meta").await.json;
    assert_eq!(
        m["github"],
        json!({"watch": 1, "helper": true, "store": "file", "install_url": null})
    );

    // GitHub refuses the user's token
    sign_in("bad");
    assert_eq!(h.get("/api/github/repos").await.status, 502);
    assert_eq!(
        h.get("/api/github/workflows?repo=acme/app").await.status,
        400
    );
    assert_eq!(
        h.post("/api/github/watch", save("acme/app", &["ci.yml"]))
            .await
            .status,
        403
    );

    // the state directory is gone: saving fails
    std::fs::remove_dir_all(dir).unwrap();
    sign_in("good");
    let r = h.post("/api/github/watch", save("acme/app", &[])).await;
    assert_eq!(r.status, 502, "{}", r.text);

    // saving off; the callback goes to public_url
    let plain = Homie::start(|_| {
        format!(
            "public_url = \"http://homie.example/\"\nallow_unauthenticated = true\n\
             [github]\ntoken = \"ghp_x\"\napi_url = \"{backend}\"\nstore = \"off\"\n\
             client_id = \"Iv1.x\"\nclient_secret = \"s\"\n"
        )
    })
    .await;
    let to = plain.get("/github/login").await.headers["location"].clone();
    assert!(
        to.to_str()
            .unwrap()
            .contains("redirect_uri=http%3A%2F%2Fhomie.example%2Fgithub%2Fcallback"),
        "{to:?}"
    );
    let r = plain
        .post("/api/github/watch", save("acme/app", &["ci.yml"]))
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (409, Some("saving watches is turned off"))
    );

    // a client id without its secret: no helper, and no code exchange
    let half = Homie::start(|_| {
        format!(
            "allow_unauthenticated = true\n[github]\ntoken = \"ghp_x\"\napi_url = \"{backend}\"\n\
             store = \"off\"\nclient_id = \"Iv1.x\"\n"
        )
    })
    .await;
    assert_eq!(half.get("/github/login").await.status, 404);
    half.state
        .github_states
        .lock()
        .unwrap()
        .insert("n1".into(), ("local".into(), std::time::Instant::now()));
    let r = half.get("/github/callback?code=x&state=n1").await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (403, Some("the GitHub sign-in helper is not configured"))
    );

    // no GitHub at all
    let none = open_homie().await;
    for path in [
        "/api/github/repos",
        "/api/github/workflows?repo=a/b",
        "/github/login",
    ] {
        assert_eq!(none.get(path).await.status, 404, "{path}");
    }
    assert_eq!(
        none.post("/api/github/watch", save("a/b", &[]))
            .await
            .status,
        404
    );
    assert_eq!(
        none.get("/github/callback?code=x&state=y").await.headers["location"],
        "/"
    );
}

#[tokio::test]
async fn web_ui_fallback() {
    let h = Homie::start(|_| String::new()).await;
    for path in ["/api/nope", "/auth/nope", "/github/nope"] {
        let r = h.get(path).await;
        assert_eq!((r.status, r.text.as_str()), (404, "not found"), "{path}");
    }
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/dist");
    let built = dist.join("index.html").exists();
    if !built {
        let r = h.get("/").await;
        assert_eq!(r.status, 503);
        assert!(r.text.contains("web UI not built"));
        // debug builds read web/dist from disk: build a tiny one
        std::fs::create_dir_all(dist.join("assets")).unwrap();
        std::fs::write(dist.join("index.html"), "<html>homie</html>").unwrap();
        std::fs::write(dist.join("assets/app-1234.js"), "let x;").unwrap();
    }
    let r = h.get("/infra").await;
    assert_eq!(r.status, 200, "client-side routes get index.html");
    assert_eq!(r.headers["content-type"], "text/html");
    assert_eq!(r.headers["cache-control"], "no-cache");
    let js = std::fs::read_dir(dist.join("assets"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.ends_with(".js"));
    if let Some(js) = js {
        let r = h.get(&format!("/assets/{js}")).await;
        assert_eq!(r.status, 200);
        assert_eq!(
            r.headers["cache-control"],
            "public, max-age=31536000, immutable"
        );
    }
    if !built {
        std::fs::remove_dir_all(dist).unwrap();
    }
}

#[tokio::test]
async fn run_reports_a_busy_port_and_serves() {
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = busy.local_addr().unwrap();
    let cfg = super::harness::config(&format!(
        "listen = \"{addr}\"\nallow_unauthenticated = true"
    ));
    let e = super::run(cfg).await.unwrap_err().to_string();
    assert!(e.starts_with(&format!("listen {addr}")), "{e}");

    drop(busy);
    let cfg = super::harness::config(&format!("listen = \"{addr}\""));
    let server = tokio::spawn(super::run(cfg));
    let url = format!("http://{addr}/healthz");
    let mut answered = false;
    for _ in 0..50 {
        if let Ok(r) = reqwest::get(&url).await {
            assert_eq!(r.text().await.unwrap(), "ok");
            answered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    server.abort();
    assert!(answered);
}

#[tokio::test]
async fn session_watches_are_per_visitor() {
    let backend = backend().await;
    let h = Homie::start(|url| {
        format!(
            "public_url = \"{url}\"\n{}[demo]\nrole = \"ci\"\n\
             [roles.ci]\npages = [\"workflows\"]\n\
             [github]\nclient_id = \"Iv1.x\"\nclient_secret = \"s\"\napi_url = \"{backend}\"\nstore = \"session\"\n",
            local_login(Some("ci")),
        )
    })
    .await;
    let gh = h.state.lab.github.as_ref().unwrap();
    let sid = |c: Option<String>| c.unwrap().split_once('=').unwrap().1.to_string();
    let save = json!({"repo": "acme/app", "workflows": ["ci.yml"]});

    // visitor A: a role, but the picker is open in session mode
    assert_eq!(h.post("/auth/demo", json!({})).await.status, 204);
    let a = h.cookie();
    let meta = h.get("/api/meta").await.json;
    assert_eq!(
        (
            meta["github"]["helper"].clone(),
            meta["github"]["store"].clone()
        ),
        (json!(true), json!("session"))
    );
    assert_eq!(h.get("/api/q/github/_").await.json["data"], json!([]));
    assert_eq!(h.get("/api/github/repos").await.status, 428);
    assert_eq!(h.get("/github/login").await.status, 303);
    gh.set_user_token(&sid(a.clone()), "ta".into());
    assert_eq!(h.get("/api/github/repos").await.json, json!(["acme/app"]));
    let r = h.post("/api/github/watch", save.clone()).await;
    assert_eq!((r.status, r.json["watch"].clone()), (200, json!(1)));
    let flows = h.get("/api/q/github/_").await.json;
    assert_eq!(flows["data"][0]["repo"], "acme/app");
    assert_eq!(flows["data"][0]["source"], "session");

    // visitor B, same user name "demo": sees none of it
    h.set_cookie(None);
    assert_eq!(h.post("/auth/demo", json!({})).await.status, 204);
    assert_eq!(h.get("/api/q/github/_").await.json["data"], json!([]));
    assert_eq!(h.get("/api/github/repos").await.status, 428);
    assert_eq!(h.post("/api/github/watch", save).await.status, 428);

    // A logs out: its token and watches are gone
    h.set_cookie(a.clone());
    assert_eq!(h.post("/auth/logout", json!({})).await.status, 204);
    assert!(gh.user_token(&sid(a)).is_none());
}

/// A Proxmox with one VM on a serial port, one without, one stopped, and a
/// vncwebsocket that checks the token and ticket, then echoes.
async fn console_backend() -> String {
    use axum::{
        Form, Json, Router,
        extract::{Path, Query, WebSocketUpgrade, ws::Message},
        http::HeaderMap,
        response::IntoResponse,
        routing::{get, post},
    };
    use std::collections::HashMap;
    let data = |v: Value| Json(json!({ "data": v }));
    let app = Router::new()
        .route(
            "/api2/json/cluster/resources",
            get(move || async move {
                data(json!([
                    {"type": "qemu", "vmid": 101, "name": "serial", "node": "pve1", "status": "running"},
                    {"type": "qemu", "vmid": 102, "name": "vga", "node": "pve1", "status": "running"},
                    {"type": "qemu", "vmid": 103, "name": "off", "node": "pve1", "status": "stopped"}
                ]))
            }),
        )
        .route(
            "/api2/json/nodes/pve1/qemu/{vmid}/config",
            get(move |Path(vmid): Path<u64>| async move {
                data(if vmid == 101 { json!({"serial0": "socket"}) } else { json!({}) })
            }),
        )
        .route(
            "/api2/json/nodes/pve1/qemu/{vmid}/termproxy",
            post(move |Form(f): Form<HashMap<String, String>>| async move {
                assert_eq!(f["serial"], "serial0");
                data(json!({"port": "5901", "ticket": "PVEVNC:term", "user": "homie@pve!t"}))
            }),
        )
        .route(
            "/api2/json/nodes/pve1/qemu/{vmid}/vncproxy",
            post(move |Form(f): Form<HashMap<String, String>>| async move {
                assert_eq!((f["websocket"].as_str(), f["generate-password"].as_str()), ("1", "1"));
                data(json!({"port": "5902", "ticket": "PVEVNC:vnc", "user": "homie@pve!t", "password": "onetime"}))
            }),
        )
        .route(
            "/api2/json/nodes/pve1/qemu/{vmid}/vncwebsocket",
            get(
                |h: HeaderMap, Query(q): Query<HashMap<String, String>>, ws: WebSocketUpgrade| async move {
                    assert_eq!(h["authorization"], "PVEAPIToken=homie@pve!t=secret");
                    let term = q["vncticket"] == "PVEVNC:term";
                    assert_eq!(q["port"], if term { "5901" } else { "5902" });
                    ws.protocols(["binary"]).on_upgrade(move |mut s| async move {
                        if term {
                            let Some(Ok(Message::Text(login))) = s.recv().await else { return };
                            assert_eq!(login.as_str(), "homie@pve!t:PVEVNC:term\n");
                            let _ = s.send(Message::Text("OK".into())).await;
                        }
                        while let Some(Ok(m)) = s.recv().await {
                            if matches!(m, Message::Close(_)) || s.send(m).await.is_err() {
                                break;
                            }
                        }
                    })
                    .into_response()
                },
            ),
        );
    crate::testkit::serve(app).await
}

#[tokio::test]
async fn consoles_open_once_and_relay() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    let pve = console_backend().await;
    let h = Homie::start(|_| {
        format!(
            "public_url = \"http://homie.example\"\n{}\n\
             [proxmox.lab]\nendpoints = [\"{pve}\"]\ntoken = \"homie@pve!t=secret\"\nconsole = true\n\
             [proxmox.off]\nendpoints = [\"{pve}\"]\ntoken = \"homie@pve!t=secret\"\n",
            local_login(None)
        )
    })
    .await;
    let open = |vmid: u64, mode: Option<&str>| json!({"node": "pve1", "vmid": vmid, "type": "qemu", "mode": mode});
    assert_eq!(
        h.post("/api/act/pve/lab/console", open(101, None))
            .await
            .status,
        401
    );
    assert_eq!(h.login().await.status, 204);

    let r = h.post("/api/act/pve/off/console", open(101, None)).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (403, Some("consoles are off for this Proxmox"))
    );
    let r = h.post("/api/act/pve/lab/console", open(103, None)).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (400, Some("off is not running"))
    );
    let r = h
        .post("/api/act/pve/lab/console", open(102, Some("term")))
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (400, Some("this VM has no serial0 port"))
    );

    let ws = |id: &str, origin: &str, cookie: Option<String>| {
        let url = format!(
            "{}/api/act/pve/lab/console/{id}",
            h.url.replace("http", "ws")
        );
        let mut req = url.into_client_request().unwrap();
        req.headers_mut().insert("origin", origin.parse().unwrap());
        req.headers_mut()
            .insert("sec-websocket-protocol", "binary".parse().unwrap());
        if let Some(c) = cookie {
            req.headers_mut().insert("cookie", c.parse().unwrap());
        }
        let addr = h.url.trim_start_matches("http://").to_string();
        async move {
            let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
            tokio_tungstenite::client_async(req, tcp).await
        }
    };
    let refused = |e: tokio_tungstenite::tungstenite::Error| match e {
        tokio_tungstenite::tungstenite::Error::Http(r) => r.status().as_u16(),
        e => panic!("{e}"),
    };

    // serial VM: a terminal, the ticket never in the answer
    let r = h.post("/api/act/pve/lab/console", open(101, None)).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(
        (r.json["mode"].as_str(), r.json["switchable"].as_bool()),
        (Some("term"), Some(true))
    );
    assert!(r.json["password"].is_null() && !r.text.contains("PVEVNC"));
    let id = r.json["id"].as_str().unwrap().to_string();
    let e = ws(&id, "http://evil.example", h.cookie())
        .await
        .err()
        .unwrap();
    assert_eq!(refused(e), 403);
    let e = ws(&id, "http://homie.example", None).await.err().unwrap();
    assert_eq!(refused(e), 401, "no session");
    let (mut s, _) = ws(&id, "http://homie.example", h.cookie()).await.unwrap();
    s.send(Message::text("0:2:hi")).await.unwrap();
    assert_eq!(s.next().await.unwrap().unwrap(), Message::text("0:2:hi"));
    drop(s);
    let e = ws(&id, "http://homie.example", h.cookie())
        .await
        .err()
        .unwrap();
    assert_eq!(refused(e), 403, "an id works once");

    // VNC on request: a one-time VNC password for noVNC, binary frames relayed
    let r = h
        .post("/api/act/pve/lab/console", open(101, Some("vnc")))
        .await;
    assert_eq!(
        (r.json["mode"].as_str(), r.json["password"].as_str()),
        (Some("vnc"), Some("onetime"))
    );
    let id = r.json["id"].as_str().unwrap().to_string();
    let (mut s, resp) = ws(&id, "http://homie.example", h.cookie()).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "binary");
    s.send(Message::binary(vec![1, 2, 3])).await.unwrap();
    assert_eq!(
        s.next().await.unwrap().unwrap(),
        Message::binary(vec![1, 2, 3])
    );

    // another session cannot take an id
    let r = h.post("/api/act/pve/lab/console", open(102, None)).await;
    assert_eq!(r.json["mode"], "vnc");
    let id = r.json["id"].as_str().unwrap().to_string();
    let mine = h.cookie();
    h.set_cookie(None);
    assert_eq!(h.login().await.status, 204);
    assert_ne!(h.cookie(), mine);
    let e = ws(&id, "http://homie.example", h.cookie())
        .await
        .err()
        .unwrap();
    assert_eq!(refused(e), 403);
}

#[tokio::test]
async fn metrics_listener_token_and_cards() {
    let backend = backend().await;
    let h = Homie::start(|_| {
        format!(
            "allow_unauthenticated = true\n[metrics]\nlisten = \"127.0.0.1:0\"\ntoken = \"s3cret\"\n{}",
            lab_sections(&backend)
        )
    })
    .await;
    // the main listener has no /metrics
    assert!(!h.get("/metrics").await.text.contains("homie_build_info"));

    let serve = |state: super::Shared, token: Option<&str>| {
        let token = token.map(String::from);
        async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/metrics", l.local_addr().unwrap());
            let app = super::metrics_router(state, token);
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            url
        }
    };
    let url = serve(h.state.clone(), Some("s3cret")).await;
    let http = reqwest::Client::new();
    assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
    let wrong = http.get(&url).bearer_auth("s3cre").send().await.unwrap();
    assert_eq!(wrong.status(), 401);
    let r = http.get(&url).bearer_auth("s3cret").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/plain; version=0.0.4");
    let page = r.text().await.unwrap();
    assert!(page.contains("homie_build_info{version=\""), "{page}");
    for status in ["ok", "warn", "bad", "quiet"] {
        let s = format!("homie_card_status{{card=\"pve\",name=\"lab\",status=\"{status}\"}}");
        assert!(page.contains(&s), "{s} missing in\n{page}");
    }
    assert!(page.contains("homie_query_total{kind=\"pve\",name=\"lab\",result=\"ok\"}"));
    // no addresses or emails, whatever the backends answered
    assert!(!page.contains("127.0.0.1") && !page.contains('@'), "{page}");

    // cards = false: only what already happened, no backend call
    let h = Homie::start(|_| {
        format!(
            "allow_unauthenticated = true\n[metrics]\nlisten = \"127.0.0.1:0\"\ncards = false\n{}",
            lab_sections(&backend)
        )
    })
    .await;
    let url = serve(h.state.clone(), None).await;
    let page = http.get(&url).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("homie_sessions_active 0"), "{page}");
    assert!(!page.contains("homie_card_status"), "{page}");
}

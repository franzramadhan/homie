//! Test harness for the server: a Homie on a random port with a client that
//! keeps its session cookie, one fake backend that answers like Proxmox,
//! Grafana, OpenBao, healthchecks.io, the GitHub API and a Kubernetes API
//! server, and a fake OIDC provider signing ID tokens with the RSA test key.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Form, Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use figment::{
    Figment,
    providers::{Format, Toml},
};
use serde_json::{Value, json};

use super::{AppState, Shared, router};
use crate::{auth::Auth, cache::Cache, config::Config, integrations::Lab, testkit::serve};

pub const PASSWORD: &str = "correct-horse-battery";

/// The argon2 hash of `PASSWORD`, computed once (argon2 is slow in debug builds).
pub fn password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| crate::auth::local::hash(PASSWORD).unwrap())
}

/// Tests set no tracing subscriber, so events fall back to the `log`
/// facade. A logger that takes every record (and drops it) makes the
/// fields of log lines evaluate, as they do in production.
pub fn logs() {
    struct Drop;
    impl tracing::log::Log for Drop {
        fn enabled(&self, _: &tracing::log::Metadata) -> bool {
            true
        }
        fn log(&self, _: &tracing::log::Record) {}
        fn flush(&self) {}
    }
    if tracing::log::set_logger(&Drop).is_ok() {
        tracing::log::set_max_level(tracing::log::LevelFilter::Trace);
    }
}

/// Parses and validates a TOML config, as `config::load` would.
pub fn config(toml: &str) -> Config {
    logs();
    let cfg: Config = Figment::from(Toml::string(toml))
        .extract()
        .unwrap_or_else(|e| panic!("{e}\n{toml}"));
    crate::config::validate(&cfg).unwrap_or_else(|e| panic!("{e}\n{toml}"));
    cfg
}

/// A fresh directory under the system temp dir.
pub fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "homie-{tag}-{}-{}",
        std::process::id(),
        openidconnect::CsrfToken::new_random().secret()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A kubeconfig file for a fake API server at `url`.
pub fn kubeconfig(url: &str) -> std::path::PathBuf {
    let path = temp_dir("kube").join("config");
    std::fs::write(
        &path,
        format!(
            "apiVersion: v1\nkind: Config\ncurrent-context: fake\n\
             clusters: [{{name: fake, cluster: {{server: \"{url}\"}}}}]\n\
             users: [{{name: fake, user: {{token: fake}}}}]\n\
             contexts: [{{name: fake, context: {{cluster: fake, user: fake}}}}]\n"
        ),
    )
    .unwrap();
    path
}

/// Config sections for every integration, all pointing at the fake backend.
pub fn lab_sections(backend: &str) -> String {
    let kc = kubeconfig(backend);
    format!(
        r#"
[proxmox.lab]
endpoints = ["{backend}"]
token = "homie@pve!t=secret"
protected_tags = ["prod"]

[kubernetes.main]
auth = "kubeconfig"
kubeconfig = "{kc}"
headlamp_url = "https://headlamp.example.com"
argocd_url = "https://argocd.example.com"
metrics = {{ grafana = "mon", datasource = "prom", cpu = "cpu", memory = "mem" }}

[grafana.mon]
url = "{backend}"
token = "glsa_x"

[openbao.vault]
url = "{backend}"

[healthchecks]
url = "{backend}"
api_key = "hc"

[security.kyverno]
kubernetes = "main"
[security.tetragon]
grafana = "mon"
datasource = "vlogs"
[security.wazuh]
grafana = "mon"
datasource = "vlogs"
[security.debsecan]
grafana = "mon"
datasource = "prom"

[[logs.datasources]]
label = "Loki"
grafana = "mon"
datasource = "loki"
type = "loki"
"#,
        kc = kc.display()
    )
}

fn data(v: Value) -> Json<Value> {
    Json(json!({ "data": v }))
}

fn k8s_status(code: u16, reason: &str) -> Response {
    let body = json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
        "message": reason, "reason": reason, "code": code
    });
    (StatusCode::from_u16(code).unwrap(), Json(body)).into_response()
}

/// Kubernetes catch-all: lists are empty, watches are 404 (an absent CRD)
/// except the first Pod watch, which sends one event, patches answer with
/// the object. Flux sources are not installed.
async fn kube_fallback(req: Request) -> Response {
    static POD_EVENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    let path = req.uri().path().to_string();
    let watch = req.uri().query().is_some_and(|q| q.contains("watch="));
    let kube = path.starts_with("/api/v1/") || path.starts_with("/apis/");
    if path.starts_with("/apis/source.toolkit.fluxcd.io/") {
        return k8s_status(404, "NotFound");
    }
    if watch && path == "/api/v1/pods" && POD_EVENT.swap(false, std::sync::atomic::Ordering::SeqCst)
    {
        let event = json!({"type": "ADDED", "object": {
            "apiVersion": "meta.k8s.io/v1", "kind": "PartialObjectMetadata",
            "metadata": {"name": "web-1", "namespace": "apps", "resourceVersion": "2"}
        }});
        return format!("{event}\n").into_response();
    }
    match (req.method().clone(), kube) {
        (_, false) => StatusCode::NOT_FOUND.into_response(),
        (Method::PATCH, true) => Json(json!({
            "apiVersion": "v1", "kind": "Object",
            "metadata": { "name": path.rsplit('/').next(), "namespace": "apps" }
        }))
        .into_response(),
        (Method::GET, true) if !watch => Json(json!({
            "kind": "List", "apiVersion": "v1", "metadata": {}, "items": []
        }))
        .into_response(),
        _ => k8s_status(404, "NotFound"),
    }
}

/// GitHub: the user token "bad" is refused.
fn github(h: &HeaderMap, body: Value) -> Response {
    match h.get("authorization").and_then(|v| v.to_str().ok()) {
        Some("Bearer bad") => (StatusCode::UNAUTHORIZED, "Bad credentials").into_response(),
        _ => Json(body).into_response(),
    }
}

async fn grafana_proxy(Path((_, rest)): Path<(String, String)>) -> Response {
    if rest.contains("select/logsql") {
        return String::new().into_response();
    }
    Json(json!({ "status": "success", "data": { "resultType": "matrix", "result": [] } }))
        .into_response()
}

/// One fake server for every HTTP integration and the Kubernetes API.
pub async fn backend() -> String {
    logs();
    let app = Router::new()
        // Proxmox
        .route(
            "/api2/json/cluster/status",
            get(|| async {
                data(json!([
                    {"type": "cluster", "name": "lab", "quorate": 1},
                    {"type": "node", "name": "pve1", "online": 1}
                ]))
            }),
        )
        .route(
            "/api2/json/cluster/resources",
            get(|| async {
                data(json!([
                    {"type": "node", "node": "pve1", "status": "online", "maxcpu": 4},
                    {"type": "qemu", "vmid": 101, "name": "web", "node": "pve1", "status": "running", "tags": "demo"},
                    {"type": "qemu", "vmid": 102, "name": "db", "node": "pve1", "status": "running", "tags": "prod"}
                ]))
            }),
        )
        .route(
            "/api2/json/cluster/ha/status/current",
            get(|| async { data(json!([])) }),
        )
        .route(
            "/api2/json/access/permissions",
            get(|| async { data(json!({"/": {"VM.PowerMgmt": 1, "Sys.Audit": 1}})) }),
        )
        .route(
            "/api2/json/version",
            get(|| async { data(json!({"version": "8.2.4"})) }),
        )
        .route(
            "/api2/json/nodes/{node}/tasks",
            get(|| async { data(json!([])) }),
        )
        .route(
            "/api2/json/nodes/{node}/tasks/{upid}/status",
            get(|| async { data(json!({"status": "stopped", "exitstatus": "OK"})) }),
        )
        .route(
            "/api2/json/nodes/{node}/rrddata",
            get(|| async { data(json!([])) }),
        )
        .route(
            "/api2/json/nodes/{node}/{kind}/{vmid}/status/{action}",
            post(|| async { data(json!("UPID:pve1:00001234:start:101:homie@pve!t:")) }),
        )
        // Grafana
        .route(
            "/api/health",
            get(|| async { Json(json!({"version": "11.2.0"})) }),
        )
        .route("/api/search", get(|| async { Json(json!([])) }))
        .route(
            "/api/prometheus/grafana/api/v1/alerts",
            get(|| async { Json(json!({"data": {"alerts": []}})) }),
        )
        .route(
            "/api/datasources/proxy/uid/{ds}/{*rest}",
            get(grafana_proxy),
        )
        // OpenBao and healthchecks.io
        .route(
            "/v1/sys/health",
            get(|| async {
                Json(json!({"initialized": true, "sealed": false, "standby": false, "version": "2.1.0"}))
            }),
        )
        .route(
            "/api/v3/checks/",
            get(|| async { Json(json!({"checks": []})) }),
        )
        // GitHub
        .route(
            "/user/installations",
            get(|h: HeaderMap| async move { github(&h, json!({"installations": [{"id": 7}]})) }),
        )
        .route(
            "/user/installations/{id}/repositories",
            get(|| async { Json(json!({"repositories": [{"full_name": "acme/app"}]})) }),
        )
        .route(
            "/repos/{owner}/{repo}/actions/workflows",
            get(|h: HeaderMap| async move {
                let flow = json!({"name": "CI", "path": ".github/workflows/ci.yml", "state": "active"});
                github(&h, json!({ "workflows": [flow] }))
            }),
        )
        .route(
            "/repos/{owner}/{repo}/actions/workflows/{wf}",
            get(|Path(p): Path<HashMap<String, String>>| async move {
                match p["wf"].as_str() {
                    "gone.yml" => StatusCode::NOT_FOUND.into_response(),
                    _ => Json(json!({"name": "CI"})).into_response(),
                }
            }),
        )
        .route(
            "/repos/{owner}/{repo}/actions/workflows/{wf}/runs",
            get(|| async { Json(json!({"workflow_runs": []})) }),
        )
        // Kubernetes
        .route(
            "/version",
            get(|| async {
                Json(json!({
                    "major": "1", "minor": "32", "gitVersion": "v1.32.0", "gitCommit": "abc",
                    "gitTreeState": "clean", "buildDate": "2026-01-01T00:00:00Z",
                    "goVersion": "go1.23", "compiler": "gc", "platform": "linux/amd64"
                }))
            }),
        )
        .fallback(kube_fallback);
    serve(app).await
}

/// A Homie under test and a client that keeps its session cookie.
pub struct Homie {
    pub url: String,
    pub state: Shared,
    http: reqwest::Client,
    cookie: Mutex<Option<String>>,
}

/// A response: status, headers and the body as JSON (`Null` when it is not).
pub struct Answer {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub text: String,
    pub json: Value,
}

impl Homie {
    /// Starts Homie with the config `toml(url)`, `url` being its own address.
    pub async fn start(toml: impl FnOnce(&str) -> String) -> Self {
        Self::start_with(toml, |_| {}).await
    }

    /// Like `start`, with a chance to change the state before serving.
    pub async fn start_with(
        toml: impl FnOnce(&str) -> String,
        tweak: impl FnOnce(&mut AppState),
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let cfg = config(&toml(&url));
        let mut state = AppState {
            lab: Lab::new(&cfg).await.unwrap(),
            cache: Cache::new(None).unwrap(),
            auth: Auth::new(&cfg).await.unwrap(),
            cfg,
            github_users: Default::default(),
            github_states: Default::default(),
        };
        tweak(&mut state);
        let state = Arc::new(state);
        let app = router(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            url,
            state,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cookie: Mutex::new(None),
        }
    }

    /// A request with the session cookie and the X-Homie header.
    pub fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.url)
        };
        let mut r = self.http.request(method, url).header("x-homie", "1");
        if let Some(c) = self.cookie.lock().unwrap().clone() {
            r = r.header("cookie", c);
        }
        r
    }

    /// A request with neither the cookie nor the X-Homie header.
    pub fn bare(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, format!("{}{path}", self.url))
    }

    /// Sends `r`, keeping a Set-Cookie for later requests.
    pub async fn send(&self, r: reqwest::RequestBuilder) -> Answer {
        let resp = r.send().await.unwrap();
        if let Some(c) = resp.headers().get("set-cookie") {
            let pair = c.to_str().unwrap().split(';').next().unwrap().to_string();
            *self.cookie.lock().unwrap() = Some(pair);
        }
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap();
        let json = serde_json::from_str(&text).unwrap_or(Value::Null);
        Answer {
            status,
            headers,
            text,
            json,
        }
    }

    pub async fn get(&self, path: &str) -> Answer {
        self.send(self.req(reqwest::Method::GET, path)).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Answer {
        self.send(self.req(reqwest::Method::POST, path).json(&body))
            .await
    }

    pub fn set_cookie(&self, c: Option<String>) {
        *self.cookie.lock().unwrap() = c;
    }

    pub fn cookie(&self) -> Option<String> {
        self.cookie.lock().unwrap().clone()
    }

    pub async fn login(&self) -> Answer {
        self.post(
            "/auth/local",
            json!({"username": "homie", "password": PASSWORD}),
        )
        .await
    }
}

// ---- fake OIDC provider ----

pub const CLIENT_ID: &str = "homie";

/// What the provider puts in the next tokens, and what it saw.
pub struct OpState {
    pub claims: Value,
    /// seconds until the ID token expires
    pub exp_in: i64,
    pub refresh_ok: bool,
    pub id_token: bool,
    pub refreshes: usize,
    nonce: Option<String>,
}

pub struct Provider {
    pub url: String,
    pub state: Arc<Mutex<OpState>>,
}

fn rsa_key() -> ring::rsa::KeyPair {
    let pem = include_str!("../../tests/fixtures/test-rsa.pem");
    let b64: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    ring::rsa::KeyPair::from_der(&STANDARD.decode(b64).unwrap()).unwrap()
}

/// An RS256 JWT over `claims`.
pub fn sign(claims: &Value) -> String {
    let key = rsa_key();
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT","kid":"k1"}"#);
    let input = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
    let mut sig = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig))
}

fn jwk() -> Value {
    let key = rsa_key();
    let c = ring::rsa::PublicKeyComponents::<Vec<u8>>::from(key.public());
    json!({"kty": "RSA", "use": "sig", "alg": "RS256", "kid": "k1",
           "n": URL_SAFE_NO_PAD.encode(&c.n), "e": URL_SAFE_NO_PAD.encode(&c.e)})
}

type Op = (String, Arc<Mutex<OpState>>);

async fn op_token(State((url, st)): State<Op>, Form(f): Form<HashMap<String, String>>) -> Response {
    let mut s = st.lock().unwrap();
    let now = jiff::Timestamp::now().as_second();
    let mut claims = json!({"iss": url, "sub": "user-1", "aud": CLIENT_ID,
                            "iat": now, "exp": now + s.exp_in});
    for (k, v) in s.claims.as_object().into_iter().flatten() {
        claims[k] = v.clone();
    }
    let mut body = json!({"access_token": "at", "token_type": "Bearer", "expires_in": 300});
    match f.get("grant_type").map(String::as_str) {
        Some("authorization_code") if f.get("code").map(String::as_str) == Some("good") => {
            claims["nonce"] = json!(s.nonce);
            body["refresh_token"] = json!("r1");
        }
        Some("refresh_token") if s.refresh_ok => s.refreshes += 1,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    }
    if s.id_token {
        body["id_token"] = json!(sign(&claims));
    }
    Json(body).into_response()
}

/// Starts the provider. Every token carries `claims` on top of the standard ones.
pub async fn provider(claims: Value) -> Provider {
    logs();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(OpState {
        claims,
        exp_in: 600,
        refresh_ok: true,
        id_token: true,
        refreshes: 0,
        nonce: None,
    }));
    let discovery = json!({
        "issuer": url,
        "authorization_endpoint": format!("{url}/authorize"),
        "token_endpoint": format!("{url}/token"),
        "jwks_uri": format!("{url}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    });
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || async move { Json(discovery) }),
        )
        .route("/jwks", get(|| async { Json(json!({ "keys": [jwk()] })) }))
        .route(
            "/authorize",
            get(
                |State((_, st)): State<Op>, Query(q): Query<HashMap<String, String>>| async move {
                    st.lock().unwrap().nonce = q.get("nonce").cloned();
                    Redirect::to(&format!(
                        "{}?code=good&state={}",
                        q["redirect_uri"], q["state"]
                    ))
                },
            ),
        )
        .route("/token", post(op_token))
        .with_state((url.clone(), state.clone()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Provider { url, state }
}

impl Provider {
    /// The `[auth]` section for this provider.
    pub fn section(&self) -> String {
        format!(
            r#"
[auth]
issuer = "{}"
client_id = "{CLIENT_ID}"
client_secret = "shh"
groups_claim = "groups"
allowed_emails = ["me@example.com", "viewer@example.com"]
allowed_groups = ["admins"]
[auth.roles]
"viewer@example.com" = "viewer"
"#,
            self.url
        )
    }

    pub fn set(&self, f: impl FnOnce(&mut OpState)) {
        f(&mut self.state.lock().unwrap());
    }
}

/// Runs the browser side of an OIDC login on `homie`: /auth/login, the
/// provider's authorize page, then Homie's callback. Returns the callback's answer.
pub async fn oidc_login(homie: &Homie) -> Answer {
    let start = homie.get("/auth/login").await;
    assert_eq!(start.status, 303, "{}", start.text);
    let authorize = start.headers["location"].to_str().unwrap().to_string();
    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(&authorize)
        .send()
        .await
        .unwrap();
    let callback = resp.headers()["location"].to_str().unwrap().to_string();
    homie.get(&callback).await
}

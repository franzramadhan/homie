//! GitHub Actions workflow runs (RFC 0014) for a shared watch list, read
//! with a GitHub App installation token (or a fine-grained token), and the
//! sign-in helper that lists repositories and workflows for picking.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::future::join_all;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    Result, config,
    http::{self, Target},
    secret::SecretCell,
};

pub struct GitHub {
    pub quiet: bool,
    /// from the config file: read-only in the UI
    config_watch: Vec<config::Watch>,
    /// saved by the picker (RFC 0015)
    saved: std::sync::RwLock<Vec<config::Watch>>,
    pub store: super::watch_store::Store,
    pub helper: bool,
    /// the app's install page (RFC 0017)
    pub install_url: Option<String>,
    target: Target,
    cred: Cred,
    client_id: Option<String>,
    client_secret: Option<String>,
    // per repository owner: installation token and its expiry
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    /// per Homie session: its GitHub user token and, with store = "session",
    /// its own watches (RFC 0017)
    personal: std::sync::Mutex<HashMap<String, Personal>>,
}

/// What one Homie session holds: the user token from the GitHub sign-in, and
/// in session mode the workflows that session picked.
#[derive(Default)]
struct Personal {
    token: Option<String>,
    watches: Vec<config::Watch>,
    touched: Option<Instant>,
}

/// Homie sessions last at most this long; older personal entries are dropped.
const PERSONAL_TTL: Duration = Duration::from_secs(12 * 3600);

enum Cred {
    App {
        id: u64,
        key: ring::rsa::KeyPair,
    },
    Token(SecretCell),
    /// store = "session": Homie never reads as the app
    None,
}

#[derive(Serialize)]
pub struct Workflow {
    repo: String,
    workflow: String,
    /// "config" or "saved"
    source: &'static str,
    name: String,
    link: String,
    runs: Vec<Run>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct Run {
    number: u64,
    title: String,
    status: String,
    conclusion: Option<String>,
    event: String,
    branch: String,
    actor: String,
    started: String,
    seconds: Option<i64>,
    sha: String,
    commit_link: String,
    link: String,
}

impl Workflow {
    pub fn label(&self) -> String {
        format!("{}/{}", self.repo, self.workflow)
    }

    pub fn err(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// PEM to DER: the base64 between the BEGIN and END lines.
fn pem_der(pem: &str) -> Result<Vec<u8>> {
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    Ok(base64::engine::general_purpose::STANDARD.decode(body.trim())?)
}

/// GitHub App private keys are PKCS#1 ("RSA PRIVATE KEY"); PKCS#8 works too.
fn load_key(pem: &str) -> Result<ring::rsa::KeyPair> {
    let der = pem_der(pem)?;
    ring::rsa::KeyPair::from_der(&der)
        .or_else(|_| ring::rsa::KeyPair::from_pkcs8(&der))
        .map_err(|e| format!("github.private_key: not an RSA key ({e})").into())
}

/// RS256 JWT for the GitHub App API, valid 9 minutes (GitHub allows 10).
fn app_jwt(key: &ring::rsa::KeyPair, app_id: u64, now: i64) -> Result<String> {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = URL_SAFE_NO_PAD
        .encode(json!({"iat": now - 60, "exp": now + 540, "iss": app_id.to_string()}).to_string());
    let input = format!("{header}.{claims}");
    let mut sig = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .map_err(|_| "signing the GitHub App JWT failed")?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn run(r: &Value, repo_url: &str) -> Run {
    let sha = text(&r["head_sha"]);
    let started = text(&r["run_started_at"]);
    let seconds = (r["status"] == "completed")
        .then(|| {
            let s = started.parse::<jiff::Timestamp>().ok()?;
            let e = r["updated_at"].as_str()?.parse::<jiff::Timestamp>().ok()?;
            Some(e.duration_since(s).as_secs())
        })
        .flatten();
    Run {
        number: r["run_number"].as_u64().unwrap_or(0),
        title: text(&r["display_title"]),
        status: text(&r["status"]),
        conclusion: r["conclusion"].as_str().map(String::from),
        event: text(&r["event"]),
        branch: text(&r["head_branch"]),
        actor: text(&r["actor"]["login"]),
        commit_link: format!("{repo_url}/commit/{sha}"),
        sha: sha.chars().take(7).collect(),
        link: text(&r["html_url"]),
        started,
        seconds,
    }
}

impl GitHub {
    pub async fn new(cfg: &config::GitHub, state_dir: Option<&std::path::Path>) -> Result<Self> {
        let tls = config::Tls {
            tls_verify: true,
            ca_file: None,
            tls_server_name: None,
        };
        let target = http::target(&cfg.api_url, &tls)
            .await
            .map_err(|e| format!("github: {e}"))?;
        let cred = match (&cfg.app_id, &cfg.private_key, &cfg.token) {
            (Some(id), Some(key), _) => Cred::App {
                id: *id,
                key: load_key(&key.resolve().await?)?,
            },
            (_, _, Some(t)) => Cred::Token(SecretCell::new(t).await?),
            _ if cfg.store == config::WatchStore::Session => Cred::None,
            _ => return Err("github: set app_id and private_key, or token".into()),
        };
        let resolve = |s: &Option<crate::secret::Secret>| {
            let s = s.clone();
            async move {
                match s {
                    Some(s) => s.resolve().await.map(Some),
                    None => Ok(None),
                }
            }
        };
        let (client_id, client_secret) = (
            resolve(&cfg.client_id).await?,
            resolve(&cfg.client_secret).await?,
        );
        let store = super::watch_store::Store::new(cfg.store, state_dir)
            .await
            .map_err(|e| format!("github.store: {e}"))?;
        let saved = store
            .load()
            .await
            .map_err(|e| format!("github.store: {e}"))?;
        Ok(Self {
            quiet: cfg.quiet,
            config_watch: cfg.watch.clone(),
            saved: std::sync::RwLock::new(saved),
            store,
            helper: client_id.is_some() && client_secret.is_some(),
            install_url: cfg.install_url.clone(),
            target,
            cred,
            client_id,
            client_secret,
            tokens: Mutex::new(HashMap::new()),
            personal: std::sync::Mutex::new(HashMap::new()),
        })
    }

    async fn request(&self, method: reqwest::Method, path: &str, bearer: &str) -> Result<Value> {
        let resp = self
            .target
            .client
            .request(method, http::join(&self.target.base, path))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(bearer)
            .send()
            .await?;
        if resp.status().as_u16() == 403 || resp.status().as_u16() == 429 {
            let left = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if left == "0" {
                return Err("GitHub rate limit reached; showing the last data".into());
            }
        }
        Ok(http::ok(resp).await?.json().await?)
    }

    /// The token for reading `owner`'s repositories: the fine-grained token,
    /// or a cached installation token of the app.
    async fn token_for(&self, owner: &str) -> Result<String> {
        let (id, key) = match &self.cred {
            Cred::Token(t) => return Ok(t.get().await),
            Cred::App { id, key } => (id, key),
            Cred::None => {
                return Err("this Homie reads GitHub with each visitor's own token".into());
            }
        };
        let mut tokens = self.tokens.lock().await;
        if let Some((t, until)) = tokens.get(owner)
            && Instant::now() < *until
        {
            return Ok(t.clone());
        }
        let jwt = app_jwt(key, *id, jiff::Timestamp::now().as_second())?;
        let inst = self
            .request(
                reqwest::Method::GET,
                &format!("/users/{owner}/installation"),
                &jwt,
            )
            .await;
        let inst = match inst {
            Ok(v) => v,
            // organizations have their own endpoint
            Err(_) => self
                .request(
                    reqwest::Method::GET,
                    &format!("/orgs/{owner}/installation"),
                    &jwt,
                )
                .await
                .map_err(|e| format!("the GitHub App is not installed for {owner}: {e}"))?,
        };
        let inst_id = inst["id"].as_u64().ok_or("installation without id")?;
        let t = self
            .request(
                reqwest::Method::POST,
                &format!("/app/installations/{inst_id}/access_tokens"),
                &jwt,
            )
            .await?;
        let token = t["token"]
            .as_str()
            .ok_or("no installation token")?
            .to_string();
        // tokens live 1 h; renew 5 minutes early
        tokens.insert(
            owner.to_string(),
            (token.clone(), Instant::now() + Duration::from_secs(55 * 60)),
        );
        Ok(token)
    }

    /// Config watches, then saved ones not already in the config.
    pub fn watch(&self) -> Vec<(config::Watch, &'static str)> {
        let mut all: Vec<_> = self
            .config_watch
            .iter()
            .map(|w| (w.clone(), "config"))
            .collect();
        for w in self.saved.read().unwrap().iter() {
            if !self.config_watch.contains(w) {
                all.push((w.clone(), "saved"));
            }
        }
        all
    }

    /// Sets the saved watches of `repo` to `workflows` (config ones are
    /// skipped) and persists the list. Returns (added, removed).
    pub async fn save_repo(
        &self,
        repo: &str,
        workflows: &[String],
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut next: Vec<config::Watch> = self
            .saved
            .read()
            .unwrap()
            .iter()
            .filter(|w| w.repo != repo)
            .cloned()
            .collect();
        let before: Vec<String> = self
            .saved
            .read()
            .unwrap()
            .iter()
            .filter(|w| w.repo == repo)
            .map(|w| w.workflow.clone())
            .collect();
        let mine: Vec<config::Watch> = workflows
            .iter()
            .map(|f| config::Watch {
                repo: repo.to_string(),
                workflow: f.clone(),
            })
            .filter(|w| !self.config_watch.contains(w))
            .collect();
        let added = mine
            .iter()
            .filter(|w| !before.contains(&w.workflow))
            .map(|w| w.workflow.clone())
            .collect();
        let removed = before
            .iter()
            .filter(|f| !mine.iter().any(|w| &w.workflow == *f))
            .cloned()
            .collect();
        next.extend(mine);
        // ponytail: last writer wins between two Homie replicas; Homie runs one (in-memory sessions)
        self.store.save(&next).await?;
        *self.saved.write().unwrap() = next;
        Ok((added, removed))
    }

    /// `user_token`: read as that GitHub user (session mode), else as the app.
    async fn workflow(
        &self,
        w: &config::Watch,
        source: &'static str,
        user_token: Option<&str>,
    ) -> Workflow {
        let (owner, _) = w.repo.split_once('/').unwrap_or((&w.repo, ""));
        let repo_url = format!("https://github.com/{}", w.repo);
        let mut out = Workflow {
            repo: w.repo.clone(),
            workflow: w.workflow.clone(),
            source,
            name: w.workflow.clone(),
            link: format!("{repo_url}/actions/workflows/{}", w.workflow),
            runs: Vec::new(),
            error: None,
        };
        let result: Result<()> = async {
            let token = match user_token {
                Some(t) => t.to_string(),
                None => self.token_for(owner).await?,
            };
            let meta = self
                .request(
                    reqwest::Method::GET,
                    &format!("/repos/{}/actions/workflows/{}", w.repo, w.workflow),
                    &token,
                )
                .await?;
            out.name = meta["name"].as_str().unwrap_or(&w.workflow).to_string();
            let runs = self
                .request(
                    reqwest::Method::GET,
                    &format!(
                        "/repos/{}/actions/workflows/{}/runs?per_page=10",
                        w.repo, w.workflow
                    ),
                    &token,
                )
                .await?;
            out.runs = runs["workflow_runs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| run(r, &repo_url))
                .collect();
            Ok(())
        }
        .await;
        out.error = result.err().map(|e| e.to_string());
        out
    }

    /// Every watched workflow. One failing workflow does not hide the others.
    pub async fn workflows(&self) -> Result<Vec<Workflow>> {
        let watch = self.watch();
        let all = join_all(watch.iter().map(|(w, s)| self.workflow(w, s, None))).await;
        if !all.is_empty() && all.iter().all(|w| w.error.is_some()) {
            return Err(all[0].error.clone().unwrap_or_default().into());
        }
        Ok(all)
    }

    // ---- per session (RFC 0017): the sign-in token and personal watches ----

    pub fn session_mode(&self) -> bool {
        matches!(self.store, super::watch_store::Store::Session)
    }

    fn personal<T>(&self, key: &str, f: impl FnOnce(&mut Personal) -> T) -> T {
        let mut all = self.personal.lock().unwrap();
        all.retain(|_, p| p.touched.is_some_and(|t| t.elapsed() < PERSONAL_TTL));
        let p = all.entry(key.to_string()).or_default();
        p.touched = Some(Instant::now());
        f(p)
    }

    pub fn set_user_token(&self, key: &str, token: String) {
        self.personal(key, |p| p.token = Some(token));
    }

    pub fn user_token(&self, key: &str) -> Option<String> {
        self.personal.lock().unwrap().get(key)?.token.clone()
    }

    /// Drops a session's token and watches (logout).
    pub fn forget(&self, key: &str) {
        self.personal.lock().unwrap().remove(key);
    }

    /// Sets a session's watches for `repo` to `workflows`; returns how many
    /// it watches now.
    pub fn save_session(&self, key: &str, repo: &str, workflows: &[String]) -> usize {
        self.personal(key, |p| {
            p.watches.retain(|w| w.repo != repo);
            p.watches.extend(workflows.iter().map(|f| config::Watch {
                repo: repo.to_string(),
                workflow: f.clone(),
            }));
            p.watches.len()
        })
    }

    /// A session's own workflows, read with its GitHub user token. Empty
    /// until it signs in and picks some.
    pub async fn session_workflows(&self, key: &str) -> Result<Vec<Workflow>> {
        let (token, watches) = match self.personal.lock().unwrap().get(key) {
            Some(p) => (p.token.clone(), p.watches.clone()),
            None => (None, Vec::new()),
        };
        let Some(token) = token.filter(|_| !watches.is_empty()) else {
            return Ok(Vec::new());
        };
        Ok(join_all(
            watches
                .iter()
                .map(|w| self.workflow(w, "session", Some(&token))),
        )
        .await)
    }

    // ---- sign-in helper: pick repositories and workflows ----

    pub fn authorize_url(&self, redirect: &str, state: &str) -> Option<String> {
        let id = self.client_id.as_ref()?;
        let mut u = url::Url::parse("https://github.com/login/oauth/authorize").ok()?;
        u.query_pairs_mut()
            .append_pair("client_id", id)
            .append_pair("redirect_uri", redirect)
            .append_pair("state", state);
        Some(u.to_string())
    }

    pub async fn exchange(&self, code: &str, redirect: &str) -> Result<String> {
        let (Some(id), Some(secret)) = (&self.client_id, &self.client_secret) else {
            return Err("the GitHub sign-in helper is not configured".into());
        };
        let resp = self
            .target
            .client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", id.as_str()),
                ("client_secret", secret),
                ("code", code),
                ("redirect_uri", redirect),
            ])
            .send()
            .await?;
        let body: Value = http::ok(resp).await?.json().await?;
        body["access_token"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                format!(
                    "GitHub sign-in failed: {}",
                    body["error_description"].as_str().unwrap_or("no token")
                )
                .into()
            })
    }

    /// Repositories the signed-in user reaches through the app's installations.
    pub async fn user_repos(&self, user_token: &str) -> Result<Vec<String>> {
        let inst = self
            .request(
                reqwest::Method::GET,
                "/user/installations?per_page=100",
                user_token,
            )
            .await?;
        let mut repos = Vec::new();
        for i in inst["installations"].as_array().into_iter().flatten() {
            let Some(id) = i["id"].as_u64() else { continue };
            let r = self
                .request(
                    reqwest::Method::GET,
                    &format!("/user/installations/{id}/repositories?per_page=100"),
                    user_token,
                )
                .await?;
            repos.extend(
                r["repositories"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x["full_name"].as_str().map(String::from)),
            );
        }
        repos.sort();
        Ok(repos)
    }

    pub async fn user_workflows(&self, user_token: &str, repo: &str) -> Result<Vec<Value>> {
        if !valid_repo(repo) {
            return Err("repository must be owner/name".into());
        }
        let w = self
            .request(
                reqwest::Method::GET,
                &format!("/repos/{repo}/actions/workflows?per_page=100"),
                user_token,
            )
            .await?;
        Ok(w["workflows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| {
                let file = x["path"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                json!({"name": x["name"], "file": file, "state": x["state"]})
            })
            .collect())
    }
}

/// `owner/name` with GitHub's allowed characters: these go into API paths.
pub fn valid_repo(s: &str) -> bool {
    let Some((owner, name)) = s.split_once('/') else {
        return false;
    };
    let owner_ok =
        !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    owner_ok && name_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    // a throwaway 2048-bit test key, never used anywhere else
    const TEST_KEY: &str = include_str!("../../tests/fixtures/test-rsa.pem");

    #[tokio::test]
    async fn session_reads_with_the_visitors_token() {
        use axum::{Json, Router, http::HeaderMap, routing::get};
        // the fake answers only for the visitor's token, never an app token
        let runs = |h: HeaderMap| async move {
            let ok = h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer visitor");
            if ok {
                Json(serde_json::json!({"workflow_runs": [], "name": "CI"})).into_response()
            } else {
                axum::http::StatusCode::UNAUTHORIZED.into_response()
            }
        };
        use axum::response::IntoResponse;
        let url = crate::testkit::serve(
            Router::new()
                .route("/repos/{o}/{r}/actions/workflows/{w}", get(runs))
                .route("/repos/{o}/{r}/actions/workflows/{w}/runs", get(runs)),
        )
        .await;
        let cfg: config::GitHub = serde_json::from_value(serde_json::json!({
            "client_id": "Iv1.x", "client_secret": "s", "api_url": url, "store": "session"
        }))
        .unwrap();
        let gh = GitHub::new(&cfg, None).await.unwrap();
        assert!(gh.session_mode() && gh.store.name() == Some("session"));
        assert!(gh.workflows().await.unwrap().is_empty(), "no shared list");
        assert!(gh.session_workflows("a").await.unwrap().is_empty());

        gh.set_user_token("a", "visitor".into());
        assert_eq!(gh.save_session("a", "o/r", &["ci.yml".into()]), 1);
        let flows = gh.session_workflows("a").await.unwrap();
        assert_eq!(
            (flows.len(), flows[0].source, flows[0].err()),
            (1, "session", None)
        );
        assert_eq!(gh.save_session("a", "o/r", &[]), 0, "unticking removes");

        // another session sees nothing, and an app token is never used
        gh.save_session("b", "o/r", &["ci.yml".into()]);
        assert!(
            gh.session_workflows("b").await.unwrap().is_empty(),
            "no token, no reads"
        );
        assert!(gh.token_for("o").await.is_err());
        gh.forget("a");
        assert!(gh.user_token("a").is_none());
    }

    #[tokio::test]
    async fn saved_watches_merge_with_config() {
        let dir = std::env::temp_dir().join(format!("homie-gh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |f: &str| config::Watch {
            repo: "o/r".into(),
            workflow: f.into(),
        };
        let cfg = config::GitHub {
            app_id: None,
            private_key: None,
            token: Some(crate::secret::Secret::Plain("t".into())),
            client_id: None,
            client_secret: None,
            watch: vec![w("plan.yml")],
            api_url: "https://api.github.com".into(),
            quiet: false,
            store: config::WatchStore::File,

            install_url: None,
        };
        let gh = GitHub::new(&cfg, Some(&dir)).await.unwrap();
        let names = |gh: &GitHub| {
            gh.watch()
                .into_iter()
                .map(|(w, s)| format!("{}:{s}", w.workflow))
                .collect::<Vec<_>>()
        };
        // the config one is not saved again
        let (added, removed) = gh
            .save_repo("o/r", &["plan.yml".into(), "ci.yml".into()])
            .await
            .unwrap();
        assert_eq!((added, removed), (vec!["ci.yml".to_string()], vec![]));
        assert_eq!(names(&gh), ["plan.yml:config", "ci.yml:saved"]);
        // a restart reads the saved list back
        let again = GitHub::new(&cfg, Some(&dir)).await.unwrap();
        assert_eq!(names(&again), ["plan.yml:config", "ci.yml:saved"]);
        // unticking removes saved ones only
        let (_, removed) = again.save_repo("o/r", &[]).await.unwrap();
        assert_eq!(removed, ["ci.yml"]);
        assert_eq!(names(&again), ["plan.yml:config"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn signs_a_verifiable_jwt() {
        let key = load_key(TEST_KEY).unwrap();
        let jwt = app_jwt(&key, 42, 1_800_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(
            (claims["iss"].as_str(), claims["exp"].as_i64()),
            (Some("42"), Some(1_800_000_540))
        );
        let public = ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            key.public().as_ref(),
        );
        let sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert!(
            public
                .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
                .is_ok()
        );
    }

    #[test]
    fn maps_runs_and_validates_repos() {
        let r = json!({"run_number": 7, "display_title": "terraform apply", "status": "completed", "conclusion": "failure",
            "event": "push", "head_branch": "main", "actor": {"login": "bot"}, "head_sha": "abcdef1234",
            "run_started_at": "2026-10-22T08:00:00Z", "updated_at": "2026-10-22T08:02:30Z", "html_url": "https://github.com/o/r/actions/runs/1"});
        let run = run(&r, "https://github.com/o/r");
        assert_eq!(
            (run.sha.as_str(), run.seconds, run.conclusion.as_deref()),
            ("abcdef1", Some(150), Some("failure"))
        );
        assert_eq!(run.commit_link, "https://github.com/o/r/commit/abcdef1234");
        assert!(valid_repo("franirafa/infrastructure"));
        assert!(
            !valid_repo("a/b/c")
                && !valid_repo("../x")
                && !valid_repo("o/..")
                && !valid_repo("noslash")
        );
        assert!(valid_repo("org/.github"));
    }

    use axum::{
        Json, Router,
        extract::Path,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn bearer(h: &HeaderMap) -> String {
        h.get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .trim_start_matches("Bearer ")
            .to_string()
    }

    fn not_found() -> Response {
        (StatusCode::NOT_FOUND, r#"{"message": "Not Found"}"#).into_response()
    }

    fn authorized(h: &HeaderMap) -> bool {
        ["inst-1", "inst-2", "pat"].contains(&bearer(h).as_str())
    }

    /// A GitHub API: user1 is a user installation (id 1), org1 an
    /// organization installation (id 2), noid and notoken are broken
    /// installations. Workflow reads need an installation token or "pat".
    /// Counts the access token requests.
    async fn fake(minted: Arc<AtomicUsize>) -> String {
        let installation = |kind: &'static str| {
            move |Path(owner): Path<String>, h: HeaderMap| async move {
                assert_eq!(bearer(&h).split('.').count(), 3, "app JWT expected");
                match (kind, owner.as_str()) {
                    ("users", "user1") => Json(json!({"id": 1})).into_response(),
                    ("orgs", "org1") => Json(json!({"id": 2})).into_response(),
                    ("users", "noid") => Json(json!({})).into_response(),
                    ("users", "notoken") => Json(json!({"id": 9})).into_response(),
                    _ => not_found(),
                }
            }
        };
        let access = move |Path(id): Path<u64>| {
            let minted = minted.clone();
            async move {
                minted.fetch_add(1, Ordering::SeqCst);
                match id {
                    9 => Json(json!({})),
                    _ => Json(json!({"token": format!("inst-{id}")})),
                }
            }
        };
        let meta = |Path((_, repo, file)): Path<(String, String, String)>, h: HeaderMap| async move {
            if !authorized(&h) {
                return (StatusCode::UNAUTHORIZED, "Bad credentials").into_response();
            }
            match repo.as_str() {
                "limited" => (
                    StatusCode::FORBIDDEN,
                    [("x-ratelimit-remaining", "0")],
                    "API rate limit exceeded",
                )
                    .into_response(),
                "private" => (StatusCode::FORBIDDEN, "Resource not accessible").into_response(),
                _ if file == "ci.yml" => Json(json!({"name": "CI"})).into_response(),
                _ => Json(json!({})).into_response(),
            }
        };
        let runs = |h: HeaderMap| async move {
            assert!(authorized(&h));
            Json(json!({"workflow_runs": [
                {"run_number": 12, "display_title": "fix", "status": "in_progress", "event": "push",
                 "head_branch": "main", "actor": {"login": "dev"}, "head_sha": "0123456789",
                 "run_started_at": "2026-10-22T08:00:00Z", "html_url": "https://github.com/o/r/actions/runs/12"},
                {"run_number": 11, "status": "completed", "conclusion": "success"},
            ]}))
        };
        let installations = |h: HeaderMap| async move {
            assert_eq!(bearer(&h), "user-token");
            Json(json!({"installations": [{"id": 1}, {"id": "x"}, {"id": 2}]}))
        };
        let repositories = |Path(id): Path<u64>| async move {
            Json(match id {
                1 => json!({"repositories": [{"full_name": "zed/b"}, {"name": "no full name"}]}),
                _ => json!({"repositories": [{"full_name": "acme/a"}]}),
            })
        };
        let workflows = |Path((owner, _)): Path<(String, String)>| async move {
            if owner == "gone" {
                return not_found();
            }
            Json(json!({"workflows": [
                {"name": "CI", "path": ".github/workflows/ci.yml", "state": "active"},
                {"name": "Dynamic", "state": "active"},
            ]}))
            .into_response()
        };
        crate::testkit::serve(
            Router::new()
                .route("/users/{owner}/installation", get(installation("users")))
                .route("/orgs/{owner}/installation", get(installation("orgs")))
                .route("/app/installations/{id}/access_tokens", post(access))
                .route("/repos/{owner}/{repo}/actions/workflows", get(workflows))
                .route("/repos/{owner}/{repo}/actions/workflows/{file}", get(meta))
                .route(
                    "/repos/{owner}/{repo}/actions/workflows/{file}/runs",
                    get(runs),
                )
                .route("/user/installations", get(installations))
                .route("/user/installations/{id}/repositories", get(repositories)),
        )
        .await
    }

    fn watch(repo: &str, workflow: &str) -> config::Watch {
        config::Watch {
            repo: repo.into(),
            workflow: workflow.into(),
        }
    }

    fn cfg(api_url: &str) -> config::GitHub {
        config::GitHub {
            app_id: Some(42),
            private_key: Some(crate::secret::Secret::Plain(TEST_KEY.into())),
            token: None,
            client_id: None,
            client_secret: None,
            watch: Vec::new(),
            api_url: api_url.into(),
            quiet: false,
            store: config::WatchStore::Off,

            install_url: None,
        }
    }

    #[tokio::test]
    async fn app_installation_tokens_and_runs() {
        let minted = Arc::new(AtomicUsize::new(0));
        let url = fake(minted.clone()).await;
        let mut c = cfg(&url);
        c.watch = vec![
            watch("user1/app", "ci.yml"),
            watch("org1/infra", "plan.yml"),
            watch("none/x", "a.yml"),
            watch("user1/limited", "ci.yml"),
            watch("user1/private", "ci.yml"),
        ];
        let gh = GitHub::new(&c, None).await.unwrap();
        assert!(!gh.helper && gh.store.name().is_none());

        let all = gh.workflows().await.unwrap();
        let by = |label: &str| all.iter().find(|w| w.label() == label).unwrap();
        let app = by("user1/app/ci.yml");
        assert_eq!(
            (app.name.as_str(), app.source, app.err()),
            ("CI", "config", None)
        );
        assert_eq!(
            app.link,
            "https://github.com/user1/app/actions/workflows/ci.yml"
        );
        let r = &app.runs[0];
        assert_eq!(
            (r.number, r.status.as_str(), r.seconds, r.sha.as_str()),
            (12, "in_progress", None, "0123456")
        );
        assert_eq!(app.runs[1].conclusion.as_deref(), Some("success"));
        // organizations: /users fails, /orgs answers; a workflow without a name keeps its file name
        let infra = by("org1/infra/plan.yml");
        assert_eq!((infra.name.as_str(), infra.err()), ("plan.yml", None));
        assert!(
            by("none/x/a.yml")
                .err()
                .unwrap()
                .starts_with("the GitHub App is not installed for none: HTTP 404")
        );
        assert_eq!(
            by("user1/limited/ci.yml").err(),
            Some("GitHub rate limit reached; showing the last data")
        );
        assert_eq!(
            by("user1/private/ci.yml").err(),
            Some("HTTP 403 Forbidden: Resource not accessible")
        );
        // one token per owner, then cached
        assert_eq!(minted.load(Ordering::SeqCst), 2);
        gh.workflows().await.unwrap();
        assert_eq!(minted.load(Ordering::SeqCst), 2);

        let e = gh.token_for("noid").await.unwrap_err();
        assert_eq!(e.to_string(), "installation without id");
        let e = gh.token_for("notoken").await.unwrap_err();
        assert_eq!(e.to_string(), "no installation token");
    }

    #[tokio::test]
    async fn token_workflows_fail_together() {
        let url = fake(Arc::new(AtomicUsize::new(0))).await;
        let mut c = cfg(&url);
        (c.app_id, c.private_key) = (None, None);
        c.token = Some(crate::secret::Secret::Plain("pat".into()));
        let gh = GitHub::new(&c, None).await.unwrap();
        assert!(gh.workflows().await.unwrap().is_empty());

        c.watch = vec![watch("o/limited", "ci.yml"), watch("o/private", "ci.yml")];
        let gh = GitHub::new(&c, None).await.unwrap();
        let e = gh.workflows().await.err().unwrap();
        assert_eq!(
            e.to_string(),
            "GitHub rate limit reached; showing the last data"
        );

        c.token = Some(crate::secret::Secret::Plain("wrong".into()));
        c.watch = vec![watch("o/r", "ci.yml")];
        let gh = GitHub::new(&c, None).await.unwrap();
        let e = gh.workflows().await.err().unwrap();
        assert_eq!(e.to_string(), "HTTP 401 Unauthorized: Bad credentials");
    }

    #[tokio::test]
    async fn sign_in_helper() {
        let url = fake(Arc::new(AtomicUsize::new(0))).await;
        let gh = GitHub::new(&cfg(&url), None).await.unwrap();
        assert!(gh.authorize_url("https://homie.example/cb", "s").is_none());
        let e = gh
            .exchange("code", "https://homie.example/cb")
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "the GitHub sign-in helper is not configured");

        let mut c = cfg(&url);
        c.client_id = Some(crate::secret::Secret::Plain("Iv1.abc".into()));
        c.client_secret = Some(crate::secret::Secret::Plain("shh".into()));
        let gh = GitHub::new(&c, None).await.unwrap();
        assert!(gh.helper);
        let u =
            url::Url::parse(&gh.authorize_url("https://homie.example/cb", "st8").unwrap()).unwrap();
        assert_eq!(u.path(), "/login/oauth/authorize");
        let q: HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(
            (
                q["client_id"].as_str(),
                q["redirect_uri"].as_str(),
                q["state"].as_str()
            ),
            ("Iv1.abc", "https://homie.example/cb", "st8")
        );

        assert_eq!(
            gh.user_repos("user-token").await.unwrap(),
            ["acme/a", "zed/b"]
        );
        let w = gh.user_workflows("user-token", "o/r").await.unwrap();
        assert_eq!(
            w[0],
            json!({"name": "CI", "file": "ci.yml", "state": "active"})
        );
        assert_eq!(w[1]["file"], "");
        let e = gh.user_workflows("user-token", "o/r/x").await.unwrap_err();
        assert_eq!(e.to_string(), "repository must be owner/name");
        assert!(gh.user_workflows("user-token", "gone/r").await.is_err());
    }

    #[tokio::test]
    async fn config_errors() {
        async fn err(c: config::GitHub) -> String {
            GitHub::new(&c, None).await.err().unwrap().to_string()
        }
        let mut c = cfg("https://api.example");
        c.private_key = None;
        assert_eq!(err(c).await, "github: set app_id and private_key, or token");
        let mut c = cfg("https://api.example");
        c.private_key = Some(crate::secret::Secret::Plain(
            "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----".into(),
        ));
        assert!(
            err(c)
                .await
                .starts_with("github.private_key: not an RSA key")
        );
        let mut c = cfg("https://api.example");
        c.private_key = Some(crate::secret::Secret::Plain("not base64!".into()));
        assert!(!err(c).await.is_empty());
        assert!(err(cfg("no url")).await.starts_with("github: "));
        let mut c = cfg("https://api.example");
        c.client_id = Some(crate::secret::Secret::Env {
            env: "HOMIE_TEST_GH_UNSET".into(),
        });
        assert_eq!(
            err(c).await,
            "environment variable HOMIE_TEST_GH_UNSET is not set"
        );
        let mut c = cfg("https://api.example");
        c.store = config::WatchStore::File;
        assert_eq!(
            err(c).await,
            "github.store: github.store = \"file\" needs state_dir"
        );

        let dir = std::env::temp_dir().join(format!("homie-gh-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("github-watch.json"), "{not json").unwrap();
        let mut c = cfg("https://api.example");
        c.store = config::WatchStore::File;
        let e = GitHub::new(&c, Some(&dir)).await.err().unwrap().to_string();
        assert!(e.starts_with("github.store: "), "{e}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

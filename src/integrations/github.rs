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
    target: Target,
    cred: Cred,
    client_id: Option<String>,
    client_secret: Option<String>,
    // per repository owner: installation token and its expiry
    tokens: Mutex<HashMap<String, (String, Instant)>>,
}

enum Cred {
    App { id: u64, key: ring::rsa::KeyPair },
    Token(SecretCell),
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
            target,
            cred,
            client_id,
            client_secret,
            tokens: Mutex::new(HashMap::new()),
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

    async fn workflow(&self, w: &config::Watch, source: &'static str) -> Workflow {
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
            let token = self.token_for(owner).await?;
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
        let all = join_all(watch.iter().map(|(w, s)| self.workflow(w, s))).await;
        if !all.is_empty() && all.iter().all(|w| w.error.is_some()) {
            return Err(all[0].error.clone().unwrap_or_default().into());
        }
        Ok(all)
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
}

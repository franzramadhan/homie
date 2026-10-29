//! Upstream versions (RFC 0020): what is the newest release of something,
//! through a few generic sources (GitHub releases, the OCI distribution API,
//! Helm repository indexes, dl.k8s.io). Every lookup is opt-in by config and
//! cached for 12 hours.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;

use crate::{Result, config::Tls, http};

const TTL: Duration = Duration::from_secs(12 * 3600);
/// a failed lookup is retried after this, not on every query
const FAILED_TTL: Duration = Duration::from_secs(30 * 60);

/// A version tag: optional `v`, dotted numbers, and the rest (`-alpine`,
/// `-rc.1`, `+k3s1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Ver {
    pub prefix: String,
    pub nums: Vec<u64>,
    pub suffix: String,
}

pub fn parse(tag: &str) -> Option<Ver> {
    let prefix = if tag.starts_with('v') { "v" } else { "" };
    let rest = &tag[prefix.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    let (numbers, suffix) = rest.split_at(end);
    let nums: Vec<u64> = numbers
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    if nums.is_empty() || numbers.ends_with('.') {
        return None;
    }
    Some(Ver {
        prefix: prefix.into(),
        nums,
        suffix: suffix.into(),
    })
}

impl Ver {
    /// `-rc.1`, `-beta`, `-alpha.2`, `-dev`, `-pre`, `-snapshot`
    pub fn prerelease(&self) -> bool {
        let s = self.suffix.to_ascii_lowercase();
        s.starts_with('-')
            && ["rc", "alpha", "beta", "dev", "pre", "snapshot"]
                .iter()
                .any(|w| s.contains(w))
    }

    /// The letters of a `+` build suffix (`k3s` of `+k3s1`, `k0s` of `+k0s.0`)
    /// and its trailing number, if any.
    fn build(&self) -> (String, Option<u64>) {
        let Some(b) = self.suffix.strip_prefix('+') else {
            return (self.suffix.clone(), None);
        };
        // the name is all but the trailing number: k3s1, k0s.0, rke2r1
        let name = b.trim_end_matches(|c: char| c.is_ascii_digit());
        let number = b[name.len()..].parse().ok();
        (format!("+{}", name.trim_end_matches('.')), number)
    }
}

/// Newer versions than the installed one, by the same shape.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Latest {
    /// the newest with the same major and minor
    pub patch: Option<String>,
    /// the newest of all
    pub newest: Option<String>,
}

impl Latest {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.patch.is_none() && self.newest.is_none()
    }
}

/// numbers, then the build number when it counts
type Key = (Vec<u64>, u64);

/// Compares `installed` with `candidates`: same number of parts, same
/// suffix (or the same build letters), no pre-releases. A build number only
/// counts when the installed version has one (k0s reports `+k0s`, not
/// `+k0s.1`).
pub fn latest(installed: &str, candidates: &[String]) -> Latest {
    let Some(inst) = parse(installed) else {
        return Latest::default();
    };
    let (inst_build, inst_n) = inst.build();
    let key = |v: &Ver, n: Option<u64>| {
        (
            v.nums.clone(),
            if inst_n.is_some() { n.unwrap_or(0) } else { 0 },
        )
    };
    let mine = key(&inst, inst_n);
    let newer: Vec<(&String, Ver, Key)> = candidates
        .iter()
        .filter_map(|c| parse(c).map(|v| (c, v)))
        .filter(|(_, v)| {
            !v.prerelease()
                && v.prefix == inst.prefix
                && v.nums.len() == inst.nums.len()
                && v.build().0 == inst_build
        })
        .map(|(c, v)| {
            let k = key(&v, v.build().1);
            (c, v, k)
        })
        .filter(|(_, _, k)| *k > mine)
        .collect();
    let best = |f: &dyn Fn(&Ver) -> bool| {
        newer
            .iter()
            .filter(|(_, v, _)| f(v))
            .max_by(|a, b| a.2.cmp(&b.2))
            .map(|(c, _, _)| (*c).clone())
    };
    let minor = inst.nums.len().min(2);
    Latest {
        patch: best(&|v: &Ver| v.nums[..minor] == inst.nums[..minor]),
        newest: best(&|_| true),
    }
}

type Cached = (Instant, std::result::Result<Vec<String>, String>);
static CACHE: LazyLock<Mutex<HashMap<String, Cached>>> = LazyLock::new(Default::default);

/// One lock per cache key, so queries that miss together share one fetch
/// (one entry per configured source, never removed).
static FLIGHTS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);
/// At most this many upstream fetches at once across every query: a cold
/// cache otherwise opens dozens of connections and big tag lists together.
static FETCHES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(6);

/// Waits until no other caller is fetching `key`. The caller checks its
/// cache again with the guard held, then fetches with a [`FETCHES`] permit.
async fn flight(key: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = FLIGHTS
        .lock()
        .unwrap()
        .entry(key.to_string())
        .or_default()
        .clone();
    lock.lock_owned().await
}

fn fresh(key: &str) -> Option<Result<Vec<String>>> {
    let cache = CACHE.lock().unwrap();
    let (at, r) = cache.get(key)?;
    (at.elapsed() < if r.is_ok() { TTL } else { FAILED_TTL }).then(|| r.clone().map_err(Into::into))
}

/// `key`'s list from the cache, else `fetch` (failures cached shorter).
async fn cached<F, Fut>(key: &str, fetch: F) -> Result<Vec<String>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>>>,
{
    if let Some(r) = fresh(key) {
        return r;
    }
    let _flight = flight(key).await;
    if let Some(r) = fresh(key) {
        return r;
    }
    let _permit = FETCHES.acquire().await.expect("never closed");
    let started = Instant::now();
    let r = fetch().await.map_err(|e| e.to_string());
    let ms = started.elapsed().as_millis() as u64;
    match &r {
        Ok(v) => tracing::info!(
            msg = "upstream",
            source = key,
            versions = v.len(),
            ms,
            result = "ok"
        ),
        Err(e) => tracing::warn!(msg = "upstream", source = key, error = %e, ms, result = "error"),
    }
    CACHE
        .lock()
        .unwrap()
        .insert(key.to_string(), (Instant::now(), r.clone()));
    r.map_err(Into::into)
}

/// Puts `versions` in the cache under `key`, as if fetched: the GitHub and
/// dl.k8s.io hosts are fixed, so tests answer them from the cache.
#[cfg(test)]
pub fn seed(key: &str, versions: &[&str]) {
    let v = versions.iter().map(|s| s.to_string()).collect();
    CACHE
        .lock()
        .unwrap()
        .insert(key.to_string(), (Instant::now(), Ok(v)));
}

fn tls() -> Tls {
    Tls {
        tls_verify: true,
        ca_file: None,
        tls_server_name: None,
    }
}

/// GET with up to three redirects (dl.k8s.io answers with one, registries
/// send blobs to a CDN). Credentials never follow a redirect to another host.
async fn get(url: &str, headers: &[(&str, String)]) -> Result<reqwest::Response> {
    let mut url = url.to_string();
    let mut host = None;
    for _ in 0..4 {
        let t = http::target(&url, &tls()).await?;
        let first = host.get_or_insert_with(|| t.base.host_str().map(String::from));
        let same = first.as_deref() == t.base.host_str();
        let mut req = t.client.get(t.base.clone());
        for (k, v) in headers {
            if same || *k != "Authorization" {
                req = req.header(*k, v);
            }
        }
        let resp = req.send().await?;
        if resp.status().is_redirection()
            && let Some(loc) = resp.headers().get("location").and_then(|l| l.to_str().ok())
        {
            url = t.base.join(loc)?.to_string();
            continue;
        }
        return Ok(resp);
    }
    Err(format!("{url}: too many redirects").into())
}

/// Release tags of a GitHub repository, drafts and pre-releases dropped.
pub async fn github_releases(repo: &str, token: Option<&str>) -> Result<Vec<String>> {
    cached(&format!("github:{repo}"), || async move {
        let mut headers = vec![("Accept", "application/vnd.github+json".to_string())];
        if let Some(t) = token {
            headers.push(("Authorization", format!("Bearer {t}")));
        }
        let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
        let v: Value = http::ok(get(&url, &headers).await?).await?.json().await?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["draft"] != true && r["prerelease"] != true)
            .filter_map(|r| r["tag_name"].as_str().map(String::from))
            .collect())
    })
    .await
}

/// The newest Kubernetes patch of `major.minor`, and the newest stable.
pub async fn kubernetes(installed: &str) -> Result<Latest> {
    let v = parse(installed)
        .filter(|v| v.nums.len() >= 2)
        .ok_or("no Kubernetes version")?;
    let minor = format!("stable-{}.{}", v.nums[0], v.nums[1]);
    let mut found = Vec::new();
    for file in [minor.as_str(), "stable"] {
        let tags = cached(&format!("dl.k8s.io:{file}"), || async move {
            let url = format!("https://dl.k8s.io/release/{file}.txt");
            let text = http::ok(get(&url, &[]).await?).await?.text().await?;
            Ok(vec![text.trim().to_string()])
        })
        .await?;
        found.extend(tags);
    }
    // the Kubernetes part only: distributions add +k0s, +k3s1
    let plain = format!(
        "v{}",
        v.nums
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".")
    );
    Ok(latest(&plain, &found))
}

/// `realm`, `service` and `scope` of a `WWW-Authenticate: Bearer` challenge.
fn challenge(h: &str) -> HashMap<String, String> {
    let h = h.trim_start_matches("Bearer").trim();
    let mut out = HashMap::new();
    let mut rest = h;
    while let Some((k, v)) = rest.split_once("=\"") {
        let Some((val, after)) = v.split_once('"') else {
            break;
        };
        out.insert(
            k.trim().trim_start_matches(',').trim().to_string(),
            val.to_string(),
        );
        rest = after;
    }
    out
}

/// The registry's base URL; one given with http:// is used as is (an
/// insecure registry).
fn registry_base(registry: &str) -> String {
    match registry {
        "docker.io" | "index.docker.io" => "https://registry-1.docker.io".to_string(),
        r if r.starts_with("http://") || r.starts_with("https://") => {
            r.trim_end_matches('/').to_string()
        }
        r => format!("https://{r}"),
    }
}

/// Basic credentials as a header value.
fn basic(creds: Option<(String, String)>) -> Option<String> {
    creds.map(|(u, p)| {
        use base64::Engine;
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))
        )
    })
}

/// GET on a registry, through its bearer-token challenge: `auth` starts as
/// the basic credentials and keeps the token for the next requests.
async fn registry_get(
    url: &str,
    accept: &str,
    basic: &Option<String>,
    auth: &mut Option<String>,
) -> Result<reqwest::Response> {
    let headers = |a: &Option<String>| {
        let mut h = vec![("Accept", accept.to_string())];
        h.extend(a.iter().map(|a| ("Authorization", a.clone())));
        h
    };
    let resp = get(url, &headers(auth)).await?;
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(resp);
    }
    let c = challenge(
        resp.headers()
            .get("www-authenticate")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default(),
    );
    let realm = c
        .get("realm")
        .ok_or("registry asked for auth without a realm")?;
    let mut token_url = url::Url::parse(realm)?;
    for k in ["service", "scope"] {
        if let Some(v) = c.get(k) {
            token_url.query_pairs_mut().append_pair(k, v);
        }
    }
    let th: Vec<(&str, String)> = basic.iter().map(|a| ("Authorization", a.clone())).collect();
    let t: Value = http::ok(get(token_url.as_str(), &th).await?)
        .await?
        .json()
        .await?;
    let token = t["token"]
        .as_str()
        .or(t["access_token"].as_str())
        .ok_or("registry gave no token")?;
    *auth = Some(format!("Bearer {token}"));
    get(url, &headers(auth)).await
}

/// Tags of an OCI repository (`registry`, `name`), anonymous or with basic
/// credentials, through the registry's bearer-token challenge.
pub async fn oci_tags(
    registry: &str,
    name: &str,
    creds: Option<(String, String)>,
) -> Result<Vec<String>> {
    let base = registry_base(registry);
    let key = format!("oci:{base}/{name}");
    cached(&key, || async move {
        let basic = basic(creds);
        let mut auth = basic.clone();
        let mut url = format!("{base}/v2/{name}/tags/list?n=1000");
        let mut tags = Vec::new();
        for _ in 0..10 {
            let resp = registry_get(&url, "application/json", &basic, &mut auth).await?;
            let next = resp
                .headers()
                .get("link")
                .and_then(|l| l.to_str().ok())
                .and_then(|l| l.split(';').next())
                .map(|l| l.trim().trim_matches(['<', '>']).to_string());
            let v: Value = http::ok(resp).await?.json().await?;
            tags.extend(
                v["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(String::from)),
            );
            match next {
                Some(n) => url = url::Url::parse(&base)?.join(&n)?.to_string(),
                None => break,
            }
        }
        Ok(tags)
    })
    .await
}

const MANIFESTS: &str = "application/vnd.oci.image.index.v1+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/vnd.oci.image.manifest.v1+json, \
    application/vnd.docker.distribution.manifest.v2+json";
const SOURCE: &str = "org.opencontainers.image.source";

/// The project's repository from an OCI artifact (RFC 0026): the
/// `org.opencontainers.image.source` annotation of the manifest, else the
/// image config label, else a Helm chart config's `sources` and `home`.
/// Read once per repository, at `reference` (a tag), cached with the tags.
pub async fn oci_source(
    registry: &str,
    name: &str,
    reference: &str,
    creds: Option<(String, String)>,
) -> Result<Option<String>> {
    let base = registry_base(registry);
    let key = format!("oci-source:{base}/{name}");
    let found = cached(&key, || async move {
        let basic = basic(creds);
        let mut auth = basic.clone();
        let mut fetch = async |kind: &str, r: &str| -> Result<Value> {
            // a digest from a manifest goes into the path: nothing but a digest
            if !r
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
            {
                return Err(format!("bad reference {r:?}").into());
            }
            let url = format!("{base}/v2/{name}/{kind}/{r}");
            let resp = registry_get(&url, MANIFESTS, &basic, &mut auth).await?;
            Ok(http::ok(resp).await?.json().await?)
        };
        let annotation = |v: &Value| v["annotations"][SOURCE].as_str().map(String::from);
        let mut m = fetch("manifests", reference).await?;
        if let Some(list) = m["manifests"].as_array() {
            // an index: its own annotation, else the linux/amd64 image
            if let Some(s) = annotation(&m) {
                return Ok(vec![s]);
            }
            let pick = list
                .iter()
                .find(|d| {
                    d["platform"]["os"] == "linux" && d["platform"]["architecture"] == "amd64"
                })
                .or(list.first());
            let digest = pick
                .and_then(|d| d["digest"].as_str())
                .ok_or("empty index")?;
            m = fetch("manifests", digest).await?;
        }
        if let Some(s) = annotation(&m) {
            return Ok(vec![s]);
        }
        let Some(digest) = m["config"]["digest"].as_str() else {
            return Ok(Vec::new());
        };
        let c = fetch("blobs", digest).await?;
        // labels are inherited from the base image: only one that names
        // this image counts
        let urls = c["config"]["Labels"][SOURCE]
            .as_str()
            .filter(|u| names_image(name, u))
            .into_iter()
            .chain(
                c["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            )
            .chain(c["home"].as_str());
        Ok(pick_source(urls).into_iter().collect())
    })
    .await?;
    Ok(found.into_iter().next())
}

/// Whether the GitHub repository at `url` plausibly is the one of the image
/// `name`: a path part (but `library`) and the owner or the repository
/// contain one another (letters and digits only, at least 3), or share a
/// word of 3 or more. A label from a base image (`distroless` on
/// `external-dns`) does not.
fn names_image(name: &str, url: &str) -> bool {
    let Some(repo) = github_repo(url) else {
        return false;
    };
    let squash = |s: &str| -> String {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= 3)
            .map(str::to_ascii_lowercase)
            .collect()
    };
    let close = |a: &str, b: &str| {
        let (x, y) = (squash(a), squash(b));
        let (short, long) = if x.len() <= y.len() { (x, y) } else { (y, x) };
        (short.len() >= 3 && long.contains(&short)) || words(a).iter().any(|w| words(b).contains(w))
    };
    name.split('/')
        .filter(|p| *p != "library")
        .any(|p| repo.split('/').any(|r| close(p, r)))
}

/// The first GitHub repository among `urls`, else the first https URL.
fn pick_source<'a>(urls: impl Iterator<Item = &'a str>) -> Option<String> {
    let urls: Vec<&str> = urls.collect();
    urls.iter()
        .find(|u| github_repo(u).is_some())
        .or(urls.iter().find(|u| u.starts_with("https://")))
        .map(|u| u.to_string())
}

/// Every chart of a Helm HTTP repository's `index.yaml`, one string per
/// version: name, version and the project's repository (RFC 0026), split by
/// NUL.
async fn helm_entries(repo: &str) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Index {
        entries: HashMap<String, Vec<Entry>>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        version: String,
        #[serde(default)]
        sources: Option<Vec<String>>,
        #[serde(default)]
        home: Option<String>,
    }
    let repo = repo.trim_end_matches('/').to_string();
    // the whole index is one download: cache every chart of it at once
    let key = format!("helm:{repo}");
    cached(&key, || async {
        let body = http::ok(get(&format!("{repo}/index.yaml"), &[]).await?)
            .await?
            .bytes()
            .await?;
        let index: Index = serde_yaml_ng::from_slice(&body)?;
        Ok(index
            .entries
            .into_iter()
            .flat_map(|(name, es)| {
                es.into_iter().map(move |e| {
                    let urls = e.sources.iter().flatten().map(String::as_str);
                    let src = pick_source(urls.chain(e.home.as_deref())).unwrap_or_default();
                    format!("{name}\u{0}{}\u{0}{src}", e.version)
                })
            })
            .collect())
    })
    .await
}

/// (version, repository) of each version of `chart` in a Helm repository.
async fn helm_chart(repo: &str, chart: &str) -> Result<Vec<(String, String)>> {
    Ok(helm_entries(repo)
        .await?
        .iter()
        .filter_map(|e| {
            let mut p = e.split('\u{0}');
            let (n, v, s) = (p.next()?, p.next()?, p.next().unwrap_or_default());
            (n == chart).then(|| (v.to_string(), s.to_string()))
        })
        .collect())
}

/// Versions of `chart` in a Helm HTTP repository's `index.yaml`.
pub async fn helm_index(repo: &str, chart: &str) -> Result<Vec<String>> {
    Ok(helm_chart(repo, chart)
        .await?
        .into_iter()
        .map(|(v, _)| v)
        .collect())
}

/// Registry and repository of a chart in `oci://registry/path[/chart]`.
fn oci_chart(rest: &str, chart: &str) -> Result<(String, String)> {
    let rest = rest.trim_end_matches('/');
    let full = if rest.rsplit('/').next() == Some(chart) {
        rest.to_string()
    } else {
        format!("{rest}/{chart}")
    };
    let (registry, name) = full
        .split_once('/')
        .ok_or("oci:// URL without a repository")?;
    Ok((registry.to_string(), name.to_string()))
}

/// Versions of a chart from its repository URL: `oci://registry/path/chart`
/// or an HTTP repository plus the chart name.
pub async fn chart_versions(
    repo: &str,
    chart: &str,
    creds: impl Fn(&str) -> Option<(String, String)>,
) -> Result<Vec<String>> {
    match repo.strip_prefix("oci://") {
        Some(rest) => {
            let (registry, name) = oci_chart(rest, chart)?;
            oci_tags(&registry, &name, creds(&registry)).await
        }
        None => helm_index(repo, chart).await,
    }
}

/// The project's repository of a chart: `sources` or `home` of its index
/// entry at `version` (else of any version), or the OCI artifact's metadata.
pub async fn chart_source(
    repo: &str,
    chart: &str,
    version: &str,
    creds: impl Fn(&str) -> Option<(String, String)>,
) -> Result<Option<String>> {
    match repo.strip_prefix("oci://") {
        Some(rest) => {
            let (registry, name) = oci_chart(rest, chart)?;
            oci_source(&registry, &name, version, creds(&registry)).await
        }
        None => {
            let all = helm_chart(repo, chart).await?;
            let src = |v: Option<&str>| {
                all.iter()
                    .find(|(ver, s)| !s.is_empty() && v.is_none_or(|v| v == ver))
                    .map(|(_, s)| s.clone())
            };
            Ok(src(Some(version)).or_else(|| src(None)))
        }
    }
}

/// GitHub's API; tests pass a local server.
pub const GITHUB_API: &str = "https://api.github.com";

/// `owner/repo` of a GitHub repository URL (`https://github.com/o/r`, with
/// `.git` or a deeper path) or of a plain `o/r`. Both parts are checked
/// against GitHub's rules, so links built from them stay on that repository.
pub fn github_repo(s: &str) -> Option<String> {
    let s = s.trim();
    let url = [
        "https://github.com/",
        "http://github.com/",
        "https://www.github.com/",
        "git+https://github.com/",
    ]
    .iter()
    .find_map(|p| s.strip_prefix(p));
    let path = url.unwrap_or(s);
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if url.is_none() && parts.next().is_some() {
        return None;
    }
    let owner_ok = (1..=39).contains(&owner.len())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let repo_ok = (1..=100).contains(&repo.len())
        && repo != "."
        && repo != ".."
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    (owner_ok && repo_ok).then(|| format!("{owner}/{repo}"))
}

/// `https://github.com/<repo>/<segments...>`, each segment escaped. `repo`
/// comes from [`github_repo`].
pub fn github_url(repo: &str, segments: &[&str]) -> String {
    let mut u = url::Url::parse("https://github.com").unwrap();
    u.path_segments_mut()
        .unwrap()
        .extend(repo.split('/'))
        .extend(segments);
    u.to_string()
}

/// One GitHub release, reduced to what the Updates page shows (RFC 0026).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Note {
    pub tag: String,
    pub name: String,
    pub published: Option<String>,
    pub url: String,
    /// the first lines of the body, as plain text
    pub excerpt: String,
    /// the body mentions a CVE, a GHSA or the word "security"
    pub security: bool,
}

/// Removes HTML tags and comments; a `<` that opens no tag stays.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let at = &rest[i..];
        let tag = at[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!');
        if !tag {
            out.push('<');
            rest = &at[1..];
            continue;
        }
        let end = if at.starts_with("<!--") {
            at.find("-->").map(|e| e + 3)
        } else {
            at.find('>').map(|e| e + 1)
        };
        // an unterminated tag hides the rest
        rest = end.map_or("", |e| &at[e..]);
    }
    out.push_str(rest);
    out
}

/// The first non-empty lines of a release body, without HTML, at most 600
/// characters. The UI shows it as text.
pub fn excerpt(body: &str) -> String {
    let text = strip_html(body);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(6)
        .collect();
    lines.join("\n").chars().take(600).collect()
}

/// `CVE-<year>-<n>`, `GHSA-`, or "security" as a word (a heading too).
pub fn security(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    let cve = b.match_indices("cve-").any(|(i, _)| {
        let r = &b.as_bytes()[i + 4..];
        r.len() >= 6
            && r[..4].iter().all(u8::is_ascii_digit)
            && r[4] == b'-'
            && r[5].is_ascii_digit()
    });
    let word = b.match_indices("security").any(|(i, w)| {
        let before = b[..i].chars().next_back();
        let after = b[i + w.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    });
    cve || word || b.contains("ghsa-")
}

/// The version part of a release tag and what comes before it: `v1.2.3` is
/// `("", 1.2.3)`, `chart-1.2.3` is `("chart-", 1.2.3)`.
pub fn tag_version(tag: &str) -> Option<(&str, Ver)> {
    std::iter::once(0)
        .chain(
            tag.char_indices()
                .filter(|(_, c)| "-_/@".contains(*c))
                .map(|(i, _)| i + 1),
        )
        .find_map(|i| {
            parse(&tag[i..])
                .filter(|v| v.suffix.is_empty() || v.suffix.starts_with(['-', '+']))
                .map(|v| (&tag[..i], v))
        })
}

/// The releases of `name` after `from` up to `to`, newest first, and the tag
/// of `from` when it is a release. A repository with several projects tags
/// them `<name>-1.2.3`: those tags win when there are any, else plain
/// version tags, else every tag.
pub fn between<'a>(
    notes: &'a [Note],
    name: &str,
    from: &str,
    to: &str,
) -> (Vec<&'a Note>, Option<&'a str>) {
    let (Some(from), Some(to)) = (parse(from), parse(to)) else {
        return (Vec::new(), None);
    };
    let tagged: Vec<(&Note, &str, Ver)> = notes
        .iter()
        .filter_map(|n| tag_version(&n.tag).map(|(p, v)| (n, p, v)))
        .filter(|(_, _, v)| !v.prerelease())
        .collect();
    let named = |p: &str| p.trim_end_matches(['-', '_', '/', '@']) == name;
    let group: Vec<&(&Note, &str, Ver)> = if tagged.iter().any(|t| named(t.1)) {
        tagged.iter().filter(|t| named(t.1)).collect()
    } else if tagged.iter().any(|t| t.1.is_empty()) {
        tagged.iter().filter(|t| t.1.is_empty()).collect()
    } else {
        tagged.iter().collect()
    };
    let mut inside: Vec<_> = group
        .iter()
        .filter(|t| from.nums < t.2.nums && t.2.nums <= to.nums)
        .collect();
    inside.sort_by(|a, b| b.2.nums.cmp(&a.2.nums));
    let running = group
        .iter()
        .find(|t| t.2.nums == from.nums)
        .map(|t| t.0.tag.as_str());
    (inside.into_iter().map(|t| t.0).collect(), running)
}

struct NotesEntry {
    /// no request before this
    until: Instant,
    notes: Vec<Note>,
    /// the last lookup failed: the last notes are kept
    error: Option<String>,
}
static NOTES: LazyLock<Mutex<HashMap<String, NotesEntry>>> = LazyLock::new(Default::default);

/// The GitHub releases of `repo` (`owner/repo`), drafts and pre-releases
/// left out, kept 12 hours. A failure (a 403 or 429 rate limit too) keeps
/// the last notes, is logged once, and is retried after 30 minutes; the
/// error comes back with the notes until then.
pub async fn release_notes(
    api: &str,
    repo: &str,
    token: Option<&str>,
) -> (Vec<Note>, Option<String>) {
    let fresh = || {
        let cache = NOTES.lock().unwrap();
        let e = cache.get(repo).filter(|e| Instant::now() < e.until)?;
        Some((e.notes.clone(), e.error.clone()))
    };
    if let Some(r) = fresh() {
        return r;
    }
    let _flight = flight(&format!("notes:{repo}")).await;
    if let Some(r) = fresh() {
        return r;
    }
    let _permit = FETCHES.acquire().await.expect("never closed");
    let started = Instant::now();
    let r: Result<Vec<Note>> = async {
        let mut headers = vec![("Accept", "application/vnd.github+json".to_string())];
        if let Some(t) = token {
            headers.push(("Authorization", format!("Bearer {t}")));
        }
        let url = format!("{api}/repos/{repo}/releases?per_page=100");
        let v: Value = http::ok(get(&url, &headers).await?).await?.json().await?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["draft"] != true && r["prerelease"] != true)
            .filter_map(|r| {
                let tag = r["tag_name"].as_str()?.to_string();
                let body = r["body"].as_str().unwrap_or_default();
                let name = r["name"].as_str().filter(|n| !n.trim().is_empty());
                Some(Note {
                    name: name.unwrap_or(&tag).to_string(),
                    published: r["published_at"].as_str().map(String::from),
                    url: github_url(repo, &["releases", "tag", &tag]),
                    excerpt: excerpt(body),
                    security: security(body),
                    tag,
                })
            })
            .collect())
    }
    .await;
    let ms = started.elapsed().as_millis() as u64;
    let mut cache = NOTES.lock().unwrap();
    match r {
        Ok(notes) => {
            tracing::info!(
                msg = "upstream",
                kind = "notes",
                repo,
                releases = notes.len(),
                ms,
                result = "ok"
            );
            let e = NotesEntry {
                until: Instant::now() + TTL,
                notes: notes.clone(),
                error: None,
            };
            cache.insert(repo.to_string(), e);
            (notes, None)
        }
        Err(err) => {
            let e = cache.entry(repo.to_string()).or_insert(NotesEntry {
                until: Instant::now(),
                notes: Vec::new(),
                error: None,
            });
            if e.error.is_none() {
                tracing::warn!(msg = "upstream", kind = "notes", repo, error = %err, ms, result = "error");
            }
            e.until = Instant::now() + FAILED_TTL;
            e.error = Some(err.to_string());
            (e.notes.clone(), e.error.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_tags() {
        assert_eq!(parse("v1.36.1").unwrap().nums, [1, 36, 1]);
        assert_eq!(parse("1.27.3-alpine").unwrap().suffix, "-alpine");
        assert_eq!(
            parse("v1.36.1+k3s1").unwrap().build(),
            ("+k3s".into(), Some(1))
        );
        assert_eq!(
            parse("v1.36.1+k0s.2").unwrap().build(),
            ("+k0s".into(), Some(2))
        );
        assert_eq!(parse("v1.36.1+k0s").unwrap().build(), ("+k0s".into(), None));
        assert!(parse("latest").is_none());
        assert!(parse("1.").is_none());
        assert!(parse("v2.1.0-rc.1").unwrap().prerelease());
        assert!(!parse("1.27-alpine").unwrap().prerelease());
    }

    #[test]
    fn newest_patch_and_overall() {
        let c = s(&[
            "v1.35.9",
            "v1.36.0",
            "v1.36.3",
            "v1.37.0-rc.1",
            "v1.37.1",
            "1.2",
        ]);
        assert_eq!(
            latest("v1.36.1", &c),
            Latest {
                patch: Some("v1.36.3".into()),
                newest: Some("v1.37.1".into())
            }
        );
        assert!(latest("v1.37.1", &c).is_empty());
        assert!(latest("garbage", &c).is_empty());
        // same shape only
        let c = s(&[
            "2.9.1",
            "2.10.0",
            "2.10.0-beta.1",
            "v2.11.0",
            "2.12.0-alpine",
            "2.12",
        ]);
        assert_eq!(latest("2.9.0", &c).newest.as_deref(), Some("2.10.0"));
        // image shapes: the suffix and the prefix must match
        assert_eq!(
            latest("2.9.0-alpine", &c).newest.as_deref(),
            Some("2.12.0-alpine")
        );
        assert_eq!(latest("v2.9.0", &c).newest.as_deref(), Some("v2.11.0"));
        assert_eq!(latest("2.9", &c).newest.as_deref(), Some("2.12"));
    }

    #[test]
    fn distribution_builds() {
        let k3s = s(&[
            "v1.36.1+k3s1",
            "v1.36.1+k3s2",
            "v1.36.2+k3s1",
            "v1.36.2-rc1+k3s1",
        ]);
        assert_eq!(
            latest("v1.36.1+k3s1", &k3s).patch.as_deref(),
            Some("v1.36.2+k3s1")
        );
        assert!(latest("v1.36.2+k3s1", &k3s).is_empty());
        assert_eq!(
            latest("v1.36.1+k3s2", &k3s).patch.as_deref(),
            Some("v1.36.2+k3s1")
        );
        // k0s reports no build number: only the Kubernetes part counts
        let k0s = s(&["v1.36.1+k0s.0", "v1.36.1+k0s.1", "v1.36.2+k0s.0"]);
        assert_eq!(
            latest("v1.36.1+k0s", &k0s).patch.as_deref(),
            Some("v1.36.2+k0s.0")
        );
        assert!(latest("v1.36.2+k0s", &k0s).is_empty());
        // a plain version never matches distribution builds
        assert!(latest("v1.36.1", &k0s).is_empty());
    }

    #[test]
    fn bearer_challenges() {
        let c = challenge(
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:a/b:pull""#,
        );
        assert_eq!(c["realm"], "https://ghcr.io/token");
        assert_eq!(c["service"], "ghcr.io");
        assert_eq!(c["scope"], "repository:a/b:pull");
        // an unterminated value ends the parse
        assert!(challenge(r#"Bearer realm="https://ghcr.io/token"#).is_empty());
    }

    #[tokio::test]
    async fn cached_github_and_kubernetes_lookups() {
        // both stable files are seeded the same way in updates.rs
        seed("dl.k8s.io:stable-1.997", &["v1.997.3"]);
        seed("dl.k8s.io:stable", &["v1.998.1"]);
        assert_eq!(
            kubernetes("v1.997.1+k0s").await.unwrap(),
            Latest {
                patch: Some("v1.997.3".into()),
                newest: Some("v1.998.1".into())
            }
        );
        for bad in ["garbage", "v1"] {
            let e = kubernetes(bad).await.unwrap_err().to_string();
            assert_eq!(e, "no Kubernetes version");
        }
        seed("github:acme-test/upstream", &["v1.0.0", "v1.1.0"]);
        assert_eq!(
            github_releases("acme-test/upstream", Some("t"))
                .await
                .unwrap(),
            ["v1.0.0", "v1.1.0"]
        );
        // a registry without a scheme is https
        seed("oci:https://registry.test/org/app", &["2.0.0"]);
        assert_eq!(
            oci_tags("registry.test", "org/app", None).await.unwrap(),
            ["2.0.0"]
        );
    }

    #[tokio::test]
    async fn chart_versions_by_repository_kind() {
        seed("oci:https://charts.test/org/chart", &["1.0.0"]);
        let asked = Mutex::new(Vec::new());
        let creds = |host: &str| {
            asked.lock().unwrap().push(host.to_string());
            None
        };
        for repo in ["oci://charts.test/org/", "oci://charts.test/org/chart"] {
            assert_eq!(
                chart_versions(repo, "chart", creds).await.unwrap(),
                ["1.0.0"]
            );
        }
        assert_eq!(*asked.lock().unwrap(), ["charts.test", "charts.test"]);
        let e = chart_versions("oci://charts.test", "charts.test", creds)
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "oci:// URL without a repository");
        let index =
            "apiVersion: v1\nentries:\n  a:\n  - version: 0.1.0\n  b:\n  - version: 9.0.0\n";
        let url = crate::testkit::serve(axum::Router::new().route(
            "/index.yaml",
            axum::routing::get(move || async move { index }),
        ))
        .await;
        assert_eq!(
            chart_versions(&format!("{url}/"), "a", creds)
                .await
                .unwrap(),
            ["0.1.0"]
        );
    }

    /// A fake registry: redirects, a failing repository, pages, and the
    /// ways a token challenge goes wrong.
    async fn registry(hits: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> String {
        use axum::{
            Json, Router,
            extract::Query,
            http::{HeaderMap, StatusCode, header},
            response::IntoResponse,
            routing::get as route,
        };
        use std::sync::atomic::Ordering;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let unauthorized =
            |c: String| (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, c)]).into_response();
        let realm = format!("{base}/auth");
        let empty = format!("{base}/empty");
        let app = Router::new()
            .route(
                "/v2/moved/app/tags/list",
                route(|| async {
                    (
                        StatusCode::FOUND,
                        [(header::LOCATION, "/v2/plain/app/tags/list")],
                    )
                }),
            )
            .route(
                "/v2/loop/app/tags/list",
                route(|| async {
                    (
                        StatusCode::FOUND,
                        [(header::LOCATION, "/v2/loop/app/tags/list")],
                    )
                }),
            )
            .route(
                "/v2/plain/app/tags/list",
                route(|| async { Json(serde_json::json!({"tags": ["1.0.0"]})) }),
            )
            .route(
                "/v2/broken/app/tags/list",
                route(move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    async { (StatusCode::INTERNAL_SERVER_ERROR, "down") }
                }),
            )
            .route(
                "/v2/paged/app/tags/list",
                route(|Query(q): Query<HashMap<String, String>>| async move {
                    if q.contains_key("last") {
                        return Json(serde_json::json!({"tags": ["1.1.0"]})).into_response();
                    }
                    let next = r#"</v2/paged/app/tags/list?n=1000&last=1.0.0>; rel="next""#;
                    (
                        [(header::LINK, next)],
                        Json(serde_json::json!({"tags": ["1.0.0"]})),
                    )
                        .into_response()
                }),
            )
            .route(
                "/v2/private/app/tags/list",
                route(move |h: HeaderMap| {
                    let realm = realm.clone();
                    async move {
                        match h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
                            Some("Bearer abc") => {
                                Json(serde_json::json!({"tags": ["3.0.0"]})).into_response()
                            }
                            _ => unauthorized(format!(r#"Bearer realm="{realm}""#)),
                        }
                    }
                }),
            )
            .route(
                "/auth",
                route(|h: HeaderMap| async move {
                    // base64 of u:p
                    let auth = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
                    assert_eq!(auth, Some("Basic dTpw"));
                    Json(serde_json::json!({"access_token": "abc"}))
                }),
            )
            .route(
                "/v2/norealm/app/tags/list",
                route(move || async move { unauthorized(r#"Bearer service="x""#.into()) }),
            )
            .route(
                "/v2/notoken/app/tags/list",
                route(move || {
                    let empty = empty.clone();
                    async move { unauthorized(format!(r#"Bearer realm="{empty}""#)) }
                }),
            )
            .route("/empty", route(|| async { Json(serde_json::json!({})) }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn registry_redirects_pages_and_failures() {
        use std::sync::{Arc, atomic::AtomicUsize, atomic::Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let base = registry(hits.clone()).await;
        assert_eq!(oci_tags(&base, "moved/app", None).await.unwrap(), ["1.0.0"]);
        let e = oci_tags(&base, "loop/app", None).await.unwrap_err();
        assert!(e.to_string().ends_with("too many redirects"), "{e}");
        assert_eq!(
            oci_tags(&base, "paged/app", None).await.unwrap(),
            ["1.0.0", "1.1.0"]
        );
        // a failure is cached too: the second lookup does not ask again
        for _ in 0..2 {
            let e = oci_tags(&base, "broken/app", None).await.unwrap_err();
            assert_eq!(e.to_string(), "HTTP 500 Internal Server Error: down");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // basic credentials buy the token; the challenge has no service
        assert_eq!(
            oci_tags(&base, "private/app", Some(("u".into(), "p".into())))
                .await
                .unwrap(),
            ["3.0.0"]
        );
        let e = oci_tags(&base, "norealm/app", None).await.unwrap_err();
        assert_eq!(e.to_string(), "registry asked for auth without a realm");
        let e = oci_tags(&base, "notoken/app", None).await.unwrap_err();
        assert_eq!(e.to_string(), "registry gave no token");
    }

    #[tokio::test]
    async fn registry_tags_through_the_token_challenge() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode, header},
            response::IntoResponse,
            routing::get as route,
        };
        // the fake registry needs to know its own address for the realm
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let realm = format!("{base}/token");
        let app = Router::new()
            .route(
                "/v2/org/chart/tags/list",
                route(move |h: HeaderMap| {
                    let realm = realm.clone();
                    async move {
                        if h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer t0k") {
                            let c = format!(r#"Bearer realm="{realm}",service="test",scope="repository:org/chart:pull""#);
                            return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, c)]).into_response();
                        }
                        Json(serde_json::json!({"name": "org/chart", "tags": ["1.0.0", "1.1.0"]})).into_response()
                    }
                }),
            )
            .route("/token", route(|| async { Json(serde_json::json!({"token": "t0k"})) }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            oci_tags(&base, "org/chart", None).await.unwrap(),
            ["1.0.0", "1.1.0"]
        );
        assert_eq!(
            latest("1.0.0", &oci_tags(&base, "org/chart", None).await.unwrap())
                .newest
                .as_deref(),
            Some("1.1.0")
        );
    }

    #[test]
    fn github_repositories_and_links() {
        for (input, want) in [
            ("https://github.com/acme/app", Some("acme/app")),
            ("https://github.com/acme/app.git", Some("acme/app")),
            (
                "https://www.github.com/acme/app/tree/main/charts/x",
                Some("acme/app"),
            ),
            ("git+https://github.com/acme/app#readme", Some("acme/app")),
            ("acme/app.js", Some("acme/app.js")),
            ("acme/app/extra", None),
            ("https://gitlab.com/acme/app", None),
            ("https://github.com/acme", None),
            ("https://github.com/ac%2fme/app", None),
            ("https://github.com/acme/..", None),
            ("https://github.com/acme/a\"><script>", None),
            ("https://github.com.evil.example/acme/app", None),
        ] {
            assert_eq!(github_repo(input).as_deref(), want, "{input}");
        }
        assert_eq!(github_url("acme/app", &[]), "https://github.com/acme/app");
        assert_eq!(
            github_url("acme/app", &["compare", "v1/x?y#z...v2"]),
            "https://github.com/acme/app/compare/v1%2Fx%3Fy%23z...v2"
        );
        let urls = ["https://app.example", "https://github.com/acme/app"];
        assert_eq!(
            pick_source(urls.into_iter()).as_deref(),
            Some("https://github.com/acme/app")
        );
        let urls = ["http://plain.example", "https://app.example"];
        assert_eq!(
            pick_source(urls.into_iter()).as_deref(),
            Some("https://app.example")
        );
        assert_eq!(pick_source(["javascript:alert(1)"].into_iter()), None);
    }

    #[test]
    fn image_labels_must_name_the_image() {
        for (name, repo, ok) in [
            ("library/node", "nodejs/docker-node", true),
            ("pihole/pihole", "pi-hole/docker-pi-hole", true),
            (
                "victoriametrics/victoria-metrics",
                "VictoriaMetrics/VictoriaMetrics",
                true,
            ),
            (
                "otel/opentelemetry-collector-k8s",
                "open-telemetry/opentelemetry-collector-releases",
                true,
            ),
            ("berriai/litellm-non_root", "BerriAI/litellm", true),
            ("k0sproject/envoy-distroless", "envoyproxy/envoy", true),
            (
                "k0sproject/apiserver-network-proxy-agent",
                "k0sproject/image-builder",
                true,
            ),
            (
                "external-dns/external-dns",
                "GoogleContainerTools/distroless",
                false,
            ),
            ("library/node", "acme/no", false),
        ] {
            let url = format!("https://github.com/{repo}");
            assert_eq!(names_image(name, &url), ok, "{name} {repo}");
        }
        assert!(!names_image("acme/app", "https://acme.example/app"));
    }

    #[test]
    fn excerpts_are_plain_text() {
        let body = "<!-- a comment with > inside -->\r\n## What's <b>new</b>\n\n\
                    <details><summary>x</summary><script>alert(1)</script></details>\n\
                    - a < b and 2<3\n<img src=x onerror=alert(1)>\nline 4\nline 5\nline 6\nline 7\n";
        let e = excerpt(body);
        assert_eq!(
            e,
            "## What's new\nxalert(1)\n- a < b and 2<3\nline 4\nline 5\nline 6"
        );
        assert!(!e.contains("<b") && !e.contains("<script") && !e.contains("<img"));
        // an unterminated tag hides the rest
        assert_eq!(excerpt("fixed <a href=\"x"), "fixed");
        assert_eq!(excerpt(&"é".repeat(700)).chars().count(), 600);
    }

    #[test]
    fn security_patterns() {
        for yes in [
            "Fixes CVE-2024-12345",
            "see ghsa-xxxx-yyyy",
            "## Security\n- bump",
            "security: patch",
            "a Security-related fix",
        ] {
            assert!(security(yes), "{yes}");
        }
        for no in [
            "set securityContext on pods",
            "podSecurityContext defaults",
            "insecurity",
            "CVE-24-1",
            "cve-2024-x",
            "",
        ] {
            assert!(!security(no), "{no}");
        }
    }

    fn note(tag: &str) -> Note {
        Note {
            tag: tag.into(),
            name: tag.into(),
            published: None,
            url: String::new(),
            excerpt: String::new(),
            security: false,
        }
    }

    #[test]
    fn tags_match_by_their_version_part() {
        let v = |t: &str| tag_version(t).map(|(p, v)| (p.to_string(), v.nums));
        assert_eq!(v("v1.2.3"), Some(("".into(), vec![1, 2, 3])));
        assert_eq!(v("chart-1.2.3"), Some(("chart-".into(), vec![1, 2, 3])));
        assert_eq!(
            v("kube-prometheus-stack-65.1.0"),
            Some(("kube-prometheus-stack-".into(), vec![65, 1, 0]))
        );
        assert_eq!(
            v("controller-v1.11.3"),
            Some(("controller-".into(), vec![1, 11, 3]))
        );
        assert_eq!(v("3scale-1.0.0"), Some(("3scale-".into(), vec![1, 0, 0])));
        assert_eq!(v("nightly"), None);

        // a repository of several charts: the chart's own tags win
        let mono: Vec<Note> = [
            "kube-state-metrics-6.0.0",
            "app-2.1.0",
            "app-2.0.1",
            "app-2.0.0",
            "app-1.9.0",
            "app-2.2.0-rc.1",
        ]
        .map(note)
        .to_vec();
        let (rel, from) = between(&mono, "app", "2.0.0", "2.1.0");
        let tags: Vec<&str> = rel.iter().map(|n| n.tag.as_str()).collect();
        assert_eq!(
            (tags, from),
            (vec!["app-2.1.0", "app-2.0.1"], Some("app-2.0.0"))
        );
        // plain tags for any name, `v` or not; images compare numbers only
        let plain: Vec<Note> = ["v1.28.0", "v1.27.4", "v1.27.3", "other-9.0.0"]
            .map(note)
            .to_vec();
        let (rel, from) = between(&plain, "nginx", "1.27.3-alpine", "1.28.0-alpine");
        assert_eq!(rel.len(), 2);
        assert_eq!(from, Some("v1.27.3"));
        // only prefixed tags of other names: every tag counts
        let other: Vec<Note> = ["helm-chart-4.11.3", "controller-v1.11.3"]
            .map(note)
            .to_vec();
        let (rel, from) = between(&other, "ingress-nginx", "4.11.2", "4.11.3");
        assert_eq!(
            (rel[0].tag.as_str(), rel.len(), from),
            ("helm-chart-4.11.3", 1, None)
        );
        assert!(between(&plain, "x", "latest", "1.0.0").0.is_empty());
    }

    /// Queries that miss together share one fetch.
    #[tokio::test]
    async fn concurrent_misses_fetch_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let lookups = (0..8).map(|_| {
            let hits = hits.clone();
            cached("test:single-flight", move || async move {
                hits.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(vec!["1.0.0".to_string()])
            })
        });
        for r in futures::future::join_all(lookups).await {
            assert_eq!(r.unwrap(), ["1.0.0"]);
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    /// A fake GitHub API: releases of `acme-test/notes`, then a rate limit.
    #[tokio::test]
    async fn release_notes_keep_the_last_on_rate_limits() {
        use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get as route};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let api = crate::testkit::serve(Router::new().route(
            "/repos/acme-test/notes/releases",
            route(move || {
                let n = h.fetch_add(1, Ordering::SeqCst);
                async move {
                    match n {
                        0 => (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response(),
                        1 => Json(serde_json::json!([
                            {"tag_name": "v1.1.0", "name": "", "published_at": "2026-10-01T00:00:00Z",
                             "body": "Fixes CVE-2026-1234 <b>now</b>", "draft": false, "prerelease": false},
                            {"tag_name": "v1.2.0-rc.1", "prerelease": true},
                            {"tag_name": "v9.0.0", "draft": true},
                            {"name": "no tag"}
                        ]))
                        .into_response(),
                        _ => (StatusCode::FORBIDDEN, "API rate limit exceeded").into_response(),
                    }
                }
            }),
        ))
        .await;
        let repo = "acme-test/notes";
        // nothing yet: no notes, the error
        let (n, e) = release_notes(&api, repo, None).await;
        assert!(n.is_empty());
        assert_eq!(e.as_deref(), Some("HTTP 429 Too Many Requests: slow down"));
        // not asked again before the failure is 30 minutes old
        let (n, e) = release_notes(&api, repo, None).await;
        assert!(n.is_empty() && e.is_some());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let due = |repo: &str| NOTES.lock().unwrap().get_mut(repo).unwrap().until = Instant::now();
        // then retried, and the notes cached
        due(repo);
        for _ in 0..2 {
            let (n, e) = release_notes(&api, repo, Some("t")).await;
            assert_eq!(e, None);
            assert_eq!(
                n,
                [Note {
                    tag: "v1.1.0".into(),
                    name: "v1.1.0".into(),
                    published: Some("2026-10-01T00:00:00Z".into()),
                    url: "https://github.com/acme-test/notes/releases/tag/v1.1.0".into(),
                    excerpt: "Fixes CVE-2026-1234 now".into(),
                    security: true,
                }]
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        // 12 hours later: a 403 keeps the last notes, and is not retried
        // at once
        due(repo);
        for _ in 0..2 {
            let (n, e) = release_notes(&api, repo, None).await;
            assert_eq!(n.len(), 1);
            assert!(e.unwrap().starts_with("HTTP 403"));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn chart_sources_from_the_index() {
        let index = "apiVersion: v1\nentries:\n  a:\n  - version: 1.1.0\n  - version: 1.0.0\n    home: https://a.example\n    sources: [https://github.com/acme/a]\n  b:\n  - version: 1.0.0\n    home: https://b.example\n  c:\n  - version: 1.0.0\n    sources: null\n";
        let url = crate::testkit::serve(axum::Router::new().route(
            "/index.yaml",
            axum::routing::get(move || async move { index }),
        ))
        .await;
        let src = async |chart: &str, version: &str| {
            chart_source(&url, chart, version, |_| None).await.unwrap()
        };
        assert_eq!(
            src("a", "1.0.0").await.as_deref(),
            Some("https://github.com/acme/a")
        );
        // a version without metadata: another version's
        assert_eq!(
            src("a", "1.1.0").await.as_deref(),
            Some("https://github.com/acme/a")
        );
        assert_eq!(
            src("b", "1.0.0").await.as_deref(),
            Some("https://b.example")
        );
        assert_eq!(src("c", "1.0.0").await, None);
        assert_eq!(helm_index(&url, "a").await.unwrap(), ["1.1.0", "1.0.0"]);
        let e = chart_source("oci://charts.test", "charts.test", "1", |_| None)
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "oci:// URL without a repository");
    }

    /// A fake registry for `oci_source`: annotations, an index behind a
    /// token, a blob on another host, a Helm chart config, nothing.
    #[tokio::test]
    async fn oci_sources_from_annotations_labels_and_chart_config() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode, header},
            response::IntoResponse,
            routing::get as route,
        };
        use serde_json::json;
        // the "CDN": another host name for the same loopback address
        let cdn = crate::testkit::serve(Router::new().route(
            "/blob/c",
            route(|h: HeaderMap| async move {
                if h.contains_key(header::AUTHORIZATION) {
                    return (StatusCode::BAD_REQUEST, "credentials leaked").into_response();
                }
                Json(json!({"config": {"Labels": {SOURCE: "https://github.com/acme/idx"}}}))
                    .into_response()
            }),
        ))
        .await
        .replace("127.0.0.1", "localhost");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let realm = format!("{base}/token");
        let manifest = |v: serde_json::Value| {
            move || {
                let v = v.clone();
                async move { Json(v) }
            }
        };
        let app = Router::new()
            .route(
                "/v2/ann/app/manifests/1.0.0",
                route(manifest(json!({"annotations": {SOURCE: "https://github.com/acme/ann"}}))),
            )
            .route(
                "/v2/idx/app/manifests/{r}",
                route(move |axum::extract::Path(r): axum::extract::Path<String>, h: HeaderMap| {
                    let realm = realm.clone();
                    async move {
                        if h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer t") {
                            let c = format!(r#"Bearer realm="{realm}""#);
                            return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, c)]).into_response();
                        }
                        Json(match r.as_str() {
                            "2.0.0" => json!({"manifests": [
                                {"digest": "sha256:a", "platform": {"os": "linux", "architecture": "arm64"}},
                                {"digest": "sha256:b", "platform": {"os": "linux", "architecture": "amd64"}}]}),
                            _ => json!({"config": {"digest": "sha256:c"}}),
                        })
                        .into_response()
                    }
                }),
            )
            .route(
                "/v2/idx/app/blobs/sha256:c",
                route(move || {
                    let to = format!("{cdn}/blob/c");
                    async move { (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, to)]) }
                }),
            )
            .route("/token", route(|| async { Json(json!({"token": "t"})) }))
            .route(
                "/v2/chart/app/manifests/1.0.0",
                route(manifest(json!({"config": {"digest": "sha256:h"}}))),
            )
            .route(
                "/v2/chart/app/blobs/sha256:h",
                route(manifest(json!({"name": "app", "home": "https://app.example",
                                      "sources": ["https://github.com/acme/chart"]}))),
            )
            .route("/v2/none/app/manifests/1.0.0", route(manifest(json!({}))))
            // a label inherited from the base image
            .route(
                "/v2/external-dns/external-dns/manifests/1.0.0",
                route(manifest(json!({"config": {"digest": "sha256:d"}}))),
            )
            .route(
                "/v2/external-dns/external-dns/blobs/sha256:d",
                route(manifest(
                    json!({"config": {"Labels": {SOURCE: "https://github.com/GoogleContainerTools/distroless"}}}),
                )),
            )
            .route(
                "/v2/bad/app/manifests/1.0.0",
                route(manifest(json!({"manifests": [{"digest": "../../x"}]}))),
            )
            .route(
                "/v2/listed/app/manifests/1.0.0",
                route(manifest(json!({"manifests": [], "annotations": {SOURCE: "https://github.com/acme/listed"}}))),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let src = async |name: &str, tag: &str| oci_source(&base, name, tag, None).await;
        for (name, tag, want) in [
            ("ann/app", "1.0.0", Some("https://github.com/acme/ann")),
            ("idx/app", "2.0.0", Some("https://github.com/acme/idx")),
            ("chart/app", "1.0.0", Some("https://github.com/acme/chart")),
            (
                "listed/app",
                "1.0.0",
                Some("https://github.com/acme/listed"),
            ),
            ("none/app", "1.0.0", None),
            ("external-dns/external-dns", "1.0.0", None),
        ] {
            assert_eq!(src(name, tag).await.unwrap().as_deref(), want, "{name}");
        }
        let e = src("bad/app", "1.0.0").await.unwrap_err();
        assert_eq!(e.to_string(), "bad reference \"../../x\"");
        // oci:// chart repositories are https: this plain server fails
        let repo = format!("oci://{}/chart", base.trim_start_matches("http://"));
        assert!(chart_source(&repo, "app", "1.0.0", |_| None).await.is_err());
    }
}

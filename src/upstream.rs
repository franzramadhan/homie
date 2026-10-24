//! Upstream versions (RFC 0020): what is the newest release of something,
//! through a few generic sources (GitHub releases, the OCI distribution API,
//! Helm repository indexes, dl.k8s.io). Every lookup is opt-in by config and
//! cached for 12 hours.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
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
            !v.prerelease() && v.nums.len() == inst.nums.len() && v.build().0 == inst_build
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

/// `key`'s list from the cache, else `fetch` (failures cached shorter).
async fn cached<F, Fut>(key: &str, fetch: F) -> Result<Vec<String>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>>>,
{
    if let Some((at, r)) = CACHE.lock().unwrap().get(key)
        && at.elapsed() < if r.is_ok() { TTL } else { FAILED_TTL }
    {
        return r.clone().map_err(Into::into);
    }
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

fn tls() -> Tls {
    Tls {
        tls_verify: true,
        ca_file: None,
        tls_server_name: None,
    }
}

/// GET with up to three redirects (dl.k8s.io answers with one).
async fn get(url: &str, headers: &[(&str, String)]) -> Result<reqwest::Response> {
    let mut url = url.to_string();
    for _ in 0..4 {
        let t = http::target(&url, &tls()).await?;
        let mut req = t.client.get(t.base.clone());
        for (k, v) in headers {
            req = req.header(*k, v);
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

/// Tags of an OCI repository (`registry`, `name`), anonymous or with basic
/// credentials, through the registry's bearer-token challenge.
pub async fn oci_tags(
    registry: &str,
    name: &str,
    creds: Option<(String, String)>,
) -> Result<Vec<String>> {
    // a registry given with http:// is used as is (an insecure registry)
    let base = match registry {
        "docker.io" | "index.docker.io" => "https://registry-1.docker.io".to_string(),
        r if r.starts_with("http://") || r.starts_with("https://") => {
            r.trim_end_matches('/').to_string()
        }
        r => format!("https://{r}"),
    };
    let key = format!("oci:{base}/{name}");
    cached(&key, || async move {
        let basic = creds.as_ref().map(|(u, p)| {
            use base64::Engine;
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))
            )
        });
        let mut auth = basic.clone();
        let mut url = format!("{base}/v2/{name}/tags/list?n=1000");
        let mut tags = Vec::new();
        for _ in 0..10 {
            let headers: Vec<(&str, String)> =
                auth.iter().map(|a| ("Authorization", a.clone())).collect();
            let mut resp = get(&url, &headers).await?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
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
                let th: Vec<(&str, String)> =
                    basic.iter().map(|a| ("Authorization", a.clone())).collect();
                let t: Value = http::ok(get(token_url.as_str(), &th).await?)
                    .await?
                    .json()
                    .await?;
                let token = t["token"]
                    .as_str()
                    .or(t["access_token"].as_str())
                    .ok_or("registry gave no token")?;
                auth = Some(format!("Bearer {token}"));
                let headers = vec![("Authorization", auth.clone().unwrap())];
                resp = get(&url, &headers).await?;
            }
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

/// Versions of `chart` in a Helm HTTP repository's `index.yaml`.
pub async fn helm_index(repo: &str, chart: &str) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Index {
        entries: HashMap<String, Vec<Entry>>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        version: String,
    }
    let repo = repo.trim_end_matches('/').to_string();
    // the whole index is one download: cache every chart of it at once
    let key = format!("helm:{repo}");
    let all = cached(&key, || async {
        let body = http::ok(get(&format!("{repo}/index.yaml"), &[]).await?)
            .await?
            .bytes()
            .await?;
        let index: Index = serde_yaml_ng::from_slice(&body)?;
        Ok(index
            .entries
            .into_iter()
            .flat_map(|(name, es)| {
                es.into_iter()
                    .map(move |e| format!("{name}\u{0}{}", e.version))
            })
            .collect())
    })
    .await?;
    Ok(all
        .iter()
        .filter_map(|e| e.split_once('\u{0}'))
        .filter(|(n, _)| *n == chart)
        .map(|(_, v)| v.to_string())
        .collect())
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
            let rest = rest.trim_end_matches('/');
            let full = if rest.rsplit('/').next() == Some(chart) {
                rest.to_string()
            } else {
                format!("{rest}/{chart}")
            };
            let (registry, name) = full
                .split_once('/')
                .ok_or("oci:// URL without a repository")?;
            oci_tags(registry, name, creds(registry)).await
        }
        None => helm_index(repo, chart).await,
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
        let c = s(&["2.9.1", "2.10.0", "2.10.0-beta.1"]);
        assert_eq!(latest("2.9.0", &c).newest.as_deref(), Some("2.10.0"));
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
}

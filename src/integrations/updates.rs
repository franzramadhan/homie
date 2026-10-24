//! Pending updates (RFC 0020): what is installed, read from the lab, and
//! (opt-in per kind) the newest upstream versions. Read-only: nothing here
//! installs, refreshes or bumps anything.

use std::collections::{BTreeMap, HashSet};

use k8s_openapi::api::core::v1::{Namespace, Node};
use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ListParams},
    core::GroupVersionKind,
};
use serde::Serialize;
use serde_json::Value;

use super::{helm, kubernetes::Kube, proxmox::Proxmox};
use crate::{
    Result,
    config::{Distro, Updates},
    upstream::{self, Latest},
};

#[derive(Debug, Serialize, Default)]
pub struct Report {
    pub proxmox: Vec<PveNode>,
    pub kubernetes: Vec<Cluster>,
    pub charts: Vec<Chart>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Package {
    package: String,
    installed: String,
    new: String,
}

#[derive(Debug, Serialize)]
pub struct PveNode {
    cluster: String,
    node: String,
    updates: usize,
    packages: Vec<Package>,
    running_kernel: String,
    newest_kernel: Option<String>,
    reboot: bool,
}

#[derive(Debug, Serialize)]
pub struct NodeVersion {
    name: String,
    kubelet: String,
}

#[derive(Debug, Serialize)]
pub struct Cluster {
    cluster: String,
    server: String,
    nodes: Vec<NodeVersion>,
    skew: bool,
    distro: String,
    distro_version: String,
    kubernetes_latest: Latest,
    distro_latest: Latest,
    flux: Option<String>,
    flux_latest: Latest,
}

#[derive(Debug, Serialize)]
pub struct Chart {
    cluster: String,
    namespace: String,
    release: String,
    chart: String,
    version: String,
    /// `flux`, `argocd` or `helm`
    managed_by: String,
    /// repository URL (`oci://...` or an HTTP Helm repository)
    source: Option<String>,
    latest: Latest,
}

/// `7.0.14-25` of `7.0.14-25-pve` as comparable numbers.
fn kernel_key(v: &str) -> Vec<u64> {
    v.trim_end_matches("-pve")
        .split(['.', '-'])
        .map_while(|p| p.parse().ok())
        .collect()
}

/// The newest installed kernel from `apt/versions`: packages
/// `proxmox-kernel-<ver>-pve[-signed]` (PVE 8, 9) or `pve-kernel-<ver>-pve`.
fn newest_kernel(versions: &Value) -> Option<String> {
    versions
        .as_array()?
        .iter()
        .filter(|p| p["CurrentState"] == "Installed")
        .filter_map(|p| {
            let name = p["Package"].as_str()?;
            let rest = name
                .strip_prefix("proxmox-kernel-")
                .or_else(|| name.strip_prefix("pve-kernel-"))?;
            let ver = rest
                .strip_suffix("-pve-signed")
                .or_else(|| rest.strip_suffix("-pve"))?;
            (kernel_key(ver).len() >= 4).then(|| ver.to_string())
        })
        .max_by_key(|v| kernel_key(v))
}

fn node_updates(cluster: &str, node: &str, update: &Value, versions: &Value) -> PveNode {
    let packages: Vec<Package> = update
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Package {
            package: p["Package"].as_str().unwrap_or_default().into(),
            installed: p["OldVersion"].as_str().unwrap_or_default().into(),
            new: p["Version"].as_str().unwrap_or_default().into(),
        })
        .collect();
    let running = versions
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["Package"] == "proxmox-ve")
        .and_then(|p| p["RunningKernel"].as_str())
        .unwrap_or_default()
        .to_string();
    let newest = newest_kernel(versions);
    PveNode {
        cluster: cluster.into(),
        node: node.into(),
        updates: packages.len(),
        packages,
        reboot: newest
            .as_deref()
            .is_some_and(|n| !running.is_empty() && kernel_key(n) > kernel_key(&running)),
        running_kernel: running,
        newest_kernel: newest,
    }
}

/// Package updates and reboot pending per online node. The lists are what
/// the node's daily `pveupdate` found: Homie never refreshes them.
pub async fn proxmox(p: &Proxmox) -> Result<Vec<PveNode>> {
    let status = p.get("/cluster/status").await?;
    let nodes: Vec<String> = status
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "node" && e["online"].as_u64() != Some(0))
        .filter_map(|e| e["name"].as_str().map(String::from))
        .collect();
    let lists = futures::future::join_all(nodes.iter().map(|n| async move {
        let (update, versions) = (
            format!("/nodes/{n}/apt/update"),
            format!("/nodes/{n}/apt/versions"),
        );
        let (u, v) = tokio::join!(p.get(&update), p.get(&versions));
        (n, u, v)
    }))
    .await;
    lists
        .into_iter()
        .map(|(n, u, v)| Ok(node_updates(&p.name, n, &u?, &v?)))
        .collect()
}

/// Distribution, its installed version, and where its releases are. A new
/// distribution is one arm here.
fn detect(
    git: &str,
    os_images: &[String],
    providers: &[String],
    over: &Distro,
) -> (String, String, Option<String>) {
    let plain = upstream::parse(git)
        .map(|v| {
            format!(
                "v{}",
                v.nums
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(".")
            )
        })
        .unwrap_or_else(|| git.to_string());
    let talos = os_images.iter().find_map(|o| {
        o.strip_prefix("Talos (")
            .and_then(|r| r.strip_suffix(')'))
            .map(String::from)
    });
    let provider = |p: &str| providers.iter().any(|x| x.starts_with(p));
    let name = match over {
        Distro::GitHub { github } => return ("custom".into(), git.into(), Some(github.clone())),
        Distro::Name(n) if n != "auto" => n.clone(),
        _ if git.contains("+k0s") => "k0s".into(),
        _ if git.contains("+k3s") => "k3s".into(),
        _ if git.contains("+rke2") => "rke2".into(),
        _ if talos.is_some() => "talos".into(),
        _ if git.contains("-eks-") || provider("aws://") => "eks".into(),
        _ if git.contains("-gke.") || provider("gce://") => "gke".into(),
        _ if provider("azure://") => "aks".into(),
        _ => "kubernetes".into(),
    };
    match name.as_str() {
        "k0s" => (name, git.into(), Some("k0sproject/k0s".into())),
        "k3s" => (name, git.into(), Some("k3s-io/k3s".into())),
        "rke2" => (name, git.into(), Some("rancher/rke2".into())),
        "talos" => (
            name,
            talos.unwrap_or_default(),
            Some("siderolabs/talos".into()),
        ),
        "none" => (String::new(), String::new(), None),
        // managed clusters are upgraded by the provider; plain ones are the
        // Kubernetes version itself
        _ => (name, plain, None),
    }
}

pub async fn cluster(
    k: &Kube,
    client: Client,
    cfg: &Updates,
    gh: Option<&str>,
    errors: &mut Vec<String>,
) -> Result<Cluster> {
    let server = client.apiserver_version().await?.git_version;
    let nodes = Api::<Node>::all(client.clone())
        .list(&ListParams::default())
        .await?;
    let mut versions = Vec::new();
    let (mut os, mut providers) = (Vec::new(), Vec::new());
    for n in &nodes.items {
        let info = n.status.as_ref().and_then(|s| s.node_info.as_ref());
        versions.push(NodeVersion {
            name: n.metadata.name.clone().unwrap_or_default(),
            kubelet: info.map(|i| i.kubelet_version.clone()).unwrap_or_default(),
        });
        os.extend(info.map(|i| i.os_image.clone()));
        providers.extend(n.spec.as_ref().and_then(|s| s.provider_id.clone()));
    }
    let skew = versions
        .iter()
        .map(|v| v.kubelet.as_str())
        .collect::<HashSet<_>>()
        .len()
        > 1;
    let (distro, distro_version, repo) = detect(&server, &os, &providers, &k.distro);
    let flux = Api::<Namespace>::all(client)
        .get_opt(&k.flux_namespace)
        .await
        .ok()
        .flatten()
        .and_then(|ns| {
            ns.metadata
                .labels?
                .get("app.kubernetes.io/version")
                .cloned()
        });
    let up = &cfg.upstream;
    let mut note = |what: &str, r: Result<Latest>| match r {
        Ok(l) => l,
        Err(e) => {
            errors.push(format!("{}: {what}: {e}", k.name));
            Latest::default()
        }
    };
    let kubernetes_latest = if up.kubernetes {
        note("kubernetes", upstream::kubernetes(&server).await)
    } else {
        Latest::default()
    };
    let distro_latest = match (&repo, up.distro) {
        (Some(r), true) => note(
            "distribution",
            upstream::github_releases(r, gh)
                .await
                .map(|t| upstream::latest(&distro_version, &t)),
        ),
        _ => Latest::default(),
    };
    let flux_latest = match (&flux, up.flux) {
        (Some(v), true) => note(
            "flux",
            upstream::github_releases("fluxcd/flux2", gh)
                .await
                .map(|t| upstream::latest(v, &t)),
        ),
        _ => Latest::default(),
    };
    Ok(Cluster {
        cluster: k.name.clone(),
        server,
        nodes: versions,
        skew,
        distro,
        distro_version,
        kubernetes_latest,
        distro_latest,
        flux,
        flux_latest,
    })
}

async fn list(
    client: &Client,
    group: &str,
    version: &str,
    kind: &str,
) -> Result<Vec<DynamicObject>> {
    let ar = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
    match Api::<DynamicObject>::all_with(client.clone(), &ar)
        .list(&ListParams::default())
        .await
    {
        Ok(l) => Ok(l.items),
        Err(kube::Error::Api(s)) if s.code == 404 => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn ns(o: &DynamicObject) -> String {
    o.metadata.namespace.clone().unwrap_or_default()
}

/// `*` matches any run of characters.
pub fn glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == s;
    }
    let mut rest = s;
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(p) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(p);
        } else {
            let Some(at) = rest.find(p) else { return false };
            rest = &rest[at + p.len()..];
        }
    }
    true
}

/// Charts deployed in a cluster, with their repository when it is known:
/// Flux HelmReleases (through their HelmRepository or OCIRepository), Argo
/// CD Applications with a Helm source, and plain releases from `charts`.
pub async fn charts(cluster: &str, client: Client, cfg: &Updates) -> Result<Vec<Chart>> {
    const SOURCE: &str = "source.toolkit.fluxcd.io";
    let (hrs, repos, ocis, apps) = tokio::join!(
        list(&client, "helm.toolkit.fluxcd.io", "v2", "HelmRelease"),
        list(&client, SOURCE, "v1", "HelmRepository"),
        list(&client, SOURCE, "v1", "OCIRepository"),
        list(&client, "argoproj.io", "v1alpha1", "Application"),
    );
    let (hrs, repos, ocis, apps) = (hrs?, repos?, ocis?, apps?);
    let url_of = |objs: &[DynamicObject], name: &str, namespace: &str| {
        objs.iter()
            .find(|o| o.metadata.name.as_deref() == Some(name) && ns(o) == namespace)
            .map(|o| text(&o.data["spec"]["url"]))
    };
    let mut out = Vec::new();
    let mut flux_releases = HashSet::new();
    for hr in &hrs {
        let spec = &hr.data["spec"];
        let hist = &hr.data["status"]["history"][0];
        let chart_spec = &spec["chart"]["spec"];
        let chart = text(&hist["chartName"]).to_string();
        let chart = if chart.is_empty() {
            text(&chart_spec["chart"])
        } else {
            chart
        };
        let release_ns = text(&hist["namespace"]);
        let release = text(&hist["name"]);
        let (release_ns, release) = (
            if release_ns.is_empty() {
                ns(hr)
            } else {
                release_ns
            },
            if release.is_empty() {
                hr.metadata.name.clone().unwrap_or_default()
            } else {
                release
            },
        );
        flux_releases.insert((release_ns.clone(), release.clone()));
        let refns = |r: &Value| {
            r["namespace"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| ns(hr))
        };
        let source = match (&chart_spec["sourceRef"], &spec["chartRef"]) {
            (r, _) if r["kind"] == "HelmRepository" => url_of(&repos, &text(&r["name"]), &refns(r)),
            (_, r) if r["kind"] == "OCIRepository" => url_of(&ocis, &text(&r["name"]), &refns(r)),
            _ => None,
        };
        out.push(Chart {
            cluster: cluster.into(),
            namespace: release_ns,
            release,
            chart,
            version: text(&hist["chartVersion"]),
            managed_by: "flux".into(),
            source,
            latest: Latest::default(),
        });
    }
    for app in &apps {
        let spec = &app.data["spec"];
        let sources = std::iter::once(&spec["source"])
            .chain(spec["sources"].as_array().into_iter().flatten());
        for s in sources.filter(|s| s["chart"].is_string()) {
            let repo = text(&s["repoURL"]);
            // Argo CD names OCI Helm repositories without a scheme
            let repo = if repo.contains("://") {
                repo
            } else {
                format!("oci://{repo}")
            };
            out.push(Chart {
                cluster: cluster.into(),
                namespace: text(&spec["destination"]["namespace"]),
                release: app.metadata.name.clone().unwrap_or_default(),
                chart: text(&s["chart"]),
                version: text(&s["targetRevision"]),
                managed_by: "argocd".into(),
                source: Some(repo),
                latest: Latest::default(),
            });
        }
    }
    if !cfg.charts.is_empty() {
        for r in helm::releases(client).await? {
            if flux_releases.contains(&(r.namespace.clone(), r.name.clone())) {
                continue;
            }
            if let Some(m) = cfg.charts.iter().find(|m| m.name == r.chart) {
                out.push(Chart {
                    cluster: cluster.into(),
                    namespace: r.namespace,
                    release: r.name,
                    chart: r.chart,
                    version: r.chart_version,
                    managed_by: "helm".into(),
                    source: Some(m.repo.clone()),
                    latest: Latest::default(),
                });
            }
        }
    }
    out.retain(|c| !cfg.ignore.iter().any(|g| glob(g, &c.chart)));
    Ok(out)
}

/// The newest versions of each chart with a known repository, one lookup
/// per repository and chart.
pub async fn chart_latest(
    charts: &mut [Chart],
    creds: &BTreeMap<String, (String, String)>,
    errors: &mut Vec<String>,
) {
    let mut done: BTreeMap<(String, String), std::result::Result<Vec<String>, String>> =
        BTreeMap::new();
    for c in charts.iter_mut() {
        let Some(repo) = c.source.clone() else {
            continue;
        };
        let key = (repo.clone(), c.chart.clone());
        if !done.contains_key(&key) {
            let r =
                upstream::chart_versions(&repo, &c.chart, |host| creds.get(host).cloned()).await;
            if let Err(e) = &r {
                errors.push(format!("{}: chart {} from {repo}: {e}", c.cluster, c.chart));
            }
            done.insert(key.clone(), r.map_err(|e| e.to_string()));
        }
        if let Some(Ok(tags)) = done.get(&key) {
            // build metadata has no precedence (semver); Flux adds
            // +<digest> to charts from OCI repositories
            let plain = |v: &str| v.split('+').next().unwrap_or(v).to_string();
            let tags: Vec<String> = tags.iter().map(|t| plain(t)).collect();
            c.latest = upstream::latest(&plain(&c.version), &tags);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{json as route, kube_client, list as klist, serve};
    use serde_json::json;

    #[test]
    fn kernels_and_reboot() {
        let versions = json!([
            {"Package": "proxmox-ve", "RunningKernel": "7.0.14-23-pve", "CurrentState": "Installed"},
            {"Package": "proxmox-kernel-7.0.14-25-pve-signed", "CurrentState": "Installed"},
            {"Package": "proxmox-kernel-7.0.14-23-pve-signed", "CurrentState": "Installed"},
            {"Package": "proxmox-kernel-6.8.12-43-pve-signed", "CurrentState": "Installed"},
            {"Package": "proxmox-kernel-7.1.0-1-pve-signed", "CurrentState": "ConfigFiles"},
            {"Package": "proxmox-kernel-helper", "CurrentState": "Installed"},
            {"Package": "proxmox-kernel-7.0", "CurrentState": "Installed"},
        ]);
        let update =
            json!([{"Package": "bind9-host", "OldVersion": "1:9.20.29", "Version": "1:9.20.30"}]);
        let n = node_updates("lab", "pve-01", &update, &versions);
        assert_eq!(n.newest_kernel.as_deref(), Some("7.0.14-25"));
        assert_eq!(n.running_kernel, "7.0.14-23-pve");
        assert!(n.reboot);
        assert_eq!(n.updates, 1);
        assert_eq!(
            n.packages[0],
            Package {
                package: "bind9-host".into(),
                installed: "1:9.20.29".into(),
                new: "1:9.20.30".into()
            }
        );
        let mut current = versions.clone();
        current[0]["RunningKernel"] = json!("7.0.14-25-pve");
        assert!(!node_updates("lab", "pve-01", &json!([]), &current).reboot);
        // PVE 7 names
        let old = json!([{"Package": "pve-kernel-5.15.158-2-pve", "CurrentState": "Installed"}]);
        assert_eq!(newest_kernel(&old).as_deref(), Some("5.15.158-2"));
    }

    #[test]
    fn distributions() {
        let none: Vec<String> = vec![];
        let auto = Distro::default();
        assert_eq!(
            detect("v1.36.1+k0s", &none, &none, &auto),
            (
                "k0s".into(),
                "v1.36.1+k0s".into(),
                Some("k0sproject/k0s".into())
            )
        );
        assert_eq!(
            detect("v1.31.4+k3s1", &none, &none, &auto).2.as_deref(),
            Some("k3s-io/k3s")
        );
        assert_eq!(detect("v1.31.4+rke2r1", &none, &none, &auto).0, "rke2");
        let talos = vec!["Talos (v1.9.1)".to_string()];
        assert_eq!(
            detect("v1.32.0", &talos, &none, &auto),
            (
                "talos".into(),
                "v1.9.1".into(),
                Some("siderolabs/talos".into())
            )
        );
        assert_eq!(
            detect("v1.31.4-eks-2d5f260", &none, &none, &auto),
            ("eks".into(), "v1.31.4".into(), None)
        );
        assert_eq!(
            detect("v1.31.4", &none, &["azure:///x".to_string()], &auto).0,
            "aks"
        );
        assert_eq!(
            detect("v1.31.4", &none, &none, &auto),
            ("kubernetes".into(), "v1.31.4".into(), None)
        );
        let gh = Distro::GitHub {
            github: "acme/k8s".into(),
        };
        assert_eq!(
            detect("v1.31.4+acme.2", &none, &none, &gh).2.as_deref(),
            Some("acme/k8s")
        );
        assert_eq!(
            detect("v1.36.1+k0s", &none, &none, &Distro::Name("none".into())).0,
            ""
        );
    }

    #[test]
    fn globs() {
        assert!(glob("registry.k8s.io/pause*", "registry.k8s.io/pause:3.10"));
        assert!(glob("*-crds", "cert-manager-crds"));
        assert!(glob("a*b*c", "a-x-b-y-c"));
        assert!(!glob("a*b*c", "a-x-c"));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
    }

    #[tokio::test]
    async fn charts_from_flux_argo_and_plain_helm() {
        let hr = json!([
            {"metadata": {"namespace": "flux-system", "name": "cilium"},
             "spec": {"chart": {"spec": {"chart": "cilium", "sourceRef": {"kind": "HelmRepository", "name": "cilium"}}}},
             "status": {"history": [{"chartName": "cilium", "chartVersion": "1.20.2", "name": "cilium", "namespace": "kube-system"}]}},
            {"metadata": {"namespace": "apps", "name": "podinfo"},
             "spec": {"chartRef": {"kind": "OCIRepository", "name": "podinfo"}},
             "status": {"history": [{"chartName": "podinfo", "chartVersion": "6.7.0", "name": "podinfo", "namespace": "apps"}]}}]);
        let repos = json!([{"metadata": {"namespace": "flux-system", "name": "cilium"}, "spec": {"url": "https://helm.cilium.io"}}]);
        let ocis = json!([{"metadata": {"namespace": "apps", "name": "podinfo"}, "spec": {"url": "oci://ghcr.io/stefanprodan/charts/podinfo"}}]);
        let apps = json!([{"metadata": {"namespace": "argocd", "name": "redis"},
            "spec": {"destination": {"namespace": "cache"},
                     "source": {"repoURL": "registry-1.docker.io/bitnamicharts", "chart": "redis", "targetRevision": "20.1.0"}}}]);
        let url = serve(
            route(
                "/apis/helm.toolkit.fluxcd.io/v2/helmreleases",
                klist("HelmRelease", hr),
            )
            .merge(route(
                "/apis/source.toolkit.fluxcd.io/v1/helmrepositories",
                klist("HelmRepository", repos),
            ))
            .merge(route(
                "/apis/source.toolkit.fluxcd.io/v1/ocirepositories",
                klist("OCIRepository", ocis),
            ))
            .merge(route(
                "/apis/argoproj.io/v1alpha1/applications",
                klist("Application", apps),
            )),
        )
        .await;
        let cfg = Updates {
            ignore: vec!["*-crds".into()],
            ..Default::default()
        };
        let mut got = charts("lab", kube_client(&url), &cfg).await.unwrap();
        got.sort_by(|a, b| a.chart.cmp(&b.chart));
        let rows: Vec<_> = got
            .iter()
            .map(|c| {
                (
                    c.chart.as_str(),
                    c.version.as_str(),
                    c.namespace.as_str(),
                    c.managed_by.as_str(),
                    c.source.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "cilium",
                    "1.20.2",
                    "kube-system",
                    "flux",
                    Some("https://helm.cilium.io")
                ),
                (
                    "podinfo",
                    "6.7.0",
                    "apps",
                    "flux",
                    Some("oci://ghcr.io/stefanprodan/charts/podinfo")
                ),
                (
                    "redis",
                    "20.1.0",
                    "cache",
                    "argocd",
                    Some("oci://registry-1.docker.io/bitnamicharts")
                ),
            ]
        );
    }

    #[tokio::test]
    async fn helm_index_versions() {
        let index = "apiVersion: v1\nentries:\n  cilium:\n  - version: 1.20.3\n  - version: 1.21.0\n  - version: 1.22.0-rc.1\n  other:\n  - version: 9.9.9\n";
        let app = axum::Router::new().route(
            "/index.yaml",
            axum::routing::get(move || async move { index }),
        );
        let url = serve(app).await;
        let mut c = vec![Chart {
            cluster: "lab".into(),
            namespace: "kube-system".into(),
            release: "cilium".into(),
            chart: "cilium".into(),
            version: "1.20.2".into(),
            managed_by: "flux".into(),
            source: Some(url.clone()),
            latest: Latest::default(),
        }];
        let mut errors = Vec::new();
        chart_latest(&mut c, &BTreeMap::new(), &mut errors).await;
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            c[0].latest,
            Latest {
                patch: Some("1.20.3".into()),
                newest: Some("1.21.0".into())
            }
        );
        // a Flux OCI chart version carries +<digest>
        c[0].version = "1.20.2+bdfdad17cf8e".into();
        c[0].latest = Latest::default();
        chart_latest(&mut c, &BTreeMap::new(), &mut errors).await;
        assert_eq!(c[0].latest.newest.as_deref(), Some("1.21.0"));
    }
}

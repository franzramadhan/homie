# 0020: Pending updates

| Status | Date | PR | Release |
|---|---|---|---|
| implemented | 2026-10-24 | #82 | v0.10.0 |

> Show what is out of date: package updates and pending reboots on Proxmox
> nodes, the Kubernetes distribution, Flux and Helm charts, each against
> the newest stable release. Read-only: Homie never installs or bumps
> anything. Container images follow in RFC 0021 on the same clients.

## Problem

Updates in a homelab are remembered, not tracked. Proxmox lists pending
packages per node in its UI, a new kernel waits for a reboot nobody did,
and whether the cluster or its charts are behind means checking release
pages by hand. Renovate covers what is in git, not what is actually
running (a `helm install` from a laptop, the distribution itself).

## Proposal

Two layers: **installed versions** (read from the lab, no outbound calls)
and **upstream lookups** (opt-in per kind, through generic clients).

### Installed versions

- **Proxmox**:
  - `GET /nodes/{node}/apt/update`: the packages the node's daily
    `pveupdate` run found. Count per node, the list in a row expander
    (package, installed, new). Homie never refreshes the list (`POST`
    needs `Sys.Modify`); `PVEAuditor` is enough. Security updates are not
    marked separately (see Alternatives).
  - Reboot pending: the running kernel (`/nodes/{node}/status`) is older
    than the newest installed one (`/nodes/{node}/apt/versions`). Amber,
    "reboot pending".
- **Kubernetes**: API server version (`/version`) and each node's
  `status.nodeInfo.kubeletVersion`. Nodes on different versions are amber
  ("version skew").
- **Distribution**, detected per cluster from the version strings and
  node info. Built-in detectors, each one row in a table in the code:

  | Distribution | Detected from | Installed version | Upstream |
  |---|---|---|---|
  | k0s | `+k0s` in `gitVersion` | Kubernetes part, `+k0s.N` matched to releases | GitHub `k0sproject/k0s` |
  | k3s | `+k3sN` in `gitVersion` | the tag itself (`v1.36.1+k3s1`) | GitHub `k3s-io/k3s` |
  | RKE2 | `+rke2rN` in `gitVersion` | the tag itself | GitHub `rancher/rke2` |
  | Talos | `osImage` `Talos (vX.Y.Z)` | from `osImage` | GitHub `siderolabs/talos` |
  | Managed (EKS, GKE, AKS) | `-eks-`, `-gke.`, provider ID | Kubernetes part only | none: the provider upgrades |
  | Anything else | - | Kubernetes part only | `dl.k8s.io` stable patch |

  `kubernetes.<name>.distro` overrides detection: `"none"`, a built-in
  name, or `{ github = "owner/repo" }` for a distribution without a
  detector. A new distribution is one row, nothing else changes.
- **Flux**: the `app.kubernetes.io/version` label on the Flux namespace.
- **Helm charts**: chart name and version from the Helm release Secrets
  (already read for the Helm view). The chart's source:
  - Flux HelmReleases: the `HelmRepository` URL (`https://` index or
    `oci://`) or the `OCIRepository` already resolved by the Flux
    integration through `chart.spec.sourceRef` and `chartRef`.
  - Argo CD Applications with a Helm source: `repoURL` and `chart`.
  - Plain `helm install` releases: the repository is not recorded by
    Helm, so `[[updates.charts]]` maps a chart name to a repository URL.
    Unmapped charts show the installed version only.

### Upstream lookups

Generic clients serve every kind; a kind only maps "what is installed" to
"which repository to ask". RFC 0021 adds images on the same clients.

- **GitHub releases**: `GET /repos/{owner}/{repo}/releases`, drafts and
  pre-releases dropped. Unauthenticated (60 requests an hour, enough with
  the cache), or with `updates.github_token`.
- **OCI distribution API** (any registry: GHCR, Docker Hub, Quay, Harbor,
  Zot, ...): `GET /v2/<name>/tags/list`, with the anonymous bearer-token
  challenge every registry speaks. Credentials per registry host in
  `[[updates.registries]]`. Used here for OCI Helm charts.
- **Helm HTTP repositories**: `index.yaml`, the chart's entries only.

Version comparison, the same for every kind: versions parse as dotted
numbers with an optional `v` prefix; pre-release suffixes (`-rc.1`,
`-beta`) are skipped. Both the newest patch of the installed minor and the
newest overall are shown ("1.27.5 / 1.29.0 available").

- **Cache and limits**: results cached 12 h per repository (snapshot
  included), at most 4 lookups at a time, one lookup per repository however
  many releases use it. A failed lookup keeps the last result and shows
  "lookup failed" (neutral).
- **Ignore**: `updates.ignore` takes chart globs for things that are
  never upgraded on their own.

### Display

- Neutral by default: "update available" never changes a card's tone and
  never notifies (RFC 0019). Only reboot pending and version skew are
  amber.
- Infrastructure page: per node "N updates", "reboot pending".
- Workloads page: the cluster header with Kubernetes and distribution
  ("k0s v1.36.1+k0s.0, v1.36.2+k0s.0 available").
- GitOps page: chart version per HelmRelease or Application and the Flux
  version.
- An "Updates" page: everything in one table (kind, name, installed,
  newest patch, newest, where it runs), filterable by kind.
- **Roles**: nothing of this is shown to a role. Versions help an attacker
  more than a demo visitor.

### Configuration

| Key | Default | |
|---|---|---|
| `updates.proxmox` | `true` | package list and reboot pending |
| `updates.upstream.kubernetes` | `false` | newest Kubernetes patch (`dl.k8s.io`) |
| `updates.upstream.distro` | `false` | newest distribution release |
| `updates.upstream.flux` | `false` | newest Flux release |
| `updates.upstream.charts` | `false` | newest chart versions |
| `updates.github_token` | none | secret reference, raises the GitHub rate limit |
| `updates.registries[].host` | - | registry host, for example `ghcr.io` |
| `updates.registries[].username`, `password` | - | secret references |
| `updates.charts[].name`, `repo` | - | repository for a plain Helm release's chart |
| `updates.ignore` | `[]` | chart globs never looked up |
| `kubernetes.<name>.distro` | `"auto"` | `"none"`, a built-in name, or `{ github = "owner/repo" }` |
| `kubernetes.<name>.flux_namespace` | `"flux-system"` | where to read the Flux version |

### Logs and metrics

| | |
|---|---|
| `msg="query" source="updates-pve/<name>"` | as every query |
| `msg="upstream" kind repo result ms` | one per lookup, warn on failure |

## Changes during implementation

- Display: one Updates page and one Updates card instead of badges spread
  over the Infrastructure, Workloads and GitOps pages. Reboot pending and
  version skew make the Updates card amber; the Proxmox and Kubernetes
  cards keep their own rules.
- Chart versions are compared without build metadata (semver gives it no
  precedence): Flux reports OCI charts as `0.15.0+<digest>`.
- Argo CD names OCI Helm repositories without a scheme; they are read as
  `oci://`.
- k0s reports `v1.36.4+k0s` without the `.N` of its release tag, so only
  the Kubernetes part is compared for k0s.
- A chart from a GitRepository has no release to compare and shows its
  repository as unknown.
- The query is cached 5 minutes; upstream answers 12 hours, failures 30
  minutes. Upstream hosts go through the same HTTP client as integrations,
  so they show on the certificates page too.
- The YAML parser for `index.yaml` is `serde_yaml_ng`.
- `GET /nodes/{node}/apt/update` needs `Sys.Modify`, not `Sys.Audit` as
  assumed above. Homie keeps the read-only `PVEAuditor` role: without
  `Sys.Modify` the package count is unknown (with a note on the page) and
  reboot pending still comes from `apt/versions` (`Sys.Audit`). (v0.10.1)

## Alternatives

| Option | Why not |
|---|---|
| Run or bump updates from Homie | Changes the lab outside git and the host tooling (RFC 0001 non-goal) |
| Refresh the apt list from Homie | Needs `Sys.Modify` on the token for no real gain over the daily run |
| Marking security updates | `apt/update` has `Origin`, which does not reliably say "security" on Debian; a wrong "no security updates" is worse than none |
| Upstream lookups on by default | Homie makes no calls to third parties unless configured |
| Renovate as the source | Only what is in git; the runtime view is the point. Both together are fine |
| A client per registry or per project | The OCI distribution API and GitHub releases cover them all |
| Artifact Hub for charts | Needs the repository name too, and only knows published repositories |
| Container images in this RFC | The largest part (tag heuristics, digests, registry auth); RFC 0021, on the same clients |

## Testing and rollout

- **Tests:** apt list parsing, kernel comparison (running vs installed,
  `-pve` suffixes), distribution detection per row of the table and the
  override, k0s `+k0s.N` matching; version parsing and comparison (prefix,
  pre-releases, newest patch vs newest); OCI token challenge against a
  local test registry; Helm index entries; source resolution for Flux,
  Argo CD and mapped plain releases; ignore globs; cache and failure keep
  the last result; role redaction.
- **Rollout:** installed versions and Proxmox on by default, every
  upstream lookup off. No new Proxmox privilege. New Kubernetes read: `get`
  on the Flux namespace. Network policies need egress to `api.github.com`,
  `dl.k8s.io` and the chart registries in use when lookups are on. A YAML
  parser dependency for `index.yaml` (smallest maintained option chosen in
  the implementation PR).
- **Off switch:** `updates.proxmox = false`, every `updates.upstream.*`
  `false`.

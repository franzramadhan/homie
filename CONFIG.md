# Configuration

Every key Homie reads, with its default. Every section is optional: configure
only what your lab has. Ready-made files are in [`examples/`](examples/):
`local.toml` (laptop), `server-oidc.toml` (a server behind any OIDC
provider), `reference-lab.toml` (the author's lab) and `docker-compose.yaml`.
The Helm chart takes the same TOML in its `config` value.

Run `homie check` after every change: it validates the file and tests each
integration. Unknown keys are errors, so a typo never goes unnoticed.

## Where the config comes from

1. Built-in defaults.
2. One TOML file, the first found of: `--config <path>`, `$HOMIE_CONFIG`,
   `~/.config/homie/config.toml`, `/etc/homie/config.toml`. No file is fine:
   Homie then runs on defaults and environment variables.
3. Environment variables `HOMIE__<SECTION>__<KEY>`, which win over the file.
   Sections and names are separated by `__`:
   `HOMIE__PROXMOX__LAB__TOKEN=...` sets `token` in `[proxmox.lab]`,
   `HOMIE__REFRESH_SECONDS=30` sets `refresh_seconds`. Lists use `[a, b]`.

### Secrets

Every key marked *secret* below takes a plain string or a reference, read
when Homie starts:

```toml
[grafana.a]
url = "https://grafana.example.com"
token = "glsa_..."                                           # plain value
[grafana.b]
url = "https://grafana.example.com"
token = { env = "GRAFANA_TOKEN" }                            # environment variable
[grafana.c]
url = "https://grafana.example.com"
token = { file = "/run/secrets/grafana" }                    # file contents
[grafana.d]
url = "https://grafana.example.com"
token = { cmd = "security find-generic-password -s homie-grafana -w" }  # command output
```

Paths may start with `~/`.

### TLS

Sections that call an HTTP endpoint (`proxmox`, `grafana`, `openbao`,
`healthchecks`) share these keys:

| Key | Default | |
|---|---|---|
| `tls_verify` | `true` | `false` accepts any certificate; prefer `ca_file` |
| `ca_file` | none | extra PEM roots, for a private CA |
| `tls_server_name` | none | name checked against the certificate, when the URL uses an IP or another name (not in `proxmox`; set it per endpoint there) |

## Server

| Key | Default | |
|---|---|---|
| `listen` | `"127.0.0.1:7777"` | `ip:port` of the web UI |
| `public_url` | `""` | the URL browsers use. Needed for login (`https://` with `[auth]`) and checked as the origin of writes |
| `state_dir` | none | directory for the generated password, the cache snapshot, the action history and the GitHub watch list. Without it, nothing survives a restart |
| `refresh_seconds` | `15` | how often the UI refreshes, 5 or more |
| `live_updates` | `true` | push results to open pages over server-sent events |
| `allow_unauthenticated` | `false` | allow a non-loopback `listen` with no login. Anyone who reaches it uses your tokens |
| `log_format` | `"text"` | `"text"` or `"json"` (one object per line) |

```toml
listen = "0.0.0.0:8080"
public_url = "https://homie.example.com"
state_dir = "/var/lib/homie"
```

### `[quiet_hours]`

A daily window in which integrations marked `quiet = true` show as
"scheduled off" instead of down (a lab that is shut down at night).

| Key | Default | |
|---|---|---|
| `window` | required | `"HH:MM-HH:MM"`, may cross midnight |
| `timezone` | the host's | IANA name such as `"Europe/Berlin"` |

```toml
[quiet_hours]
window = "23:30-06:00"
timezone = "Asia/Jakarta"
```

## Login

Without `[auth]` or `[local_login]`, Homie only starts on a loopback address
(or with `allow_unauthenticated`).

### `[auth]`: OIDC

Any OIDC provider. Register a client with the redirect URI
`<public_url>/auth/callback`.

| Key | Default | |
|---|---|---|
| `issuer` | required | the provider's issuer URL |
| `client_id` | required | *secret* |
| `client_secret` | none | *secret*; omit for a public client |
| `scopes` | `["openid", "email", "profile", "offline_access"]` | `offline_access` lets a session outlive the ID token |
| `email_claim` | `"email"` | claim holding the user's email |
| `groups_claim` | none | claim holding the user's groups; needed for `allowed_groups` |
| `allowed_emails` | `[]` | who may log in |
| `allowed_groups` | `[]` | members of these groups may log in. Set this or `allowed_emails` |
| `roles` | `{}` | email to role name (see `[roles.*]`); users not listed have full access |

```toml
[auth]
issuer = "https://auth.example.com/application/o/homie/"
client_id = "homie"
client_secret = { env = "OIDC_CLIENT_SECRET" }
allowed_emails = ["me@example.com", "friend@example.com"]
roles = { "friend@example.com" = "viewer" }
```

### `[local_login]`: username and password

Works alone or next to `[auth]` (one login page). Ten wrong passwords
within 15 minutes lock the login for 5 minutes.

| Key | Default | |
|---|---|---|
| `username` | `"homie"` | |
| `password_hash` | none | *secret*; argon2 hash from `printf %s 'pw' \| homie hash-password`. Without it, a random password is generated at the first start, logged once and kept in `state_dir` (so `state_dir` is then required) |
| `role` | none | a role for this user, e.g. on a public demo instance |

```toml
[local_login]
username = "homie"
password_hash = { env = "HOMIE_PASSWORD_HASH" }
```

### `[roles.<name>]`: read-only views

A role sees some pages and never runs actions. Give it to users with
`auth.roles`, `local_login.role` or `demo.role`.

| Key | Default | |
|---|---|---|
| `pages` | required | any of `infra workloads gitops alerts logs security tailscale workflows services outage history map`. The Overview is always there, with the cards of these pages only |
| `namespaces` | `[]` | Kubernetes namespaces it sees, exact names. Required when `pages` has `workloads` or `gitops` and a cluster is configured |
| `proxmox_tags` | `[]` | only guests with one of these tags are shown |
| `kubernetes_service_account` | none | `"namespace/name"` of a read-only ServiceAccount Homie impersonates for this role's Kubernetes reads. Required with `workloads` or `gitops` |

```toml
[roles.viewer]
pages = ["infra", "workloads"]
namespaces = ["media", "home"]
proxmox_tags = ["shared"]
kubernetes_service_account = "homie/viewer"
```

### `[demo]`: "Try the demo"

A passwordless login that gives every visitor the same role. Needs
`[auth]` or `[local_login]`.

| Key | Default | |
|---|---|---|
| `role` | required | a `[roles.*]` name |
| `max_sessions` | `200` | demo sessions at once, 1 or more; the oldest is dropped |

```toml
[demo]
role = "viewer"
```

## Integrations

Each section is a map: `[proxmox.lab]`, `[proxmox.office]` and so on. The
name appears in the UI. Every integration takes `quiet = true` to follow
`[quiet_hours]` (default `false`).

### `[proxmox.<name>]`

| Key | Default | |
|---|---|---|
| `endpoints` | required | node URLs, tried in order. Each is a string or `{ url, tls_verify, ca_file, tls_server_name }` |
| `token` | required | *secret*; `user@realm!tokenid=<uuid>`. `PVEAuditor` is read-only; add `VM.PowerMgmt` for the power buttons |
| `tls_verify`, `ca_file` | `true`, none | defaults for every endpoint |
| `protected_tags` | `[]` | guests with one of these tags need their name typed before an action |
| `ui_url` | the first endpoint | where browsers open the Proxmox UI |
| `console` | `false` | embedded guest consoles; the token also needs `VM.Console` |
| `console_max_minutes` | `60` | a console closes after this long |
| `quiet` | `false` | |

```toml
[proxmox.lab]
endpoints = [
  "https://pve1.example.com:8006",
  { url = "https://192.168.1.11:8006", tls_server_name = "pve2.example.com" },
]
token = { env = "PVE_TOKEN" }
ca_file = "/etc/homie/pve-root-ca.pem"
protected_tags = ["homie-protected"]
```

### `[kubernetes.<name>]`

| Key | Default | |
|---|---|---|
| `auth` | `"in_cluster"` in a pod, else `"kubeconfig"` | or `"forward_token"`: act as the logged-in user with their OIDC token (needs `[auth]` and an apiserver that trusts the issuer) |
| `kubeconfig` | `$KUBECONFIG` or `~/.kube/config` | |
| `context` | the current one | |
| `headlamp_url` | none | Headlamp base URL; fills the unset `links` |
| `headlamp_cluster` | `"main"` | `{cluster}` in links |
| `links` | none | deep-link templates `node`, `pod`, `workload`, `flux` with `{cluster}`, `{namespace}`, `{name}`, `{kind}`, `{plural}` and, for Flux, `{group}` |
| `argocd_url` | none | Argo CD base URL, for links to Applications |
| `cnpg_max_backup_age_hours` | `26` | a CloudNativePG backup older than this is stale |
| `cert_manager` | `false` | "renewal failing" from cert-manager Certificates |
| `distro` | `"auto"` | the distribution for the Updates page: `"auto"`, `"none"`, `"k0s"`, `"k3s"`, `"rke2"`, `"talos"` or `{ github = "owner/repo" }` |
| `flux_namespace` | `"flux-system"` | where the Flux version is read |
| `quiet` | `false` | |

`[kubernetes.<name>.metrics]` adds sparklines and the disk-full forecast,
through a `[grafana.*]` Prometheus-compatible datasource:

| Key | Default | |
|---|---|---|
| `grafana`, `datasource` | required | a `[grafana.*]` name and a datasource uid |
| `cpu`, `memory` | required | PromQL, one series per node |
| `node_label` | `"node"` | the label with the node name |
| `volume_used`, `volume_capacity` | none | PromQL, one series per volume, in bytes; set both or neither |
| `volume_label`, `namespace_label` | `"persistentvolumeclaim"`, `"namespace"` | labels naming each volume |

`[kubernetes.<name>.certificates]` finds TLS hostnames in the cluster and
probes them for expiry:

| Key | Default | |
|---|---|---|
| `discover` | `false` | |
| `sources` | `["ingress", "gateway", "annotations"]` | |
| `annotations` | `["external-dns.alpha.kubernetes.io/hostname"]` | keys read by the `annotations` source |
| `discover_address` | none | `host:port` to connect to instead of resolving each name (the ingress Service) |
| `ca_file` | none | extra roots for these hostnames |

```toml
[kubernetes.home]
auth = "kubeconfig"
context = "home"
headlamp_url = "https://headlamp.example.com"
cert_manager = true

[kubernetes.home.metrics]
grafana = "lab"
datasource = "prometheus"
cpu = 'sum by (node) (rate(node_cpu_seconds_total{mode!="idle"}[5m]))'
memory = 'sum by (node) (node_memory_MemTotal_bytes - node_memory_MemAvailable_bytes)'
volume_used = 'sum by (namespace, persistentvolumeclaim) (kubelet_volume_stats_used_bytes)'
volume_capacity = 'sum by (namespace, persistentvolumeclaim) (kubelet_volume_stats_capacity_bytes)'

[kubernetes.home.certificates]
discover = true
discover_address = "ingress-nginx-controller.ingress-nginx.svc:443"
```

Kubelet reports no volume stats for `hostPath` volumes (the default of the
local-path provisioner). Forecast the node filesystems instead, e.g.
`node_filesystem_size_bytes` with `volume_label = "mountpoint"` and
`namespace_label = "instance"`.

### `[grafana.<name>]`

Alerts, and the datasources other sections query through its proxy.

| Key | Default | |
|---|---|---|
| `url` | required | |
| `token` | required | *secret*; a service account token, role Viewer |
| `ui_url` | `url` | where browsers open Grafana when `url` is internal |
| `ignore_alerts` | `["Watchdog"]` | alert names never listed |
| TLS keys, `quiet` | | |

```toml
[grafana.lab]
url = "http://grafana.monitoring.svc"
ui_url = "https://grafana.example.com"
token = { env = "GRAFANA_TOKEN" }
```

### `[openbao.<name>]` or `[vault.<name>]`

The unauthenticated health of OpenBao or HashiCorp Vault (sealed, standby).

| Key | Default | |
|---|---|---|
| `url` | required | |
| TLS keys, `quiet` | | |

```toml
[vault.main]
url = "https://vault.example.com:8200"
```

### `[healthchecks]`

| Key | Default | |
|---|---|---|
| `url` | `"https://healthchecks.io"` | or a self-hosted instance |
| `api_key` | required | *secret*; a read-only project key |
| TLS keys, `quiet` | | |

### `[tailscale]`

| Key | Default | |
|---|---|---|
| `tailnet` | `"-"` | the token's own tailnet |
| `api_key` | none | *secret* |
| `oauth_client_id`, `oauth_client_secret` | none | *secret*; an OAuth client with `devices:core:read`. Set these or `api_key` |
| `hosts` | `["*"]` | device name globs to show |
| `quiet_hosts` | `[]` | devices that follow `[quiet_hours]` |

```toml
[tailscale]
oauth_client_id = { env = "TS_CLIENT_ID" }
oauth_client_secret = { env = "TS_CLIENT_SECRET" }
```

### `[github]`: workflow runs

| Key | Default | |
|---|---|---|
| `app_id`, `private_key` | none | a GitHub App (`private_key` *secret*, PEM). Set these or `token` (not needed with `store = "session"`) |
| `token` | none | *secret*; a fine-grained token instead of the App |
| `client_id`, `client_secret` | none | *secret*; the App's user authorization, for the repository picker and `store = "session"` |
| `install_url` | none | the App's install page, offered when a signed-in user sees no repositories |
| `api_url` | `"https://api.github.com"` | GitHub Enterprise: `https://<host>/api/v3` |
| `store` | `"auto"` | where picked watches are saved: `"auto"` (a ConfigMap in a cluster, else a file in `state_dir`, else off), `"configmap"`, `"file"`, `"off"`, or `"session"` (each visitor picks their own with their GitHub login; takes no `[[github.watch]]`) |
| `quiet` | `false` | |

`[[github.watch]]` lists workflows to show: `repo = "owner/name"`,
`workflow = "ci.yml"` (a file name or numeric id).

```toml
[github]
app_id = 123456
private_key = { file = "/run/secrets/github-app.pem" }

[[github.watch]]
repo = "me/infrastructure"
workflow = "terraform-apply.yml"
```

## `[security]`: presets

| Section | Keys | Reads |
|---|---|---|
| `[security.kyverno]` | `kubernetes` (a `[kubernetes.*]` name) | PolicyReports |
| `[security.tetragon]` | `grafana`, `datasource`, `filter` (default `"k8s.container.name:export-stdout"`) | Tetragon events in VictoriaLogs |
| `[security.wazuh]` | `grafana`, `datasource`, `filter` (default `"k8s.namespace.name:wazuh k8s.container.name:alerts"`), `min_level` (default `7`) | Wazuh alerts in VictoriaLogs |
| `[security.debsecan]` | `grafana`, `datasource` | `debsecan_vulnerabilities` metrics |

```toml
[security.kyverno]
kubernetes = "home"

[security.wazuh]
grafana = "lab"
datasource = "vlogs"
```

## `[logs]`: log search and tail

| Key | Default | |
|---|---|---|
| `max_rows` | `1000` | rows per search |
| `tail_seconds` | `5` | live tail poll interval, 2 to 60 |
| `tail_max_minutes` | `30` | a tail stops after this long; `0` turns tail off |

`[[logs.datasources]]` are the only datasources that can be searched:

| Key | Default | |
|---|---|---|
| `label` | required | shown in the UI, unique |
| `grafana`, `datasource` | required | a `[grafana.*]` name and a datasource uid |
| `type` | required | `"victorialogs"` or `"loki"` |
| `pod_query` | none | the query for one pod, with `{namespace}` and `{pod}`; adds a Logs link to pods |
| `node_query` | none | the query for one node, with `{node}` |

```toml
[[logs.datasources]]
label = "cluster"
grafana = "lab"
datasource = "loki"
type = "loki"
pod_query = '{namespace="{namespace}", pod="{pod}"}'
```

## `[certificates]`: expiry

Integration endpoints are recorded on every connection; listed and
discovered hosts are probed on a schedule.

| Key | Default | |
|---|---|---|
| `warn_days` | `14` | amber for leaf and intermediate certificates |
| `root_warn_days` | `90` | amber for roots from a `ca_file` |
| `interval_hours` | `6` | probe interval, 1 or more |
| `endpoints` | `true` | record the integrations' own endpoints |

`[[certificates.targets]]`: `host` (a name, no scheme or port), `port`
(default `443`), `address` (connect here instead of resolving `host`),
`ca_file`.

```toml
[[certificates.targets]]
host = "nas.example.com"
port = 5001
```

## `[notify]`: messages on status changes

A message when an Overview card changes status and holds it.

| Key | Default | |
|---|---|---|
| `targets` | required | at least one `[[notify.targets]]` |
| `interval_seconds` | `60` | evaluation interval, 10 or more |
| `hold_seconds` | `180` | how long a new status must hold |
| `min_tone` | `"bad"` | `"warn"` or `"bad"` |
| `min_tone_per_card` | `{ certificates = "warn", cnpg = "warn" }` | per card id |
| `recovery` | `true` | also send when a card recovers |
| `cards` | all but `alerts` | card ids: `pve k8s flux argocd alerts security tailscale openbao cnpg github healthchecks updates certificates` |
| `redact` | `false` | send the card and its status only, no names |
| `kubernetes_service_account` | none | `"namespace/name"` impersonated for `forward_token` clusters, which have no user session here |

`[[notify.targets]]`:

| Key | Default | |
|---|---|---|
| `type` | required | `"ntfy"`, `"slack"` or `"webhook"` |
| `url` | required | *secret* |
| `token` | none | *secret*; a bearer token (ntfy access token) |
| `headers` | `{}` | *secret* values |
| `ca_file` | none | |

`homie notify-test` sends a test message to every target.

```toml
[notify]
min_tone = "warn"

[[notify.targets]]
type = "ntfy"
url = "https://ntfy.sh/my-lab-topic"
```

## `[updates]`: what is out of date

| Key | Default | |
|---|---|---|
| `proxmox` | `true` | package updates and pending reboots per Proxmox node |
| `github_token` | the `[github]` token or App | *secret*; raises GitHub's limit of 60 anonymous requests an hour |
| `ignore` | `[]` | chart and image globs never looked up |

`[updates.upstream]` turns on lookups at third parties:

| Key | Default | |
|---|---|---|
| `kubernetes` | `false` | the latest Kubernetes release |
| `distro` | `false` | the distribution's latest release |
| `flux` | `false` | the latest Flux release |
| `charts` | `false` | Helm chart repositories |
| `images` | `false` | container registries |
| `notes` | `true` | release notes and security fixes of the updates found (on GitHub) |

Lists:

- `[[updates.registries]]`: `host`, `username`, `password` (*secret*), for
  private registries.
- `[[updates.charts]]`: `name`, `repo` (`oci://`, `https://` or `http://`),
  for charts installed with plain `helm install`.
- `[[updates.sources]]`: `name` (a chart or image glob), `github`
  (`owner/repo`), for projects whose metadata names no source.

```toml
[updates.upstream]
kubernetes = true
flux = true
charts = true
images = true

[[updates.sources]]
name = "pihole"
github = "pi-hole/docker-pi-hole"
```

## `[forecast]`: disk full

A straight line over a week of usage, for Proxmox storages and the volumes
of `[kubernetes.*.metrics]`.

| Key | Default | |
|---|---|---|
| `enabled` | `true` | |
| `warn_days` | `14` | amber when full within this many days |
| `bad_days` | `3` | red when full within this many days |

## `[history]`: who did what

Logins, logouts and actions with their result, on the History page.

| Key | Default | |
|---|---|---|
| `enabled` | `true` | `false`: no page, nothing kept |
| `size` | `1000` | entries kept, 1 or more. With `state_dir`, they survive restarts in `history.jsonl` |

## `[map]`: the lab as a village

| Key | Default | |
|---|---|---|
| `enabled` | `true` | `false`: no Map page and no nav entry |

## `[metrics]`: Prometheus endpoint

`GET /metrics` on a listener of its own. Alert rules are in
[`examples/metrics-alerts.yaml`](examples/metrics-alerts.yaml).

| Key | Default | |
|---|---|---|
| `listen` | required | `ip:port` |
| `token` | none | *secret*; scrapes then need `Authorization: Bearer <token>` |
| `cards` | `true` | evaluate the cards' status at scrape time |
| `kubernetes_service_account` | none | `"namespace/name"` impersonated for `forward_token` clusters |

```toml
[metrics]
listen = "0.0.0.0:9477"
token = { env = "HOMIE_METRICS_TOKEN" }
```

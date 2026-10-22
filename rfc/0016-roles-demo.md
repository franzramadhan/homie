# 0016: Roles and a public demo

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #41 | v0.6.0 |

> A role limits what a user sees: which pages, which namespaces, which
> guests, and no actions. The first use is a public read-only demo of the
> reference lab.

## Problem

Every Homie user sees and can do everything. The author wants to link a demo
from this repository: visitors should see the Infrastructure, Workloads and
GitOps pages of chosen workloads and guests, read-only, and nothing about
the rest of the lab (system namespaces, secrets stores, alerts, logs,
security findings, the tailnet).

## Proposal

### Roles

- `[roles.<name>]` in the config. A user without a role keeps full access, as
  today.
- A role lists:
  - `pages`: allowed pages (`overview`, `infra`, `workloads`, `gitops`, ...).
    The Overview shows only the cards of allowed pages.
  - `actions = false`: every `/api/act/*`, the GitHub picker and its save
    route answer 403.
  - `namespaces`: an explicit allow-list, exact names only. No globs and no
    "all": a new namespace is never shown until someone adds it.
  - `proxmox_tags`: guests are shown only with one of these tags. Nodes show
    name, status and usage; no addresses.
- Enforcement is on the server. Forbidden queries and routes answer 403;
  `/api/meta` lists only allowed pages, so the UI hides the rest. Results are
  filtered per role when served (pods, workloads, events, Flux objects,
  guests, sparklines), after the shared cache. The live stream filters the
  same way.

### Kubernetes access for a role

- A role reads Kubernetes through a ServiceAccount, never through a user's
  token: Homie impersonates `system:serviceaccount:<ns>:<role sa>`
  (`[roles.demo] kubernetes_service_account = "homie-demo"`). Homie's own
  ServiceAccount needs `impersonate` on that one ServiceAccount name; no
  long-lived token exists.
- Kubernetes enforces the namespaces too: `homie-demo` gets a RoleBinding
  (view) in each allowed namespace only, and Homie lists namespaced objects
  per allowed namespace instead of cluster-wide. Nodes need a small
  ClusterRole (`get`, `list` nodes).
- Flux objects live in `flux-system`; an object is shown when the namespace
  it deploys into (`spec.targetNamespace`, a HelmRelease's
  `spec.targetNamespace` or `spec.storageNamespace`, else its own namespace)
  is allowed. This needs read access to the Flux kinds in `flux-system`, so
  for Flux the filter in Homie is the only boundary.
- Links to other UIs (Headlamp, Grafana, Proxmox) are removed for a role:
  they would leak names of things outside the allow-list if followed, and the
  demo visitor cannot log in there anyway.

### Demo login

- `[[local_login.users]]` replaces the single local user (the old keys keep
  working as one user without a role), each with an optional `role`.
- `[demo] role = "demo"` adds a **Try the demo** button to the login page: no
  password, a session with that role. Demo sessions are capped
  (`max_sessions`, default 200, oldest dropped) and last at most 1 h.
- Demo sessions get no live-update watches: their streams follow the shared
  cache on the refresh tick only, so visitors never add apiserver watches.

### Public exposure

- Rate limits belong in front of Homie: a Cloudflare WAF rate limiting rule
  on the demo hostname (the free plan includes one rule), for example
  60 requests per 10 s per IP on `/api/*`. Homie adds no rate limiter.
- Outside the lab's hours the tunnel is down and Cloudflare shows its error
  page; a static "the lab sleeps 23:30 to 06:00 WIB" page is a Cloudflare
  custom error page, not Homie's job.

### Configuration

| Key | Default | |
|---|---|---|
| `[roles.<name>] pages, namespaces, proxmox_tags, kubernetes_service_account` | none | a role; users without one have full access |
| `[auth] roles = { "<email>" = "<role>" }` | none | roles for OIDC users |
| `[demo] role, max_sessions` | off | the passwordless demo button |

### Logs and metrics

| | |
|---|---|
| `msg="login" method="demo"` | a demo session started |
| `msg="denied" role=... route=...` | a role hit a forbidden route |

## Alternatives

| Option | Why not |
|---|---|
| Demo user through OIDC with namespaced RBAC | Visitors would need accounts; later if someone needs it |
| Deny-list of namespaces | A new namespace would appear in the demo without anyone deciding |
| Filtering only in Homie | Kubernetes RBAC per namespace keeps a Homie filter bug from leaking workloads |
| A separate demo instance | Twice the deploy; the role is useful on its own |

## Testing and rollout

- **Tests:** role parsing; 403 on forbidden queries, routes and actions;
  per-namespace filtering of pods, workloads, events and Flux objects;
  Proxmox tag filter; demo session cap; impersonation in kind with a
  namespaced RoleBinding (a pod outside the allow-list never appears).
- **Rollout:** v0.6.0; no role configured = today's behavior.
- **Off switch:** remove `[demo]` (no demo button) or the role.

## Implementation notes

Deviations found while building (v0.6.0):

- Roles never run actions: there is no `actions` key, every route other than
  `GET /api/meta`, `GET /api/q/*`, `GET /api/stream`, `POST /auth/logout`
  (and `POST /api/logs` with the `logs` page) answers 403 for a role, and a
  session whose role left the config is refused, never given full access.
- Pages: `infra`, `workloads`, `gitops`, `alerts`, `logs`, `security`,
  `tailscale`, `workflows`, `services` (OpenBao, CNPG), `outage`
  (healthchecks). Pages other than the first three are not filtered further.
- Helm releases are hidden for roles: they are read from Secrets.
- Flux: a Kustomization is shown when every namespaced object in its
  `status.inventory` is in an allowed namespace; a HelmRelease by
  `spec.targetNamespace`, else its own namespace; sources are hidden.
  Argo CD is hidden for roles: listing Applications needs extra RBAC for the
  role's ServiceAccount, and the apiserver answers 403 (not "not installed")
  without it.
- Proxmox: storage and failed tasks are empty and power is off for a role.
- `[[local_login.users]]` is not in v0.6.0: the demo button covers the demo,
  and OIDC users get roles through `[auth] roles`. `[local_login] role`
  gives the single local user a role, so a public demo instance has no
  full-access login at all.
- For a public demo, run a separate instance (same image) with only the
  demo's integrations and a read-only Proxmox token: the role is the boundary
  inside one instance, the separate pod limits what a compromise reaches.
- The Helm chart and `deploy/kubernetes` do not create the role's
  ServiceAccount yet; the reference lab's manifests show the RBAC.

Tested in kind: the demo session sees only its namespace's workloads and is
refused alerts, Helm, actions, logs and the GitHub picker (each logged as
`msg="denied"`); with a namespace in the role but no RoleBinding for the
ServiceAccount, the apiserver refuses the list and Homie returns an error,
not data.

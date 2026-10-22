# 0014: GitHub Actions workflow monitoring

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #24 | - |

> Show the runs of chosen GitHub Actions workflows (for example Terraform
> plan and apply) next to the rest of the lab. One shared watch list in the
> config, read with a GitHub App installation token (or a fine-grained
> token); a GitHub sign-in only helps pick repositories and workflows.

## Problem

Infrastructure changes in many labs run as GitHub Actions workflows
(Terraform plan on a pull request, apply on merge). Whether the last apply
succeeded, or one is stuck waiting for a runner, is only visible on GitHub.
A failed apply is exactly the kind of lab state Homie should show.

## Proposal

### One shared watch list

The watched workflows are part of the lab's state, not of the viewer, so
the list is the same for everyone, like the Proxmox or Grafana sections:

```toml
[[github.watch]]
repo = "owner/infrastructure"
workflow = "terraform-plan.yml"     # file name or numeric id

[[github.watch]]
repo = "owner/infrastructure"
workflow = "terraform-apply.yml"
```

The list lives in the config (GitOps-managed in a cluster, stateless).

### Reading runs: no user sign-in

| Credential | Access | Use |
|---|---|---|
| **GitHub App installation token (recommended)** | The app has only `Actions: read` and `Metadata: read`, installed on the watched repositories. Homie signs a JWT with the app's private key and exchanges it for 1 h installation tokens. | Cluster and laptop; no personal token |
| Fine-grained personal access token | `Actions: read`, `Metadata: read`, chosen repositories | Simplest single-user setup |
| OAuth App | Not supported: private repositories need the `repo` scope, which is full write access | - |

Reads work without anyone signed in, so the overview shows workflow health
from a laptop even when the lab's SSO is down.

### Picking workflows: a sign-in helper

- With `client_id` and `client_secret` (the same GitHub App's user
  authorization), a signed-in Homie user can "Connect GitHub" on the
  Workflows page (OAuth web flow with state; the user token stays in the
  server-side session, never in the browser, and is dropped at logout).
- Homie lists the repositories the user can reach through the app
  installation and, for a picked repository, its workflows.
- Choosing workflows produces the exact `[[github.watch]]` lines to copy
  into the config. Homie does not write the config itself.

### What is shown

- Per watched workflow, the last 10 runs: status (queued, in_progress,
  completed), conclusion (success, failure, cancelled, skipped), event,
  branch, actor, started, duration, the commit id (short, linked to the
  commit) and a link to the run.
- No Terraform plan summary: the link to the run is enough.
- Overview card "Workflows": red when the latest completed run of any
  watched workflow failed; amber when a run has been queued for more than
  10 minutes (no runner, as when an in-cluster runner is down overnight).
- Polling every 60 s per watched workflow, cached and shared. Rate limit
  (403 or 429 with `x-ratelimit-remaining: 0`) keeps the last data, marked
  stale.

### Configuration

| Key | Default | |
|---|---|---|
| `github.app_id`, `github.private_key` (secret reference) | - | GitHub App: installation tokens for reading |
| `github.token` | - | fine-grained PAT instead of the app |
| `github.client_id`, `github.client_secret` | - | optional: the sign-in helper for picking |
| `github.watch` | none | `{repo, workflow}` entries |
| `github.api_url` | `https://api.github.com` | GitHub Enterprise Server |
| `github.quiet` | `false` | quiet hours apply |

## Alternatives

| Option | Why not |
|---|---|
| Per-user watch lists | Health would depend on the viewer, nothing shows before a sign-in, and the lists need storage the stateless cluster instance does not have |
| OAuth App | `repo` scope for private repositories is full write access |
| Webhooks (`workflow_run`) | Needs Homie reachable from GitHub; Homie is tailnet-only in the reference lab |
| Terraform plan summary from job logs | Not needed: the run link and commit id are enough |

## Testing and rollout

- **Tests:** app JWT signing and installation token caching; run mapping
  from recorded API responses; queued-too-long and failed-last-run rules;
  rate-limit handling; picker output is valid TOML.
- **E2E:** GitHub App installed on the infra repository; watch
  `terraform-plan` and `terraform-apply`; the latest runs appear with the
  right state, commit and links.
- **Rollout:** optional section; nothing changes without `[github]`.

## Changes during implementation

- The app finds its installation per repository owner
  (`/users/{owner}/installation`, then `/orgs/{owner}/installation`) and
  caches the installation token for 55 minutes.
- Each watched workflow fails on its own (its error shows on its card) and
  does not hide the others; the query only fails when all of them do.
- `homie check` reads every watched workflow.
- The picker routes are `/github/login`, `/github/callback`,
  `/api/github/repos` and `/api/github/workflows`, behind the Homie session;
  GitHub user tokens are kept per Homie user in memory.
- Verified with a fine-grained token against the reference lab's
  `plan.yaml` and `apply.yaml`; the app path is verified once the app exists.

## Decisions

1. Shared list in the config, not per user.
2. No plan summary; link to the run and the commit id.

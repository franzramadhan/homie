# 0014: GitHub Actions workflow monitoring

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | - | - |

> Show the runs of chosen GitHub Actions workflows (for example Terraform
> plan and apply) next to the rest of the lab. The user signs in to GitHub
> through Homie, picks repositories they can read and the workflows to
> watch, and Homie shows each workflow's latest runs and their state.

## Problem

Infrastructure changes in many labs run as GitHub Actions workflows
(Terraform plan on a pull request, apply on merge). Whether the last apply
succeeded, or one is stuck waiting for a runner, is only visible on GitHub.
A failed apply is exactly the kind of lab state Homie should show.

## Proposal

### Authentication: a GitHub App with user authorization

| Option | Access | Fit |
|---|---|---|
| Fine-grained personal access token | One token in the config, scoped to chosen repositories, `Actions: read` + `Metadata: read` | Simplest, but one static identity for everyone, and the token's owner chooses the repositories, not the user |
| OAuth App | Classic scopes only: private repositories need `repo`, which is full read and write | Too broad for a read-only view |
| **GitHub App, user authorization (recommended)** | Permissions fixed by the app: `Actions: read`, `Metadata: read`. A user token can never do more than the app allows and the user can access. Tokens last 8 h and refresh. | Matches "the user signs in and picks repositories" with least privilege |

- The operator registers a GitHub App (in their account or organization)
  with only `Actions: read` and `Metadata: read`, user authorization on,
  callback `<public_url>/github/callback`, and installs it on the
  repositories Homie may see.
- `[github] client_id`, `client_secret` (secret reference), optional
  `allowed_orgs`. The fine-grained token is also accepted
  (`[github] token`) for laptop and single-user setups, without the sign-in.
- "Connect GitHub" on the new GitOps tab runs the GitHub OAuth web flow
  (with PKCE and state). The user token and its refresh token stay in the
  server-side session, like OIDC tokens; they never reach the browser.

### Choosing what to watch

- After connecting, Homie lists the repositories the user can access
  through the app installation (`GET /user/installations`, then
  `/user/installations/{id}/repositories`) and, for a picked repository,
  its workflows (`GET /repos/{owner}/{repo}/actions/workflows`).
- The choice is saved as watch entries `{owner, repo, workflow_id, name}`:
  - **server profile:** in `state_dir/github-watch.json` per user (by
    login), so it survives restarts when `state_dir` is set, and in the
    session otherwise;
  - **config:** `[[github.watch]]` entries are fixed for everyone (for
    example the reference lab's `terraform-plan.yml` and
    `terraform-apply.yml`) and need no sign-in when a token is set.

### What is shown

- Per watched workflow: the last 10 runs
  (`GET /repos/{owner}/{repo}/actions/workflows/{id}/runs?per_page=10`):
  status (queued, in_progress, completed), conclusion (success, failure,
  cancelled, skipped), event, branch, actor, commit title, started, duration,
  link to the run.
- Overview card "Workflows": failing when the latest completed run of any
  watched workflow failed, amber while one is queued for more than 10
  minutes (a missing runner, as when the lab's in-cluster runner is down).
- Optional later: the Terraform plan summary ("Plan: 0 to add, 3 to change")
  from the run's job log. Not in this RFC.
- Polling every 60 s per watched workflow, cached and shared; GitHub's
  limit for a user token is 5000 requests per hour.

### Configuration

| Key | Default | |
|---|---|---|
| `github.client_id`, `github.client_secret` | - | GitHub App user authorization |
| `github.token` | - | fine-grained PAT instead (no sign-in) |
| `github.allowed_orgs` | all | only list repositories of these owners |
| `github.watch` | none | fixed `{owner, repo, workflow}` entries |
| `github.api_url` | `https://api.github.com` | GitHub Enterprise Server |

## Alternatives

| Option | Why not |
|---|---|
| OAuth App | `repo` scope for private repositories is full write access |
| Webhooks (`workflow_run`) | Needs Homie reachable from GitHub; Homie is tailnet-only in the reference lab. Polling is enough for a dashboard. |
| Reading Terraform state | Different per backend; the workflow run is the shared signal |

## Testing and rollout

- **Tests:** OAuth state and PKCE; token refresh; watch-list persistence;
  run mapping from recorded API responses; rate-limit handling (403 with
  `x-ratelimit-remaining: 0` shows stale data, not an error loop).
- **E2E:** a GitHub App installed on the infra repository; watch
  `terraform-plan` and `terraform-apply`; a plan run on a test PR shows up.
- **Rollout:** optional section; nothing changes without `[github]`.

## Open questions

1. Per-user watch lists, or one shared list the admin edits in the UI?
2. Show the Terraform plan summary in a first version?

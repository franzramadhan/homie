# 0017: Personal GitHub watches per session

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-23 | #59 | v0.7.0 |

> A visitor of a public demo connects their own GitHub account and watches
> their own workflow runs. The picks live only in their session and are read
> with their own token, so no visitor sees another's repositories.

## Problem

The Workflows page (RFC 0014) reads runs with the app's installation token
for one shared watch list, stored in the config or a ConfigMap (RFC 0015).
On a public demo that is wrong twice: every visitor would share one list,
and one visitor's picks would show runs of their repositories to everyone.
Roles (RFC 0016) therefore refuse all GitHub routes, and the demo has no
Workflows page.

The demo should show what Homie does with CI: a visitor signs in with
GitHub, picks workflows from their own repositories and sees their runs.

## Proposal

- A new watch store, `[github] store = "session"`:
  - Watches live in the Homie session only (memory). They end with the
    session (logout, or the demo's one hour), are never written anywhere and
    are never shared. The page starts empty.
  - Runs are read with that session's GitHub **user** token, not the app's
    installation token. GitHub then limits each visitor to repositories they
    can access and have installed the app on: isolation is GitHub's, not
    only Homie's bookkeeping.
  - Config watches (`[[github.watch]]`) are not allowed with this store
    (they would need the shared installation token).
- GitHub user tokens are kept **per session id**, not per user name. Today
  they are keyed by the user's email, and every demo session has the same
  user name, `demo`.
- Roles: a role with the `workflows` page may use the GitHub sign-in, the
  picker and its save route **only** when the store is `session`, because
  there they only ever touch the visitor's own data. With any other store
  they stay 403, as in RFC 0016. The run list is filtered like other role
  data (links kept: they point at the visitor's own repositories on
  github.com).
- The browser remembers the last picks in `localStorage` (repository and
  workflow names only, never tokens). After a new sign-in it offers "re-add
  your last N workflows"; the server validates each pick against the
  visitor's token like any other pick.
- Only a GitHub App works for this: its user tokens are limited to the
  app's permissions (Actions: read) and to repositories where it is
  installed. An OAuth App would need the `repo` scope, read and write on all
  of a visitor's private repositories.
- In session mode the app needs no private key: Homie never reads as the
  app. It needs the client id and secret, the callback URL, and "Request
  user authorization (OAuth) during installation", so a visitor installs and
  signs in in one step. The page explains that trying it means installing
  the app on a repository.

### Configuration

| Key | Default | |
|---|---|---|
| `[github] store` | `"auto"` | new value `"session"`: per-session watches read with the visitor's token |
| `[github] app_id`, `private_key` | | not needed with `store = "session"` |
| `[github] client_id`, `client_secret` | | required with `store = "session"` |

### Logs and metrics

| | |
|---|---|
| `msg="action" action="github.watch" store="session"` | a session changed its picks; repository names are not logged in session mode |

## Alternatives

| Option | Why not |
|---|---|
| Watches only in the browser (`localStorage`), sent with each query | The server still needs the visitor's token, so they sign in anyway; the list becomes untrusted input on every request. Kept as a convenience only |
| Visitors paste a fine-grained token | Asking the public for tokens |
| Public repositories without auth | 60 requests per hour per IP, shared by all visitors |
| OAuth App instead of a GitHub App | `repo` scope is read and write on all private repositories |
| Shared list with per-user filtering | Still mixes visitors' data in one place |

## Testing and rollout

- **Tests:** session watches are isolated between two sessions with the same
  user name; runs are fetched with the session's user token (fake GitHub
  checks the bearer); a role gets the picker routes only with `store =
  "session"`; config watches are rejected with that store; picks end with
  the session; no repository names in the action log.
- **Rollout:** v0.7.0. The reference demo enables the Workflows page with a
  public GitHub App `homie`; the main instance keeps its config and
  ConfigMap watches.
- **Off switch:** any other `store` value, or no `workflows` page in the role.

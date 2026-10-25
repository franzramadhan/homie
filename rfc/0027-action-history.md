# 0027: Action history

| Status | Date | PR | Release |
|---|---|---|---|
| implemented | 2026-10-25 | #114 | v0.13.0 |

> One page listing who did what and when: power actions, Flux and Argo
> CD reconciles, consoles, logins and their failures. Read from what
> Homie already logs, kept in the state directory, without Grafana.

## Problem

Every action is logged (`msg="action"`, `msg="console"`, `msg="login"`)
and counted (RFC 0024), but reading it back needs log access and a
query. "Who restarted that VM last night" is a question for the page
that did it. Proxmox's own task log names the shared Homie token, not
the person.

## Proposal

- **Recorded.** Every event already logged at the end of an action:
  power actions, reconciles, console opened and closed, login (ok,
  failed, locked), and logout (new, also logged as `msg="logout"`). Each entry: time, user (email or `local`),
  action, target (`pve/<node>/<vmid>`, `flux/<cluster>/<ns>/<name>`),
  result (`ok`, `error`, `denied`), and the error on failure. No request
  bodies, no tokens.
- **Kept.** The last 1000 entries in memory and in
  `state_dir/history.jsonl` (append, trimmed to the cap at start), so a
  restart keeps them. Without `state_dir`: memory only, the page says so.
- **Page.** "History" in the sidebar: a table (time, user, action,
  target, result) with the usual filter, newest first, live through the
  existing SSE stream (RFC 0009). Failed entries in the bad tone.
- **Who sees it.** Full users see everything. Roles see entries whose
  target is visible to them (their namespaces and tagged guests) and,
  for logins, only their own. Other users' emails are never shown to a
  role: they read as "another user".
- **Roles and the demo.** `history` is a page like the others (RFC 0016):
  a role sees it when its `pages` list it. The demo instance lists it, so
  visitors see the demo's reconciles, consoles and refused power actions,
  every other visitor as "another user". Refused actions (403 for a role)
  are recorded too, with result `denied`; the 1000-entry cap bounds what
  a busy demo can write.
- **API.** `GET /api/history` (same filtering), for scripts.

### Configuration

| Key | Default | |
|---|---|---|
| `history.enabled` | `true` | `false`: no page, nothing kept |
| `history.size` | `1000` | entries kept |

### Logs and metrics

| | |
|---|---|
| `msg="history" result=error` | `history.jsonl` could not be written (once, then every hour) |

No new metric: the counters of RFC 0024 already count actions.

## Changes during implementation

- Targets name the integration too: `pve/<proxmox>/<node>/<vmid>`,
  `flux/<cluster>/<kind>/<ns>/<name>`, `argocd/<cluster>/<ns>/<name>`;
  a login's target is its method (`local`, `oidc`, `demo`), a logout has
  none. GitHub watch
  saves (RFC 0015, RFC 0017) go through the same path and are recorded as
  `github.watch`.
- Results: a failed or locked password login and an OIDC refusal are
  `denied`, with the reason in `error` ("too many failed logins" for the
  lockout). Consoles are `console.open` and `console.close`.
- "Their own" for a role means entries of the same session: each entry
  keeps a short digest of the session id (never the id), since every demo
  visitor is the user `demo`. A role's entries from an earlier session read
  "another user" too.
- "Visible" means the target is in the role's own view right now: a guest
  in its filtered Proxmox overview (node and vmid), a Flux object in its
  filtered Flux list. Argo CD entries never are, as Argo CD is never shown
  to a role. Errors are shown to a role with addresses redacted.
- A refused action's target comes from its request body, read in the
  guard only to name the target (at most 4 KiB, never kept); a body that
  does not parse is recorded with the Proxmox or cluster alone
  (`pve.power`, `pve/<proxmox>`), which no other role sees.
- The page is also the query `history/_` (`/api/q/history/_`), so the live
  stream carries it like any other; `GET /api/history` returns the rows
  alone. Both answer 404 when the history is off or not one of the role's
  pages. For the role route model (spec/roles.bend) `/api/history` is a
  `Query`: a read whose page is checked by its handler.
- The file is also rewritten when it reaches twice `history.size` lines,
  so a long uptime does not grow it without bound. Texts are cut at 256
  characters (a failed login's user name is whatever was typed).
- `msg="history"` is also logged at start when unreadable lines were
  dropped.
- Export, to move the history to a new node: `GET /api/history?format=json`
  or `?format=csv` answers a download
  (`homie-history-<UTC date>.json|csv`); the page links both. Roles get
  the rows they see, as the page shows them ("another user", redacted
  addresses, no session digests). CSV columns are
  `time,user,action,target,result,error` (RFC 4180), and a cell starting
  with `=`, `+`, `-`, `@`, a tab or CR gets a leading `'` so a spreadsheet
  does not run it. A full user's JSON is the stored entries, newest first,
  in the shape of `history.jsonl` lines. Restore by writing them back,
  oldest first, before the new instance starts (no import endpoint):
  `jq -c 'reverse[]' homie-history-<date>.json > <state_dir>/history.jsonl`
  The start keeps the last `history.size` and rewrites the file 0600.

## Alternatives

| Option | Why not |
|---|---|
| Link to a log search (RFC 0008) | Needs a log stack; the page should work without one |
| Proxmox and Kubernetes audit logs | Name Homie's token or ServiceAccount, not the person |
| A database | One small append-only file is enough for 1000 lines |

## Testing and rollout

- **Tests:** recording from each action path, the cap and trimming,
  reading a truncated last line, role filtering (targets and other
  users' emails), refused actions recorded as `denied`, the file permission (0600).
- **Rollout:** minor release, on by default; the file appears on the
  first action. Roles gain the page by adding `history` to `pages`; the
  reference demo config and `examples/` add it.
- **Off switch:** `history.enabled = false`; deleting the file clears it.

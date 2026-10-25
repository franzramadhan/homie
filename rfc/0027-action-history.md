# 0027: Action history

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-25 | #114 | - |

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
  result, and the error on failure. No request bodies, no tokens.
- **Kept.** The last 1000 entries in memory and in
  `state_dir/history.jsonl` (append, trimmed to the cap at start), so a
  restart keeps them. Without `state_dir`: memory only, the page says so.
- **Page.** "History" in the sidebar: a table (time, user, action,
  target, result) with the usual filter, newest first, live through the
  existing SSE stream (RFC 0009). Failed entries in the bad tone.
- **Who sees it.** Full users see everything. Roles see entries whose
  target is visible to them (their namespaces and tagged guests) and,
  for logins, only their own; on the demo instance that is the demo's
  own reconciles and power refusals. Other users' emails are never shown
  to a role: they read as "another user".
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

## Alternatives

| Option | Why not |
|---|---|
| Link to a log search (RFC 0008) | Needs a log stack; the page should work without one |
| Proxmox and Kubernetes audit logs | Name Homie's token or ServiceAccount, not the person |
| A database | One small append-only file is enough for 1000 lines |

## Testing and rollout

- **Tests:** recording from each action path, the cap and trimming,
  reading a truncated last line, role filtering (targets and other
  users' emails), the file permission (0600).
- **Rollout:** minor release, on by default; the file appears on the
  first action.
- **Off switch:** `history.enabled = false`; deleting the file clears it.

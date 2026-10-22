# 0015: Save picked workflows without a restart

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | #N | v0.5.0 |

> The workflow picker saves the watch list itself, so adding or removing a
> watched workflow takes effect at once, with no config edit and no restart.

## Problem

RFC 0014 keeps the watch list in the config: the picker only prints
`[[github.watch]]` lines to paste. Every change means editing the config
(a Git commit and a Flux sync in a cluster), then a restart. When Homie runs
on a remote host or in a cluster, a two-click change becomes a deploy.

## Proposal

- The picker gets a **Save** button. Ticked workflows are watched at once;
  unticked ones stop. Each change is logged as `msg="action"` with the user,
  like the other actions.
- Two sources, merged:
  - **config:** `[[github.watch]]` from the config file, as today. Shown with
    a `config` badge and read-only in the UI (GitOps stays the owner).
  - **saved:** what the picker stored. Editable in the UI.
- Where saved watches live, picked automatically:
  - in a cluster (in-cluster or `forward_token` config): a ConfigMap
    `homie-github-watch` in Homie's namespace, written with Homie's own
    ServiceAccount (RBAC: `get`, `create`, `update` on that one name);
  - elsewhere, with `state_dir` set: `state_dir/github-watch.json` (0600);
  - neither: no Save button, the picker prints lines to paste (today's
    behavior).
- Any logged-in user may change the saved list (Homie has no roles). The
  write needs the CSRF header like every action.
- The ConfigMap is not in Git, so Flux leaves it alone; deleting it resets
  the saved list.

### Configuration

| Key | Default | |
|---|---|---|
| `[github] store` | `"auto"` | `"auto"`, `"configmap"`, `"file"` or `"off"` (paste-only, as in RFC 0014) |

The Helm chart and `deploy/kubernetes` get the Role and RoleBinding for the
ConfigMap.

### Logs and metrics

| | |
|---|---|
| `msg="action" action="github.watch" target="owner/repo:workflow"` | who added or removed a watch |

## Alternatives

| Option | Why not |
|---|---|
| Write the config file | Read-only in a cluster (ConfigMap mount, GitOps would revert it) |
| Commit to the GitOps repository | Needs write access to Git; slow; repository layouts differ |
| Only `state_dir` | A cluster deployment has no volume (RFC 0001: no PVC) |
| Per-user lists | RFC 0014 decided one shared list |

## Testing and rollout

- **Tests:** merge of config and saved watches (config wins on duplicates),
  file store round trip, ConfigMap store in kind, CSRF and session on the
  write route.
- **Rollout:** v0.5.0; `store = "auto"`. Existing config watches keep working.
- **Off switch:** `store = "off"`.

# 0003: Flux suspend and resume

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | - | v0.2.0 |

> Let Homie suspend and resume Flux Kustomizations and HelmReleases, as an
> opt-in per cluster, and refuse cleanly where git owns `spec.suspend`.

## Problem

Pausing an app before working on it by hand means `flux suspend` from a
terminal. RFC 0001 left suspend out because a parent Kustomization that sets
`spec.suspend` in git reverts it on its next reconcile, and admission
policies may deny the patch.

## Proposal

- `[kubernetes.<name>] flux_suspend = true` turns the Suspend and Resume
  buttons on (default `false`).
- Action `flux.suspend` / `flux.resume`: merge patch `spec.suspend: true|false`.
- Before patching, Homie reads the object's `managedFields`. If
  `kustomize-controller` (or `helm-controller`) owns `f:spec.f:suspend`, git
  sets the field and the change would be reverted: Homie refuses with
  "suspend is set in git by <parent>; change it there".
- A denial from admission (for example a Kyverno policy) is shown as is.
- Suspended objects get a badge (already shown) and a Resume button.

### Configuration

| Key | Default | |
|---|---|---|
| `kubernetes.<name>.flux_suspend` | `false` | show Suspend and Resume for this cluster |

## Alternatives

| Option | Why not |
|---|---|
| Always on | Silently reverted in clusters where git owns the field |
| Suspend by committing to git | Needs repository credentials and a git workflow inside Homie |

## Testing and rollout

- **Tests:** managedFields ownership detection (owned, not owned, absent).
- **Rollout:** off by default; e2e in a cluster where `suspend` is not in git.
- **Off switch:** `flux_suspend = false`.

## Open questions

1. Should a suspend need the object name typed, like a hard stop?

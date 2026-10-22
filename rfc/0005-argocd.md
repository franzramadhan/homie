# 0005: Argo CD next to Flux

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.2.0 |

> Show Argo CD Applications (sync, health, revision) on the GitOps page and
> allow a refresh, for labs that run Argo CD instead of (or with) Flux.

## Problem

The GitOps page knows only Flux.

## Proposal

- Read `applications.argoproj.io/v1alpha1` through the existing Kubernetes
  integration (same identity rules, no Argo CD API token): sync status,
  health status, revision, target, last operation result and message.
- Action `argocd.refresh`: annotate `argocd.argoproj.io/refresh: normal`
  (hard refresh as `hard`), the same as the Argo CD UI's Refresh button.
- A cluster without the CRD shows no Argo CD section.
- Optional `argocd_url` for deep links to the Argo CD UI.

### Configuration

| Key | Default | |
|---|---|---|
| `kubernetes.<name>.argocd_url` | - | deep links to Applications |

## Alternatives

| Option | Why not |
|---|---|
| Argo CD API server | A separate token and client for data the CRDs already hold |
| Sync action | Changes the live state outside git; refresh is the safe equivalent of Flux reconcile |

## Testing and rollout

- **Tests:** status mapping from fixture Applications; refresh annotation.
- **Rollout:** automatic where the CRD exists; e2e in a kind cluster with Argo CD.

## Decisions

Accepted 2026-10-22 with the proposed defaults:

1. No Sync action; refresh only.

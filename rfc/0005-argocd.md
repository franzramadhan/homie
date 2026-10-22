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

## Changes during implementation

- Overview card and GitOps section appear only where Applications exist
  (Flux sections now do the same).
- `deploy/kubernetes/rbac.yaml`: read on `applications.argoproj.io` (and
  CloudNativePG `clusters`, `backups` for RFC 0011), patch on Applications for
  refresh.
- Tested against a kind cluster with the Argo CD v3.5.6 Application CRD:
  single- and multi-source apps, degraded and failed operations, normal and
  hard refresh (annotation verified on the object), invalid names refused.

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

# 0013: Helm chart and Homebrew tap

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.2.0 |

> Install Homie with `helm install` and `brew install`, published by the
> release workflow.

## Problem

Kubernetes users get a kustomize base; macOS users without mise download a
tarball by hand.

## Proposal

- **Helm chart** `charts/homie` in this repo: Deployment, Service, RBAC
  (with the optional Helm-secrets role), ConfigMap from `config` values,
  `envFrom` a Secret, optional Ingress and HTTPRoute. Published on each tag
  as an OCI chart: `oci://ghcr.io/franzramadhan/charts/homie`.
- **Homebrew tap** `franzramadhan/homebrew-tap` with `Formula/homie.rb`
  (the macOS and Linux tarballs and their SHA256). The release job updates it
  with a fine-grained token limited to that repository.
- `deploy/kubernetes` stays as the dependency-free option.

## Alternatives

| Option | Why not |
|---|---|
| homebrew-core | Needs a track record; a tap first |
| Chart in a separate repository | Version drift from the app |

## Testing and rollout

- **Tests:** `helm lint` and `helm template` in CI; the formula's `test do`
  runs `homie version`.
- **Rollout:** first published with v0.2.0; needs a token secret for the tap.

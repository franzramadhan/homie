# 0007: Deep-link templates

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.2.0 |

> Replace the Headlamp-only deep links with templates, so any Kubernetes UI
> (Lens, Kubernetes Dashboard, Rancher, Argo CD) can be linked.

## Problem

Kubernetes objects deep-link only to Headlamp (`headlamp_url`).

## Proposal

- `[kubernetes.<name>.links]` with one template per object kind: `node`,
  `pod`, `workload`, `flux`. Placeholders: `{cluster}` (the value of
  `headlamp_cluster`, default `main`), `{namespace}`, `{name}`, `{kind}`,
  `{plural}`, and for Flux `{group}` (`kustomize`, `helm` or `source`, the
  Headlamp Flux plugin's route groups). Values are percent-encoded, so a
  name cannot add path segments, a query or a fragment.
- Flux objects get links too (new); with `headlamp_url` they open the
  Headlamp Flux plugin pages.
- `headlamp_url` stays as a shorthand that fills the templates for Headlamp.
- Templates must start with `http://` or `https://` (validated at start; the
  UI also renders only http(s) links).

### Configuration

| Key | Default | |
|---|---|---|
| `kubernetes.<name>.links.{node,pod,workload,flux}` | from `headlamp_url` | URL templates |

## Changes during implementation

- `{group}` added: Headlamp's Flux plugin routes differ per controller
  (`/flux/kustomize/kustomizations/...`, `/flux/helm/helmreleases/...`,
  `/flux/source/<plural>/...`).

## Alternatives

| Option | Why not |
|---|---|
| One built-in preset per UI | Every UI's URL scheme changes between versions; templates follow without a release |

## Testing and rollout

- **Tests:** placeholder expansion and encoding; scheme validation.
- **Rollout:** `headlamp_url` keeps working unchanged.

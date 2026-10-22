# 0007: Deep-link templates

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | - | v0.2.0 |

> Replace the Headlamp-only deep links with templates, so any Kubernetes UI
> (Lens, Kubernetes Dashboard, Rancher, Argo CD) can be linked.

## Problem

Kubernetes objects deep-link only to Headlamp (`headlamp_url`).

## Proposal

- `[kubernetes.<name>.links]` with one template per object kind: `node`,
  `pod`, `workload`, `flux`. Placeholders: `{cluster}`, `{namespace}`,
  `{name}`, `{kind}`, `{plural}`. Values are URL-encoded.
- `headlamp_url` stays as a shorthand that fills the templates for Headlamp.
- Templates must start with `http://` or `https://` (validated at start; the
  UI also renders only http(s) links).

### Configuration

| Key | Default | |
|---|---|---|
| `kubernetes.<name>.links.{node,pod,workload,flux}` | from `headlamp_url` | URL templates |

## Alternatives

| Option | Why not |
|---|---|
| One built-in preset per UI | Every UI's URL scheme changes between versions; templates follow without a release |

## Testing and rollout

- **Tests:** placeholder expansion and encoding; scheme validation.
- **Rollout:** `headlamp_url` keeps working unchanged.

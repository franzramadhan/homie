# 0021: Image updates

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-24 | #73 | v0.10.0 |

> Show which running container images have a newer version. Versions
> only: images on a moving tag such as `latest` are flagged as
> unversioned, with the newest version available. Builds on the upstream
> clients of RFC 0020. Read-only: Homie never bumps an image.

## Problem

An image pinned to `1.27.3` stays there until someone notices, and an
image on `latest` runs whatever was pulled at the last restart, with no
version to tell. Renovate covers images written in git, not images
started by a chart's defaults, an operator, or a `kubectl run`.

## Proposal

- **Installed**: `spec.containers[].image` and `initContainers` of running
  pods, grouped by image reference, with the namespaces and workloads
  using it. Digest-pinned references (`repo:1.2.3@sha256:...`) use their
  tag; a digest without a tag is shown as pinned, no lookup.
- **Upstream**: the OCI distribution client from RFC 0020,
  `GET /v2/<name>/tags/list`, with the same anonymous token challenge and
  `[[updates.registries]]` credentials. Docker Hub short names
  (`nginx`, `user/app`) are expanded to `docker.io/library/...`.
- **Tag shape.** Tags parse as dotted numbers, an optional `v` prefix and
  a suffix (`1.27.3-alpine`, `v2.1.0.4`, `1.27`). A candidate must have the
  same shape as the running tag: same number of parts, same prefix, same
  suffix. `1.27.3-alpine` is compared with `1.28.0-alpine`, never with
  `1.28.0` or `latest`. Pre-release suffixes (`-rc.1`, `-beta`, `-dev`) are
  skipped. Newest patch of the running minor and newest overall are shown,
  as in RFC 0020.
- **Unversioned tags** (`latest`, `stable`, `main`, any tag without
  numbers, or no tag at all): no comparison. The row says "unversioned
  tag" and shows the newest version the repository has (the newest tag of
  the plain `X.Y.Z` shape, with or without `v`), as the version to pin
  to. Neutral, like every update row.
- **Cache and limits**: as RFC 0020 (12 h per repository, at most 4 at a
  time, one lookup per repository however many pods use it, last result
  kept on failure).
- **Ignore**: `updates.ignore` (RFC 0020) also takes image globs, for
  example `registry.k8s.io/pause*` or images an operator manages.
- **Display**: an "Update" column per workload on the Workloads page, and
  `image` rows on the Updates page (RFC 0020), filterable to "unversioned".
  Neutral, never notifies, hidden from roles.

### Configuration

| Key | Default | |
|---|---|---|
| `updates.upstream.images` | `false` | newest image versions |

Registries, credentials and ignore globs are the RFC 0020 keys.

### Logs and metrics

| | |
|---|---|
| `msg="upstream" kind="image" repo result ms` | as RFC 0020 |

## Changes during implementation

- Display: an Images section on the Updates page (RFC 0020) and a line on
  the Updates card, instead of an "Update" column on the Workloads page.
  Typing "unversioned" in the table filter lists the images to pin.
- The `v` prefix is part of the shape, for charts and distributions too.
- Workloads are named from the pod's owner: a ReplicaSet as its
  Deployment, anything else by kind and name, a bare pod as itself.
- The first lookup is slow on a real cluster (52 images, 74 s through
  four parallel lookups in the reference lab), then cached 12 hours.
- Released with RFC 0020 in v0.10.0.

## Alternatives

| Option | Why not |
|---|---|
| Comparing the digest of `latest` with the running `imageID` | Answers "is there a newer build", not "which version"; needs a manifest request per image and depends on how each runtime records multi-arch digests. Pinning a version is the fix either way |
| Resolving which version `latest` points to (matching digests across tags) | A manifest request per candidate tag, and many repositories do not tag `latest` and a version with the same build |
| Strict semver only | Too many images tag `1.27`, `v2.1.0.4` or `1.27-alpine`; the shape rule handles them |
| Reading imagePullSecrets for registry credentials | Needs Secret read access; credentials go in config like every other token |
| Flux image automation (`ImagePolicy`) as the source | Only where it is installed and configured per image; can be an extra source later |

## Testing and rollout

- **Tests:** reference parsing (short Docker Hub names, ports in registry
  hosts, digests with and without tags); tag shape parsing and comparison
  (prefix, suffix, four parts, pre-releases); unversioned tags with the
  newest plain version; ignore globs; one lookup per repository for many
  pods.
- **Rollout:** after RFC 0020; off by default. Network policies need
  egress to the registries in use.
- **Off switch:** `updates.upstream.images = false`.

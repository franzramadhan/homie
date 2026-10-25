# 0026: Release notes on the Updates page

| Status | Date | PR | Release |
|---|---|---|---|
| implemented | 2026-10-25 | #113 | v0.13.0 |

> Link each pending chart and image update to the release notes between
> the running and the newest version, and mark the updates whose notes
> mention a security fix, so the Updates page says what to do first.
> On by default wherever chart or image updates are on.

## Problem

The Updates page (RFC 0020, RFC 0021) says "12 charts and 20 images
behind" and stops there. Finding out what changed means looking up each
project's repository by hand. Nothing tells a routine bump from a CVE
fix, so updates are either all done at once or put off together.

## Proposal

- **Finding the repository.** In this order, first match wins:
  - charts: `sources` and `home` in the Helm index entry, or
    `org.opencontainers.image.source` in an OCI chart's manifest
    annotations;
  - images: `org.opencontainers.image.source` in the image config labels
    (one extra registry request per repository, cached with the tags);
  - `[[updates.sources]]`: a glob on the chart or image name to a GitHub
    repository, for projects that set neither.
  Only `github.com` repositories are read; others get a link to the
  repository, no notes.
- **Release notes.** GitHub releases of that repository
  (`GET /repos/{owner}/{repo}/releases`, `updates.github_token` when set),
  matched by tag to the versions between the running one (exclusive) and
  the newest (inclusive). Tags with a prefix (`v1.2.3`, `chart-1.2.3`)
  match by their version part. Kept 12 hours, like the version lookups.
- **Security mark.** An update is marked "security" when one of those
  release bodies matches `CVE-\d{4}-\d+`, `GHSA-`, or a "security"
  heading or word. The mark is a hint, says so in its tooltip, and never
  changes a card's tone or notifies.
- **Display.** On each update row: the number of releases in between, a
  link to the compare view (`/compare/<running>...<newest>`) or the
  release list, and the security mark. Expanding the row shows the
  release titles and dates, and the first lines of each body as plain
  text (no HTML from upstream is rendered). The Updates table sorts
  security first. Hidden from roles, like the Updates page.

### Configuration

| Key | Default | |
|---|---|---|
| `updates.upstream.notes` | `true` | read release notes for the chart and image updates found (only with `updates.upstream.charts` or `images` on); `false` turns it off |
| `[[updates.sources]]` | none | `name` (glob) and `github` (`owner/repo`) for projects without metadata |

### Logs and metrics

| | |
|---|---|
| `msg="upstream" kind="notes" repo result ms` | as RFC 0020 |
| `homie_updates_security{kind}` | updates marked security, per kind |

## Changes during implementation

- Repository lookups: an image manifest (for an index, its linux/amd64
  manifest too) and, only when no annotation names the source, the config
  blob: up to three registry requests per repository, cached 12 hours like
  the tags. OCI charts read the same way; their config blob is the
  `Chart.yaml`, so its `sources` and `home` count too.
- Image config labels are inherited from the base image
  (`external-dns` carried `GoogleContainerTools/distroless`), so a label
  counts only when its GitHub repository plausibly names the image: a
  path part (but `library`) and the owner or the repository contain one
  another (letters and digits only, at least 3 characters) or share a
  word of 3 or more. Otherwise it is ignored and `[[updates.sources]]`
  applies. Manifest annotations and chart metadata are trusted as they are.
- Order: metadata naming a GitHub repository wins, then
  `[[updates.sources]]` (matched on the chart name, or on the image as
  written and as `registry/repository`). A non-GitHub URL from metadata is
  the repository link only when no source matches.
- Tags: in a repository of several projects, tags `<name>-<version>` win;
  else plain version tags (`v` or not); else every tag. Versions compare
  by their numbers only, so `1.27.3-alpine` finds `v1.28.0`. Pre-releases
  and drafts are left out. Only the newest 100 releases are read (one
  request per repository).
- The compare view needs the running version to be a release itself;
  otherwise the link is the release list. Unversioned and digest-pinned
  images get no notes (no running version).
- Every failure, not only a 403 or 429, keeps the last notes, is logged
  once until a lookup succeeds, and is retried after 30 minutes (the
  failed-lookup interval of RFC 0020), not at every refresh of the Updates
  query: a repository that answers 404 or a lasting rate limit would
  otherwise spend the 60 requests an hour. The page shows one line with
  the number of repositories not read.
- Bodies: HTML tags and comments are removed on the server, then the first
  6 non-empty lines, at most 600 characters, rendered as text.
- "security" counts as a whole word only, so `securityContext` does not
  mark an update.
- Credentials no longer follow a redirect to another host (registries send
  blobs to a CDN).
- `homie_updates_security` is left out when no cluster could be read, like
  `homie_updates_available`. The log line also has `releases`.
- Without `updates.github_token`, GitHub lookups (these notes and the
  RFC 0020 release lookups) use the `[github]` credential when it reads
  github.com: the fine-grained token, or an installation token of the
  App's first installation (5000 requests an hour, public repositories
  included), cached and renewed 5 minutes before it expires. 60 an hour
  per IP was not enough for the reference lab. When minting fails the
  lookups run anonymously, logged once. An explicit
  `updates.github_token` always wins.
- Display: a "release notes" column on the Charts and Images tables; the
  security mark is a neutral badge, filterable by typing "security".

## Alternatives

| Option | Why not |
|---|---|
| A vulnerability scanner (Trivy, Grype) on running images | Scans what runs, not what an update fixes; heavy, and security presets were rejected (RFC 0004) |
| GitHub Security Advisories API | Covers only projects that file advisories; release notes cover more, and the advisory id usually appears in them |
| Renovate's dashboard | Covers what is written in git, not charts' default images (RFC 0021) |
| Rendering upstream Markdown | Untrusted HTML in an authenticated page; plain text is enough to decide |

## Testing and rollout

- **Tests:** repository discovery from index, annotations, labels and
  `updates.sources`; tag matching with prefixes; the security patterns;
  plain-text rendering (no HTML passes through).
- **Rollout:** minor release, on by default wherever chart or image
  updates are on: one GitHub request per repository with a pending
  update, every 12 hours (the reference lab: about 30). Without
  `updates.github_token` GitHub allows 60 per hour, so lookups run at
  most 4 at a time, and a 403 or 429 keeps the last notes and retries at
  the next refresh, logged once.
- **Off switch:** `updates.upstream.notes = false`.

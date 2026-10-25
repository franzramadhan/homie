# 0026: Release notes on the Updates page

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-25 | #113 | - |

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

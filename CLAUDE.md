# CLAUDE.md

Homie: one page for a whole homelab (Proxmox VE, Kubernetes, Flux, Grafana,
security findings). Rust backend with an embedded TanStack SPA. The design
and the milestones are in [rfc/0001-homie-foundation.md](rfc/0001-homie-foundation.md).
`AGENTS.md` is a symlink to this file: edit `CLAUDE.md` only.

## Layout

| Path | Contents |
|---|---|
| `src/config.rs`, `secret.rs`, `http.rs`, `quiet.rs`, `cache.rs` | config layering, secret references, per-integration TLS, quiet hours, query cache and snapshot |
| `src/integrations/` | one module per domain: `proxmox`, `kubernetes`, `helm`, `flux`, `grafana`, `security` (presets), `outage` (Tailscale, healthchecks); `mod.rs` builds them and serves named queries |
| `src/auth/` | `oidc` (provider login), `local` (username + argon2 password, generated first password, lockout), `mod.rs` (sessions, cookie) |
| `src/server/` | `api` (meta, queries, actions), `guard` (Host, CSRF, session), `login`, `assets` (embedded UI) |
| `src/check.rs` | `homie check` |
| `web/` | Vite + React + TanStack Router/Query/Table SPA; `src/api.ts` mirrors the Rust JSON shapes |
| `deploy/kubernetes`, `contrib/`, `examples/` | kustomize base, launchd/systemd units, example configs (tested by `config::tests::examples_load`) |

Build the UI before a release build: the binary embeds `web/dist`
(`cd web && bun install && bun run build`). Debug builds read it from disk,
and a missing `web/dist` only means the UI answers 503.

## Product rules

- Generic first. Every integration is optional and configurable through the
  TOML file and `HOMIE__*` environment variables. The author's lab
  (Cloudflare Access, Tailscale, k0s, VictoriaMetrics/Logs) is the reference
  deployment: it belongs in `examples/` and the RFC's reference section,
  never in defaults or required config.
- Design changes go through an RFC in `rfc/` (copy `0000-template.md`).

## Public repo hygiene

This repository is public. Never commit lab identifiers: the tailnet name,
the Cloudflare Access team or client ids, email addresses, LAN IPs, VM names
or VMIDs, tokens. Use placeholders such as `<tailnet>`, `<team>`, `<email>`.

## Writing

No em dashes or en dashes anywhere (docs, comments, commit messages). Use a
comma, colon, parentheses or a new sentence.

## CI and releases

- `.github/workflows/ci.yml` runs on pull requests only: `cargo fmt --check`,
  `cargo clippy -D warnings`, `cargo test`.
- `.github/workflows/build.yml` runs on `v*` tags only: native per-target
  binaries, the multi-arch `scratch` image from the Linux binaries
  (`ghcr.io/franzramadhan/homie`), and a GitHub release with the tarballs and
  `SHA256SUMS`. Merges to `main` publish nothing.
- Versioning: `v0.MINOR.PATCH` only. 0.x is alpha (said in the README); no
  v1 until the user decides. No `-rc` suffixes for now. Releases are not
  marked pre-release: mise's `github:` backend and `releases/latest` skip
  pre-releases.
- Release: bump `version` in `Cargo.toml`, merge, then tag `v<version>`.
  The build fails when the tag and `Cargo.toml` disagree.
- Rust is pinned in `rust-toolchain.toml`. Bump it there.

## GitHub Actions

- Always use the latest release of each action, pinned by full commit SHA
  with the version as a comment: `uses: owner/repo@<40-char sha> # vX.Y.Z`.
  Never pin by tag or branch.
- Resolve a tag to its commit SHA (annotated tags need the second lookup):

  ```bash
  r=actions/checkout; t=$(gh api repos/$r/releases/latest --jq .tag_name)
  set -- $(gh api repos/$r/git/ref/tags/$t --jq '.object.type, .object.sha')
  [ "$1" = tag ] && set -- commit $(gh api repos/$r/git/tags/$2 --jq .object.sha)
  echo "$r@$2 # $t"
  ```

- `actions/checkout` always with `persist-credentials: false`.
- `permissions: contents: read` at the top; grant more per job only where
  needed.
- Pass `${{ }}` values into `run:` scripts through `env:`, never inline.

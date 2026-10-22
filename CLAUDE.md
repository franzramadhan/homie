# CLAUDE.md

Homie: one page for a whole homelab (Proxmox VE, Kubernetes, Flux, Grafana,
security findings). Rust backend with an embedded TanStack SPA. The design
and the milestones are in [rfc/0001-homie-foundation.md](rfc/0001-homie-foundation.md).
`AGENTS.md` is a symlink to this file: edit `CLAUDE.md` only.

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
- `.github/workflows/build.yml` runs on pushes to `main` and `v*` tags:
  native per-target binaries, then the multi-arch `scratch` image from the
  Linux binaries (`ghcr.io/franzramadhan/homie`), then on tags a GitHub
  release with the tarballs and `SHA256SUMS`.
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

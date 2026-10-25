# Roadmap

Every feature starts as an RFC (`rfc/`, see `CLAUDE.md`). A release lists
the RFCs it implements. Versions stay 0.x (alpha) until a 1.0 is decided.

| RFC | Feature | Status | Target |
|---|---|---|---|
| [0001](rfc/0001-homie-foundation.md) | Foundation | implemented | v0.1.0 |
| [0002](rfc/0002-design-system.md) | Design system | implemented | v0.2.0 |
| [0003](rfc/0003-flux-suspend.md) | Flux suspend and resume | rejected | - |
| [0004](rfc/0004-security-presets.md) | Security presets: custom queries, Loki, Falco, Trivy, CrowdSec | rejected | - |
| [0005](rfc/0005-argocd.md) | Argo CD next to Flux | implemented | v0.2.0 |
| [0006](rfc/0006-alertmanager.md) | Prometheus Alertmanager as an alert source | rejected | - |
| [0007](rfc/0007-link-templates.md) | Deep-link templates | implemented | v0.2.0 |
| [0008](rfc/0008-log-search.md) | Log search | implemented | v0.2.0 |
| [0009](rfc/0009-live-updates.md) | Live updates (SSE) | implemented | v0.3.0 |
| [0010](rfc/0010-guest-migrate.md) | Guest migration | rejected | - |
| [0011](rfc/0011-service-status.md) | Service status cards (OpenBao, CNPG, backups) | implemented | v0.2.0 |
| [0012](rfc/0012-sparklines.md) | Sparklines | implemented | v0.2.0 |
| [0013](rfc/0013-packaging.md) | Helm chart and Homebrew tap | implemented | v0.2.0 |
| [0014](rfc/0014-github-actions.md) | GitHub Actions workflow monitoring | implemented | v0.2.0 |
| [0015](rfc/0015-github-watch-store.md) | Save picked workflows without a restart | implemented | v0.5.0 |
| [0016](rfc/0016-roles-demo.md) | Roles and a public demo | implemented | v0.6.0 |
| [0017](rfc/0017-github-session-watches.md) | Personal GitHub watches per session (demo Workflows page) | implemented | v0.7.0 |
| [0018](rfc/0018-certificate-expiry.md) | Certificate expiry (TLS probes, any issuer) | implemented | v0.8.0 |
| [0019](rfc/0019-notifications.md) | Notifications (ntfy, Slack-compatible, webhook) | implemented | v0.9.0 |
| [0020](rfc/0020-pending-updates.md) | Pending updates: Proxmox, distribution, Flux, charts | implemented | v0.10.0 |
| [0021](rfc/0021-image-updates.md) | Image updates (versions, unversioned tags flagged) | implemented | v0.10.0 |
| [0022](rfc/0022-log-tail.md) | Live log tail | implemented | v0.11.0 |
| [0023](rfc/0023-console.md) | Proxmox consoles (links, embedded guest console) | implemented | v0.11.0 |
| [0024](rfc/0024-metrics.md) | Metrics endpoint and JSON logs | draft | - |
| - | Per-user Proxmox tickets | not written | later |

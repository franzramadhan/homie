# 0006: Prometheus Alertmanager as an alert source

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | - | v0.2.0 |

> Read active alerts straight from Alertmanager, for labs that alert with
> Prometheus and Alertmanager instead of Grafana.

## Problem

Alerts come only from Grafana-managed rules.

## Proposal

- `[alertmanager.<name>] url`, optional `token` (bearer) or `username` +
  `password` (basic), the shared TLS options, `quiet`, `ignore_alerts`.
- `GET /api/v2/alerts?active=true&silenced=false&inhibited=false`; mapped
  to the same alert rows as Grafana (state firing, severity, summary,
  labels, since).
- The overview gets one alerts card per Alertmanager; the Alerts page lists
  them next to the Grafanas.

### Configuration

| Key | Default | |
|---|---|---|
| `alertmanager.<name>.url` | - | Alertmanager base URL |
| `alertmanager.<name>.token` or `username`/`password` | none | auth |
| `alertmanager.<name>.ignore_alerts` | `["Watchdog"]` | names never listed |

## Alternatives

| Option | Why not |
|---|---|
| Prometheus `/api/v1/alerts` | Ignores silences and inhibitions |

## Testing and rollout

- **Tests:** v2 alert mapping, silenced alerts excluded, ignore list.
- **Rollout:** new optional section.

## Open questions

1. Show silenced alerts in a collapsed group?

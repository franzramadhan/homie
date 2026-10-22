# 0011: Service status cards

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.2.0 |

> Status cards for the services a lab depends on: secret stores, database
> clusters and backups.

## Problem

OpenBao/Vault sealed, a database replica down or a backup that stopped
running are only visible in Grafana or by asking each tool.

## Proposal

- `[openbao.<name>]` (also Vault): `GET /v1/sys/health` (unauthenticated):
  initialized, sealed, standby, version. Sealed is red.
- CloudNativePG through the Kubernetes integration: `clusters.postgresql.cnpg.io`
  phase, ready instances, current primary, `status.lastSuccessfulBackup` age;
  a backup older than `max_backup_age` (default 26 h) is amber.
- Backups: healthchecks already covers backup jobs; the card links them.
- One "Services" section on the overview, cards hidden when not configured.

### Configuration

| Key | Default | |
|---|---|---|
| `openbao.<name>.url` | - | OpenBao or Vault address (TLS options apply) |
| `kubernetes.<name>.cnpg.max_backup_age` | `26h` | amber threshold |

## Alternatives

| Option | Why not |
|---|---|
| Generic HTTP checks only | Loses meaning (sealed vs standby, backup age) |

## Testing and rollout

- **Tests:** health code mapping (200, 429, 472, 473, 501, 503); CNPG status fixtures.
- **Rollout:** optional sections.

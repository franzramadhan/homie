# 0004: Security presets: custom queries, Loki, more tools

| Status | Date | PR | Release |
|---|---|---|---|
| rejected | 2026-10-22 | #16 | - |

> Rejected 2026-10-22: not planned. Kept for the record.
>
> Make security presets configurable (own query, Loki or VictoriaLogs) and
> add presets for Falco, Trivy Operator and CrowdSec, plus fully custom
> presets.

## Problem

The four presets have built-in VictoriaLogs queries and selectors that fit
the reference lab. Labs on Loki, or running Falco, Trivy Operator or
CrowdSec, get nothing.

## Proposal

- Every log or metric preset takes `query` (replaces the built-in one) and
  `datasource_type` (`victorialogs`, `loki`, `prometheus`; default from the
  preset).
- LogQL variants of the Tetragon and Wazuh presets.
- New presets:
  - `falco`: Falco events by rule and priority from logs (Falcosidekick or
    the pod log), LogsQL or LogQL.
  - `trivy`: Trivy Operator `VulnerabilityReport` CRDs through the
    Kubernetes integration: critical and high counts per namespace and image.
  - `crowdsec`: active decisions and top scenarios from CrowdSec's Prometheus
    metrics (`cs_active_decisions`, `cs_alerts`).
- `[security.custom.<name>]`: any PromQL, LogsQL or LogQL query with
  `columns` and `total_column`, rendered like the built-in presets.

### Configuration

| Key | Default | |
|---|---|---|
| `security.<preset>.query` | built in | replace the preset's query |
| `security.<preset>.datasource_type` | per preset | `victorialogs`, `loki`, `prometheus` |
| `security.custom.<name>.{grafana,datasource,datasource_type,query,columns,total_column}` | - | user-defined preset |

## Alternatives

| Option | Why not |
|---|---|
| Only custom presets | Every user rewrites the same Falco or Trivy query |
| Query each tool's own API | One more credential and client per tool; the data is already in logs, metrics or CRDs |

## Testing and rollout

- **Tests:** query building per datasource type; table mapping from
  Prometheus vectors, LogsQL NDJSON and LogQL matrix results.
- **Rollout:** built-in queries unchanged unless `query` is set.

## Open questions

1. CrowdSec through its Local API (needs a machine credential) instead of metrics?

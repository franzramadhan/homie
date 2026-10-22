# 0008: Log search

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.2.0 |

> A log search page over the configured log datasources (VictoriaLogs,
> Loki) through Grafana, with context links from pods, nodes and alerts.

## Problem

Every log question leaves Homie for Grafana Explore.

## Proposal

- Page `/logs`: datasource picker (only those listed in config), query box,
  time range (15 m to 7 d), result table with time, stream labels and
  message; newest first; at most 1000 rows.
- Backend: `POST /api/logs/{grafana}/{datasource}` with `{query, from, to,
  limit}`; LogsQL via `/select/logsql/query`, LogQL via
  `/loki/api/v1/query_range`, through the Grafana datasource proxy.
- Context links: a pod row opens `/logs` prefilled for that pod; the same
  for nodes, Wazuh agents and Tetragon policies.
- Guardrails: only datasources listed in `logs.datasources`; the time range
  and limit are capped server-side; queries are logged with the user.

### Configuration

| Key | Default | |
|---|---|---|
| `logs.datasources` | none | list of `{grafana, datasource, type, label}` |
| `logs.max_rows` | `1000` | cap per query |

## Changes during implementation

- Datasource entries: `label`, `grafana`, `datasource`, `type`
  (`victorialogs` or `loki`), and optional `pod_query` / `node_query`
  templates (`{namespace}`, `{pod}`, `{node}`) that drive the "logs" links
  on pods and nodes. Wazuh and Tetragon context links were left out: their
  security tables already link to Grafana Explore.
- Endpoint `POST /api/logs` with `{source, query, minutes, limit}`: a POST
  for the body, with the write checks (X-Homie, Origin); every query is
  logged with the user (`msg="logs"`).
- Terminal color codes are stripped from messages.
- A prefilled link runs its search on open; the URL keeps source, query and
  range.

## Alternatives

| Option | Why not |
|---|---|
| Links to Grafana Explore only | Status quo |
| Query the stores directly | Needs a route to each store; RFC 0001 chose Grafana as the gateway |

## Testing and rollout

- **Tests:** caps enforced; LogsQL and LogQL result parsing; datasource allowlist.
- **Rollout:** page appears only when `logs.datasources` is set.

## Decisions

Accepted 2026-10-22 with the proposed defaults:

1. Live tail comes later, in its own RFC.

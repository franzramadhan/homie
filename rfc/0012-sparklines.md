# 0012: Sparklines

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.2.0 |

> Small trend lines next to the numbers that matter: node and guest CPU and
> memory over the last hour.

## Problem

Bars show only "now". A node at 85% memory reads the same whether it is
climbing or steady.

## Proposal

- Proxmox: `GET /nodes/{node}/rrddata?timeframe=hour` and the guest
  equivalent (no Grafana needed), averaged to 30 points.
- Kubernetes nodes: optional, through a configured Prometheus-compatible
  datasource (`node_cpu_seconds_total`, `node_memory_*`).
- Rendered as inline SVG polylines (no chart library), colored by the
  current status, with a tooltip of min, max and last.
- Fetched lazily per visible table, cached 60 s.

## Alternatives

| Option | Why not |
|---|---|
| A chart library | Bundle size for a 60-pixel line |
| Embedding Grafana panels | Rejected in RFC 0001 |

## Testing and rollout

- **Tests:** RRD parsing, downsampling.
- **Rollout:** Proxmox first; Kubernetes when a datasource is set.

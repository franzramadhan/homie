# 0025: Disk-full forecast

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-25 | #112 | - |

> Say when storage will be full, not only how full it is now: Proxmox
> storages from their own history, Kubernetes volumes from a PromQL query.
> A storage due to fill within the warning window turns its card yellow
> and can notify.

## Problem

A storage at 70% reads the same whether it has been at 70% for a year or
grew 5% since Monday. Today Homie shows the meter only, so a backup
store or a database volume fills up between two looks at the page.
Kubernetes PersistentVolumeClaims are not shown at all.

## Proposal

- **Proxmox storages.** `GET /nodes/{node}/storage/{storage}/rrddata?timeframe=week`
  (`used`, `total`), already reachable with the Proxmox token; shared
  storages read once. No Grafana needed.
- **Kubernetes volumes.** Optional, from the existing
  `[kubernetes.<name>.metrics]` datasource (RFC 0012): two PromQL queries
  returning one series per volume, `used` and `capacity`, with
  `volume_label` (default `persistentvolumeclaim`) and `namespace_label`
  (default `namespace`). No built-in metric names, as in RFC 0012.
  Example for kubelet metrics: `kubelet_volume_stats_used_bytes` and
  `kubelet_volume_stats_capacity_bytes`.
- **Forecast.** A least-squares line over the last 7 days of `used`
  (hourly points). Days until full = (capacity - used now) / slope. No
  forecast when the slope is flat or negative, with fewer than 24 points,
  or when the fit is poor (R² below 0.5: a storage that jumps up and down
  says nothing about next week). Shown as "full in 9 days" or "steady".
- **Status.** Warn when a storage is due to fill within `warn_days`
  (default 14), bad within `bad_days` (default 3). The meter thresholds
  stay as they are; the forecast only raises the tone, never lowers it.
- **Display.** A "Full in" column on the Infrastructure storage table and
  a Volumes table on the Workloads page (namespace, claim, used,
  capacity, full in, sparkline of the week). Roles see volumes only in
  their namespaces and storages only on the Infrastructure page, as today.
- **Cards and notify.** The Proxmox card and the Kubernetes card take the
  worst forecast tone; notify (RFC 0019) sends it like any card change.
- Cached 15 minutes per storage and per cluster.

### Configuration

| Key | Default | |
|---|---|---|
| `forecast.enabled` | `true` | `false` hides the column and never raises a tone |
| `forecast.warn_days` | `14` | warn when full within this many days |
| `forecast.bad_days` | `3` | bad when full within this many days |
| `kubernetes.<name>.metrics.volume_used` | unset | PromQL, one series per volume; unset: no Volumes table |
| `kubernetes.<name>.metrics.volume_capacity` | unset | PromQL, same labels |
| `kubernetes.<name>.metrics.volume_label` | `persistentvolumeclaim` | |
| `kubernetes.<name>.metrics.namespace_label` | `namespace` | |

### Logs and metrics

| | |
|---|---|
| `msg="query" source="forecast-pve/<name>"` | as every query |
| `homie_storage_full_seconds{source,storage}` | seconds until full, absent when there is no forecast |

## Changes during implementation

- Two named queries, `forecast-pve/<name>` and `forecast-k8s/<name>`,
  each cached 15 minutes for the whole cluster (one entry, not one per
  storage). Logs name them as every query does: `source="forecast-pve:<name>"`.
  For roles they belong to the Infrastructure and Workloads pages
  (`spec/queries.bend` has them too).
- Proxmox points are what `rrddata?timeframe=week&cf=AVERAGE` returns,
  hourly or coarser depending on the Proxmox version; the 24-point minimum
  still applies. A storage whose history cannot be read is left out
  ("no data" in the column).
- Kubernetes: `volume_used` is read as a 7-day range at a 1 h step,
  `volume_capacity` as an instant query (the current size). Setting only
  one of the two is a config error.
- Roles: `forecast-pve` returns no storages to a role without reading
  Proxmox, as the overview shows a role no storage; `forecast-k8s` keeps
  the role's namespaces only.
- `forecast.enabled = false` hides the "Full in" column and the tone; the
  Volumes table (used, capacity, the week) still shows when the two
  queries are set.
- Card reasons read `local@pve-01 full in 9 days` (`storage` alone for a
  shared storage, `namespace/claim` for a volume). The column says
  "within a day" under a day and "over a year" beyond 365 days.
- `homie_storage_full_seconds` is read at scrape time through the shared
  cache, so only with `metrics.cards` (RFC 0024). Labels: `source` is
  `pve/<name>` or `k8s/<name>`, `storage` as in the card reasons.
  `examples/metrics-alerts.yaml` has a rule for it.

## Alternatives

| Option | Why not |
|---|---|
| `predict_linear()` in the user's Prometheus | Needs Prometheus for Proxmox storage too, and every user writes it again |
| A forecast from the last hour (sparkline data) | A backup job alone moves it; a week smooths daily jobs |
| Exponential or seasonal models | Explaining "why 9 days" matters more than accuracy; a straight line is readable |
| ZFS pool data (`/disks/zfs`) | No history; the storage rrddata already covers zfspool storages |

## Testing and rollout

- **Tests:** the fit (growing, flat, shrinking, noisy, too few points),
  rrddata and PromQL parsing, the tone rules, role filtering of volumes.
- **Rollout:** minor release; on by default for Proxmox, Kubernetes
  volumes only with the two queries set.
- **Off switch:** `forecast.enabled = false`.

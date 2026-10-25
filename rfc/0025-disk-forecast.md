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

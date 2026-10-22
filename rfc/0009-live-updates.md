# 0009: Live updates

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #16 | v0.3.0 |

> Push changes to the browser as they happen (server-sent events), so an
> action's effect and a new failure show up without waiting for the next poll.

## Problem

Every view polls every `refresh_seconds` (15 s). After an action the page
lags, and many open tabs multiply backend calls.

## Proposal

- `GET /api/stream`: one SSE connection per tab; the server sends
  `{key, envelope}` whenever a query result it already serves changes.
- Kubernetes and Flux: server-side watches (kube-rs `watcher`) per cluster
  feed the cache; with `forward_token`, watches run per user session and
  stop when the session ends.
- Other integrations keep polling, but on the server, once per key for all
  tabs.
- The SPA falls back to polling when SSE is unavailable.

## Alternatives

| Option | Why not |
|---|---|
| WebSockets | Bidirectional is not needed; SSE works through every proxy and with the existing cookie |
| Shorter polling | More load for the same staleness |

## Testing and rollout

- **Tests:** watch reconnect, per-session teardown, fallback to polling.
- **Rollout:** behind `live_updates = true` first, default after one release.

## Decisions

Accepted 2026-10-22 with the proposed defaults:

1. The memory budget for per-user watches is measured during the build and
   written back here (see Implementation).

## Implementation

- `GET /api/stream?q=kind/name,...` (at most 64 keys). Each round runs the
  keys through the same cache as `/api/q`, so tabs share backend calls, and
  sends `{key, envelope}` for each envelope that changed (a refetch counts:
  `fetched_at` drives the "updated" label). A round runs every
  `refresh_seconds`, or as soon as `Cache::invalidate` runs (actions, watch
  events), at most one round per 2 s.
- Watches are per stream, not per cluster: metadata-only
  (`PartialObjectMeta`), nothing stored, an event only invalidates the query.
  `k8s` watches Pods, Nodes and Deployments; `flux` the Flux kinds; `argocd`
  Applications. `helm`, `cnpg` and the rest stay on the refresh tick. A kind
  whose CRD is missing (404) is remembered and not watched again until a
  restart.
- With `forward_token`, the stream re-reads the session each round: it ends
  when the session ends and restarts its watches with a refreshed ID token.
  Closing the tab drops the stream and its watches.
- The SPA opens one `EventSource` per tab for the queries on screen and stops
  polling while it is open; on error it polls again while `EventSource`
  reconnects. The server sends `X-Accel-Buffering: no` for buffering proxies.

Measured (kind, release build, 2026-10-22): RSS 10 MB idle, 14 MB with one
stream, 31 MB with 50 streams (about 0.4 MB per stream with three watches).
Ten streams added 30 apiserver watches (3 each, the Flux CRDs absent); after
they closed Homie held none. A scale event reached the stream within a second
with `refresh_seconds = 30`. In the reference lab the stream passes the
Tailscale proxy unbuffered and the Workloads page updates right away.

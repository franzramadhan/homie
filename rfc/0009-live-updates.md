# 0009: Live updates

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | - | v0.2.0 |

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

## Open questions

1. Memory budget for per-user watches in `forward_token` mode?

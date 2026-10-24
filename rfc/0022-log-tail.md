# 0022: Live log tail

| Status | Date | PR | Release |
|---|---|---|---|
| implemented | 2026-10-24 | #91 | v0.11.0 |

> A **Follow** toggle on the Logs page: new lines matching the query appear
> as they arrive, for VictoriaLogs and Loki, through the same Grafana path as
> log search (RFC 0008).

## Problem

To watch a rollout or a failing pod, the user presses Search again and again,
or leaves for Grafana Explore's live mode. RFC 0008 deferred live tail to its
own RFC.

## Proposal

- **Follow** button next to Search on `/logs`. On: the page runs the search
  once, then asks every `logs.tail_seconds` (default 5 s) for lines newer
  than the newest one it holds and prepends them. Off, a closed tab, or a
  hidden tab (`visibilitychange`) stops the polling.
- The page keeps at most `logs.max_rows` lines; older ones drop off the
  bottom. A **Pause** keeps the view still while polling continues, with a
  "N new lines" counter.
- Pod and node "logs" links get a **follow** variant: `/logs?...&follow=1`.
- A tail stops by itself after `logs.tail_max_minutes` (default 30) with a
  "resume" button, so a forgotten tab does not query forever.

### Backend

- `POST /api/logs` takes an optional `since` (RFC 3339 with nanoseconds).
  With `since`, the window is `since` to now (capped at 5 minutes back) and
  results come oldest first, so a burst larger than `limit` keeps its order
  and the next poll continues from the last line returned.
- LogsQL: `start=<since>` on `/select/logsql/query`. LogQL: `start=<since>`
  and `direction=forward` on `query_range`. Lines at exactly `since` come
  back again; the page drops the ones it already has (same time, labels and
  message).
- No new endpoint, no stream, no server state: the existing POST keeps its
  write checks (X-Homie header, same-site Origin), the datasource allowlist
  and the caps. Roles without the `logs` page get 403 as today.

### Configuration

| Key | Default | |
|---|---|---|
| `logs.tail_seconds` | `5` | poll interval while following, 2 to 60 |
| `logs.tail_max_minutes` | `30` | a tail stops after this; `0` hides the Follow button |

### Logs and metrics

| | |
|---|---|
| `msg="logs" tail=true` | the first poll of a tail, at info |
| `msg="logs" tail=true` (later polls) | at debug only, so a tail does not add a line every poll |

## Changes during implementation

- Results stay newest first, also with `since`, and no `direction=forward`:
  VictoriaLogs' `limit` and Loki's backward direction both keep the newest
  lines, so a burst larger than `logs.max_rows` keeps those, and the page
  says that some lines were skipped.
- The first search of a tail is an ordinary search and logs as one; only
  the polls with `since` carry `tail=true`, at debug.
- No separate follow links on pods and nodes: `follow=1` in the URL starts
  following, and the Follow button is next to Search.
- A hidden tab skips its polls instead of ending the tail; the
  `tail_max_minutes` timer keeps running.
- `/api/meta` sends `log_tail: {seconds, max_minutes, max_rows}`.

## Alternatives

| Option | Why not |
|---|---|
| VictoriaLogs `/select/logsql/tail` streamed over SSE | A long-lived request through the Grafana datasource proxy hits its timeout (`dataproxy.timeout`, 30 s by default), and it is VictoriaLogs only |
| Loki `/loki/api/v1/tail` | A WebSocket; the Grafana datasource proxy does not carry it |
| Server-side tail pushed through `/api/stream` | Server state per tab for a query only one user sees; the shared cache that makes the stream worth it does not apply |
| Query the stores directly | RFC 0001 and 0008 chose Grafana as the only gateway |

## Testing and rollout

- **Tests:** `since` parsing and its 5-minute cap; oldest-first order with
  `since`; LogsQL and LogQL parameters with `since`; dedupe of the boundary
  lines in the page.
- **Rollout:** on wherever `logs.datasources` is set; the Follow button is
  new, search behaves as before.
- **Off switch:** `logs.tail_max_minutes = 0`.

## Decisions

1. Default interval 5 s, minimum 2 s: homelab log stores are small, and a
   few seconds of delay is fine for watching a rollout.

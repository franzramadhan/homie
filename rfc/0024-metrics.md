# 0024: Metrics and JSON logs

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-25 | #104 | |

> An opt-in Prometheus `/metrics` endpoint on its own listener: card status,
> query health, certificate expiry, pending update counts and Homie's own
> activity. And an opt-in JSON log format for log pipelines.

## Problem

Homie derives states that nothing else exports: a card's tone (RFC 0019), a
certificate's expiry read from the handshake (RFC 0018), the number of
charts and images behind (RFCs 0020, 0021). Notifications cover "tell me
when it changes"; labs that already run Prometheus or VictoriaMetrics also
want "graph it, alert on it with my own rules, put it on my dashboard".

Homie itself is a black box to them too: `/healthz` answers `ok` as long as
the process runs, even when every backend fails. Whether a query is slow,
stale or failing is only in the logs, at `warn`.

Logs are plain text (`tracing` fmt, `msg=... key=value`). VictoriaLogs,
Loki or Elasticsearch can parse them with a pipeline step, but every field
is then a string guessed from text.

## Proposal

- **Off by default.** `metrics.listen` turns on a second HTTP listener that
  serves only `GET /metrics` (Prometheus text format 0.0.4). Nothing changes
  on the main listener.
- **Its own listener**, because the main one requires a session and checks
  `Host`, and a scraper has neither. An optional bearer token
  (`metrics.token`, a secret reference like any other) is checked when set.
  The guidance in the docs: bind to loopback on a laptop; in a cluster, a
  separate Service port reachable only from the scraper (NetworkPolicy).
- **No new dependency.** The text format is written by hand: counters,
  gauges and one histogram (an array of bucket counters per query, bumped
  on each backend call).
- **Latency percentiles.** Backend latency is a histogram with fixed
  buckets 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5 and 10 s, so
  `histogram_quantile(0.95, ...)` works per query. An average hides a node
  that times out on one call in twenty; a p95 shows it. About 10 series per
  query (8 buckets, `+Inf`, `_sum`, `_count`), around 200 for a 20-query lab.
- **Card status is evaluated at scrape time** through the shared cache, like
  the notify evaluator (RFC 0019): a scrape costs at most one backend call
  per card per TTL, and nothing if the page or the evaluator already warmed
  the cache. Kubernetes cards under `forward_token` have no user at scrape
  time: they use `metrics.kubernetes_service_account` (impersonated, as in
  notify) or are left out with a log line, like notify does today.
- **Low cardinality, no secrets in labels.** Labels are config names
  (`card`, `name`), query kinds, action kinds and certificate targets.
  Never user emails, guest names, IPs or version strings: versions can tell
  an attacker what is vulnerable (the reason they are blurred in the demo
  recording). Counts of what is behind, not what it is.

### Metrics

| Metric | Type | Labels | What it tells you |
|---|---|---|---|
| `homie_build_info` | gauge (1) | `version` | Homie's own version (public anyway) |
| `homie_card_status` | gauge (0/1) | `card`, `name`, `status` (`ok`, `warn`, `bad`, `quiet`) | One series per status, the current one is 1. The same tone the page shows |
| `homie_query_total` | counter | `kind`, `name`, `result` (`ok`, `stale`, `error`, `quiet`) | Backend calls and how they ended (cache hits not counted) |
| `homie_query_duration_seconds` | histogram | `kind`, `name` | Backend latency per query: percentiles from `_bucket`, average from `_sum` / `_count` |
| `homie_query_last_success_timestamp_seconds` | gauge | `kind`, `name` | Age of the data the page shows; alert on `time() - x > ...` |
| `homie_cert_expiry_timestamp_seconds` | gauge | `target` | Leaf `notAfter` of each probed endpoint (RFC 0018), a blackbox exporter for free |
| `homie_cert_probe_success` | gauge (0/1) | `target` | The handshake worked and the chain verified |
| `homie_updates_available` | gauge | `kind` (`packages`, `charts`, `images`, `distribution`, `flux`) | How many items have a newer release; no names, no versions |
| `homie_node_reboot_required` | gauge (0/1) | `cluster`, `node` | Proxmox node runs an older kernel than installed (RFC 0020) |
| `homie_actions_total` | counter | `action` (`power`, `reconcile`, `refresh`, `console`), `result` | Writes done through Homie |
| `homie_logins_total` | counter | `method` (`oidc`, `local`, `demo`), `result` (`ok`, `denied`) | Login attempts; a spike of `denied` is someone guessing |
| `homie_sessions_active` | gauge | | Server-side sessions |
| `homie_stream_clients` | gauge | | Open live streams (RFC 0009) |
| `homie_consoles_active` | gauge | | Open embedded consoles (RFC 0023) |
| `homie_notify_sent_total` | counter | `target`, `result` | Notifications delivered or failed (RFC 0019) |

Certificate targets are hostnames the user configured or Homie found in
Ingress and Gateway objects. They are as sensitive as the scrape endpoint
is reachable, which is the reason for the separate listener and the token.

Not exported: guest, node or pod resource metrics (Proxmox, kubelet and
node exporters already do that better), log lines, alert contents.

### JSON logs

- `log_format = "json"` switches `tracing` to one JSON object per line with
  the same fields as today (`msg`, `source`, `ms`, `result`, `user`, ...)
  plus `timestamp` and `level`. Default `text`, unchanged, because it reads
  better in a terminal. The Helm chart and `deploy/kubernetes` set `json`,
  since there logs always go into a pipeline. No switching on whether stdout
  is a terminal: that would change the format under existing deployments
  and break pipelines that parse today's text.
- Turns on the `json` feature of `tracing-subscriber` (already a
  dependency; the feature adds `tracing-serde`).
- The events Homie logs today get a table in the docs, so a pipeline knows
  what to expect: `listening`, `config`, `login`, `session ended`, `denied`,
  `action`, `console`, `logs`, `query`, `upstream`, `cert probe`, `notify`,
  `insecure`, `snapshot ...`.

### Configuration

| Env | Default | |
|---|---|---|
| `HOMIE__METRICS__LISTEN` | unset (off) | Address of the metrics listener, e.g. `127.0.0.1:9477` |
| `HOMIE__METRICS__TOKEN` | unset | Secret reference; when set, scrapes need `Authorization: Bearer <token>` |
| `HOMIE__METRICS__CARDS` | `true` | Evaluate card status at scrape time; `false` keeps `/metrics` free of backend calls (only counters of what already happened) |
| `HOMIE__METRICS__KUBERNETES_SERVICE_ACCOUNT` | unset | Identity for Kubernetes cards under `forward_token` |
| `HOMIE__LOG_FORMAT` | `text` | `json` for one JSON object per line |

Helm chart: `metrics.enabled` adds the port to the Service and, with
`metrics.serviceMonitor`, a ServiceMonitor (or VMServiceScrape).

### Logs and metrics

| | |
|---|---|
| `msg=listening` | gains `metrics=<addr>` when the listener is on |
| `msg=metrics, result=denied` | a scrape with a missing or wrong token (`warn`) |

### Log events

Every line has `msg`; most have `result`. With `log_format = "json"` the
fields sit at the top level next to `timestamp` and `level`.

| `msg` | Level | Fields |
|---|---|---|
| `config` | info | `file` |
| `listening` | info | `addr`, `metrics`, `login`, `version` |
| `insecure` | warn | `integration`, `reason` (TLS verification off, plain HTTP, no login) |
| `login` | info, warn | `method`, `user`, `role`, `result` (`ok`, `denied`) |
| `local login locked` | warn | `failures`, `minutes` |
| `local login password generated` | warn | `file`, `note` |
| `session ended` | info | `user`, `reason` |
| `denied` | info | `role`, `route`: a role asked for a route it may not use |
| `action` | info, warn | `action`, `user`, `target`, `detail` or `error`, `result` |
| `console` | info, warn | `user`, `pve`, `guest`, `mode`, `result` (`open`, `closed`, `denied`, `error`), `reason`, `seconds`, `bytes` |
| `query` | debug, warn | `source`, `ms`, `status` (`ok`, `stale`, `error`, `quiet`), `error` |
| `logs` | debug (live tail), info, warn | `user`, `source`, `minutes`, `query`, `ms`, `result`, `error` |
| `upstream` | info, warn | `source`, `versions`, `ms`, `result`, `error`: release lookups for the updates page |
| `cert probe` | debug, warn | `target`, `result` (`ok`, `unreachable`), `error` |
| `watch` | debug | Kubernetes watch errors behind live updates |
| `notify`, `notify skipped`, `notify disabled`, `notify state not saved` | info, warn | `card`, `name`, `target`, `from`, `to`, `result` |
| `metrics` | warn | `result = "denied"`: a scrape with a missing or wrong token |
| `snapshot write failed`, `snapshot unreadable, starting empty` | warn | `error` |

### Built as

- `src/metrics.rs`: families in a map behind one lock, the histogram next
  to it, gauges read per scrape. `Cache::get` records every backend call
  (only the `kind` and `name` parts of the cache key become labels, never
  the user part).
- `homie_cert_expiry_timestamp_seconds` is the earliest expiry that counts
  for the row (leaf, intermediates, a private root), the same date the
  Certificates card shows, not only the leaf's.
- `homie_cert_probe_success` is 0 when the last handshake failed or no
  chain was ever read; a chain that does not verify still counts as a
  successful probe and shows in `homie_card_status` instead.
- `homie_actions_total{action}` also counts `github.watch` (the workflow
  picker saving its list).
- Logging starts after the config is read, since `log_format` is in it;
  config errors go to stderr as before.
- The card list for scrapes is built once at the first scrape, so a
  `forward_token` cluster without `metrics.kubernetes_service_account` logs
  `notify skipped` once, not on every scrape.
- Helm: `logFormat` (default `json`), `metrics.enabled`, `metrics.port`
  (9477), `metrics.serviceMonitor.{enabled, kind, interval, bearerTokenSecret}`.
  Example rules: `examples/metrics-alerts.yaml`.

## Alternatives

| Option | Why not |
|---|---|
| `/metrics` on the main listener behind the session | Scrapers have no session; an exception in the guard for one path is the kind of special case that leaks |
| `/metrics` on the main listener, unauthenticated | Exposes certificate targets and lab shape to anyone who can reach the UI, including through a public demo |
| The `prometheus` or `metrics` crates | A dependency for writing about a hundred lines of text, histogram included |
| An average only (`_sum` / `_count`) | Hides tail latency: 5 calls in 100 at 10 s average out to about 600 ms |
| OpenTelemetry (OTLP push) | Needs a collector, which few homelabs run; Prometheus scraping is what Grafana, VictoriaMetrics and Prometheus all read. Can come later |
| Versions as labels (`homie_chart_info{chart, version, latest}`) | Tells a reader exactly what runs and what is unpatched; counts answer "am I behind" without that |
| Per-guest and per-pod metrics | Duplicates the exporters every lab already has, with worse data |

## Testing and rollout

- **Tests:** the exposition format (names, label escaping, one series per
  status, cumulative buckets with `+Inf` equal to `_count`), the token check, `metrics.cards = false` making no backend call,
  no email, IP or version string in the output with a fixture lab, the
  listener absent when `metrics.listen` is unset. `config::tests::examples_load`
  covers a metrics example.
- **Rollout:** a minor release; no change for anyone who does not set
  `metrics.listen` or `log_format`. Reference profile: a VMServiceScrape in
  the lab and a few alert rules (card `bad` for 10 minutes, a certificate
  under 14 days, no successful query for 15 minutes, p95 query latency over
  5 s) in `examples/`. Existing in-cluster users get JSON logs after
  upgrading the chart; the release notes say so.
- **Off switch:** unset `metrics.listen`; `log_format = "text"`.

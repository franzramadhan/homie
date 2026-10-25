# 0024: Metrics and JSON logs

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-25 | | |

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
- **No new dependency.** The text format is written by hand (counters,
  gauges, `_sum`/`_count` pairs). No histograms: average latency per query
  is enough for a homelab, and buckets can come later.
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
| `homie_query_duration_seconds_sum` / `_count` | counter | `kind`, `name` | Average backend latency per query |
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
  plus `timestamp` and `level`. Default `text`, unchanged.
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

## Alternatives

| Option | Why not |
|---|---|
| `/metrics` on the main listener behind the session | Scrapers have no session; an exception in the guard for one path is the kind of special case that leaks |
| `/metrics` on the main listener, unauthenticated | Exposes certificate targets and lab shape to anyone who can reach the UI, including through a public demo |
| The `prometheus` or `metrics` crates | A dependency for writing a few dozen lines of text; histograms are the only thing they would add |
| OpenTelemetry (OTLP push) | Needs a collector, which few homelabs run; Prometheus scraping is what Grafana, VictoriaMetrics and Prometheus all read. Can come later |
| Versions as labels (`homie_chart_info{chart, version, latest}`) | Tells a reader exactly what runs and what is unpatched; counts answer "am I behind" without that |
| Per-guest and per-pod metrics | Duplicates the exporters every lab already has, with worse data |

## Testing and rollout

- **Tests:** the exposition format (names, label escaping, one series per
  status), the token check, `metrics.cards = false` making no backend call,
  no email, IP or version string in the output with a fixture lab, the
  listener absent when `metrics.listen` is unset. `config::tests::examples_load`
  covers a metrics example.
- **Rollout:** a minor release; no change for anyone who does not set
  `metrics.listen` or `log_format`. Reference profile: a VMServiceScrape in
  the lab and a few alert rules (card `bad` for 10 minutes, a certificate
  under 14 days, no successful query for 15 minutes) in `examples/`.
- **Off switch:** unset `metrics.listen`; `log_format = "text"`.

# 0019: Notifications

| Status | Date | PR | Release |
|---|---|---|---|
| implemented | 2026-10-24 | #80 | v0.9.0 |

> Send a message when an overview card changes status (to ntfy, a
> Slack-compatible webhook or a generic JSON webhook), so a problem is seen
> without the page open.

## Problem

Homie helps only while someone looks at it. Grafana alerting covers
metrics, healthchecks.io covers cron jobs, but the states Homie itself
derives have no alert: OpenBao sealed, a CloudNativePG backup gone stale, a
Proxmox node offline, a Flux Kustomization not Ready, a certificate about to
expire (RFC 0018). Today each needs its own exporter and alert rule, or is
missed.

## Proposal

- **One source of status.** The tone rules of the overview cards move from
  `web/src/routes/overview.tsx` to Rust: each overview query's envelope
  gains `tone` (`ok`, `warn`, `bad`, `quiet`) and `reasons` (short strings,
  "2 guests stopped", "backup 31 h old"). The UI renders them. The page and
  the notifier cannot disagree.
- **Evaluator.** A background task runs the overview queries through the
  cache every `interval` (default 60 s), with a background caller (no
  user, full view). It shares the cache with the UI, so a tab that is open
  costs nothing extra.
- **Transitions only.** A message is sent when a card's tone gets worse
  than `min_tone` or better again (recovery), after the new tone has held
  for `for` (default 3 min). No reminders by default.
- **Threshold per card.** `min_tone` is `bad` for most cards, and `warn`
  where amber is the state to act on: `certificates` (RFC 0018, days left
  to renew) and `cnpg` (backup stale, WAL archiving off). By red these are
  already broken. `notify.min_tone_per_card` overrides any card.
- **Quiet hours.** Cards in quiet hours never notify. The first round after
  the window ends sets the baseline silently, so the morning start of the
  lab is not a burst of "recovered".
- **Restarts.** With `state_dir`, the last notified tone per card is saved
  in `notify.json` next to the snapshot, so a restart neither repeats nor
  loses a notification. Without it, the first round is a silent baseline.
- **Kubernetes without a token.** A `forward_token` cluster has no
  credentials in the background. `notify.kubernetes_service_account`
  names a read-only ServiceAccount that Homie impersonates, as roles do
  (RFC 0016). Without it, that cluster's Kubernetes, Flux and Argo CD cards
  are not evaluated, and `homie check` says so.
- **Targets**, any number:
  - `ntfy`: POST to the topic `url` (ntfy.sh or self-hosted), title and
    tags from the tone, priority 4 for `bad`, 3 for `warn`, 2 for recovery.
  - `slack`: `{"text": ...}` for Slack, Mattermost, Rocket.Chat and
    Discord (its `/slack` endpoint).
  - `webhook`: JSON `{card, from, to, reasons, url, at}`, for Apprise API,
    n8n, Home Assistant or anything else.
- **Message.** `Homie: OpenBao lab sealed (was ok)`, the reasons, and a
  link to the page under `public_url`. With `redact = true` only the card
  and tone are sent (names of guests and namespaces stay home).
- `homie notify-test` sends a test message to every target.
- The Grafana alerts card is left out by default: Grafana notifies for its
  own rules.

### Configuration

| Key | Default | |
|---|---|---|
| `notify.interval` | `"60s"` | evaluation period |
| `notify.for` | `"3m"` | how long a new tone holds before a message |
| `notify.min_tone` | `"bad"` | `warn` to also send amber, for cards without their own default |
| `notify.min_tone_per_card` | `{ certificates = "warn", cnpg = "warn" }` | per-card threshold; set a card to `"bad"` to undo its default |
| `notify.recovery` | `true` | send when a card is ok again |
| `notify.cards` | all except Grafana alerts | card ids to evaluate |
| `notify.redact` | `false` | card and tone only |
| `notify.kubernetes_service_account` | none | `namespace/name` impersonated for `forward_token` clusters |
| `notify.targets[].type` | - | `ntfy`, `slack` or `webhook` |
| `notify.targets[].url` | - | secret reference allowed |
| `notify.targets[].token` | none | bearer token (ntfy access token), secret reference |
| `notify.targets[].headers` | none | extra headers for `webhook`, values may be secret references |

No `[notify]` section, no evaluator.

### Logs and metrics

| | |
|---|---|
| `msg="notify" card target from to result` | every message sent or failed |
| `msg="notify skipped" card reason` | quiet hours, baseline, missing credentials |

## Changes during implementation

- Durations are numbers, like the other keys: `interval_seconds` (60) and
  `hold_seconds` (180) instead of `interval` and `for`.
- Card ids are the query kinds (`pve`, `k8s`, `flux`, `argocd`, `alerts`,
  `security`, `openbao`, `cnpg`, `tailscale`, `github`, `healthchecks`),
  and `certificates` for the RFC 0018 card.
- A target can take a `ca_file` (a self-hosted ntfy behind a private CA).
- `homie notify-test` sends one message to every target and fails when one
  does not answer 2xx.
- The GitHub card is not evaluated in session mode (RFC 0017): there are no
  workflows without a visitor.
- Kubernetes cards of a `forward_token` cluster without
  `kubernetes_service_account` are left out with a `msg="notify skipped"`
  line at start, instead of in `homie check`.

## Alternatives

| Option | Why not |
|---|---|
| `/metrics` with `homie_card_status` and Grafana alert rules | Good for labs with Prometheus; others get nothing, and quiet hours would be configured twice. Can come later on top of the same tones |
| Keep tone rules in the UI, duplicate them in Rust | Two rule sets drift |
| Email (SMTP) | One more credential and dependency; ntfy, Slack and Apprise reach email already |
| Native Telegram, Pushover, Gotify, ... | Each is a client to maintain; Apprise API behind `webhook` covers them |
| Notify on every round while bad | Noise; a reminder interval can be added if asked for |
| A daily digest of amber cards | Per-change messages and per-card thresholds cover it; add if amber cards turn out noisy in practice |
| One global `min_tone` only | `bad` misses the actionable stage of certificates and backups; `warn` everywhere is noisy |

## Testing and rollout

- **Tests:** the transition state machine (hold time, recovery, quiet
  window, silent baseline, restart from `notify.json`); payloads per target
  type; redaction; per-card thresholds and overrides; tone rules per card against today's UI rules (fixtures
  from the existing UI tests).
- **Rollout:** two PRs: first the tone move (refactor, no visible change),
  then the evaluator and targets. Off unless `[notify]` is set. Network
  policies need egress to the targets.
- **Off switch:** remove `[notify]`.

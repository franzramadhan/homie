# 0029: Table export

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-11-03 | #145 | - |

> Every table gets an Export button. It downloads the rows that you
> see as CSV or JSON. It runs in the browser only: no new endpoint, no
> config.

## Problem

Homie shows lists that people want in other tools. Examples: the
certificates that expire this month (for a ticket), the image versions
of all workloads (for an upgrade plan), the action history (for an
audit). Today the only ways out are a screenshot, a manual
copy of the table text, or a call to `/api/query/<name>` with a session
cookie. The query API returns the whole result, not the filtered and
sorted view the user built on the page.

## Proposal

- Every `DataTable` gets an Export button next to the filter, with two
  entries: CSV and JSON. If the table has no rows, the button is hidden.
  All 37 tables on 11 pages get it from the one shared component:
  Infrastructure (guests, storages, certificates), Workloads, Helm
  releases, GitOps (Flux, Argo CD), Updates, History, Security, Logs,
  Tailscale, Workflows, Observability.
- The export holds the rows that the table shows now: after the text
  filter, in the current sort order. An empty filter exports every row.
- CSV: one column per table column that has a value (an accessor).
  Display-only columns, such as action buttons, are left out. Headers
  are the column titles. Values are the raw cell values, not the
  rendered text: timestamps stay ISO 8601 and byte counts stay numbers,
  so a spreadsheet can sort them.
- JSON: an array of the original row objects, the shape that
  `web/src/api.ts` describes. It is the same data that the page already
  received, so it adds no new exposure.
- File name: `homie-<table>-<UTC timestamp>.csv` (or `.json`). Each
  `DataTable` call gets a short `name` prop (`certificates`,
  `workloads`, `helm-releases`). Without one, the name is `table`.
- CSV injection: a cell that starts with `=`, `+`, `-`, `@`, tab or
  carriage return gets a leading `'`, so a spreadsheet does not run it
  as a formula. Kubernetes labels and log lines are user-controlled
  text, so this is necessary.
- Roles: no change. A viewer or a demo session can export only what
  the page already shows to it. Values that the backend redacts stay
  redacted.

### Configuration

None. The feature runs in the browser and uses data the page already
has.

### Logs and metrics

None. The server does not see an export.

## Alternatives

| Option | Why not |
|---|---|
| Server endpoint `/api/query/<name>?format=csv` | Needs a CSV shape per query in Rust, and it ignores the filter and sort on the page. Scripts can already read the JSON query API. |
| XLSX | Needs a new dependency. CSV opens in every spreadsheet. |
| Copy to clipboard | If people ask for it, it is a small addition later. A file covers tickets and audits. |
| Export on chosen pages only | The shared component makes every table cost the same. Choosing pages adds a decision per table and no saving. |

## Open questions

- Logs page: export only the rows on screen (the current result
  limit), or also a larger fetch? The proposal exports only what is on
  screen.
- Map page: export of its guests? The proposal says no, because the
  Infrastructure page has the same rows in a table.

## Testing and rollout

- **Tests:** a unit test for the CSV writer (quoting of commas, quotes
  and new lines, the formula guard, columns without an accessor left
  out). The web build covers the types. A manual check that a filtered
  and sorted certificates table exports the same rows in the same
  order.
- **Rollout:** the next minor version. No upgrade steps.
- **Off switch:** none needed. Roll back to the earlier image.

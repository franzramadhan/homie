# 0029: Table export

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-11-03 | #145 | - |

> Every table gets an Export button. It downloads the rows that you
> see as CSV or JSON. The Logs page exports the whole time range of
> the search through a new endpoint. The other tables export in the
> browser.

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

### Logs page

A search shows at most `logs.max_rows` lines (default 1000). The Logs
export covers the whole time range of the search instead, not only the
lines on screen:

- The Export button on the Logs page calls a new endpoint,
  `POST /api/logs/export`, with the same source, query and range as the
  search. The guard rules are the same as for `/api/logs`: a session
  with the Logs page, and the CSRF header.
- The server asks the datasource in pages of `logs.max_rows`, newest
  first. Each next page ends at the oldest timestamp of the page
  before. Lines on that boundary come back twice, so the server drops
  the repeats. The response streams to the browser as a file, so no
  page is held in memory longer than needed.
- A cap stops very large exports: `logs.max_export_rows`, default
  `50000`. When the export hits the cap, the last line of the file
  says so, and the range is the thing to narrow. `0` turns the logs
  export off (the button is hidden). The table on screen can still be
  exported as it is.
- CSV columns: time, then the labels the search table shows, then the
  line. JSON is NDJSON (one object per line), so a large file can be
  read line by line.
- One export at a time per session. A second request answers `429`
  while the first one runs.

### Configuration

| Key | Default | |
|---|---|---|
| `logs.max_export_rows` | `50000` | lines per logs export. `0` turns the logs export off |

The other tables need no configuration: the export runs in the
browser and uses data the page already has.

### Logs and metrics

| | |
|---|---|
| `msg="logs export"` | user, source, range, lines written, whether the cap was hit |
| `homie_logs_exports_total{result}` | `ok`, `capped`, `error` |

Exports of the other tables run in the browser. The server does not
see them.

## Alternatives

| Option | Why not |
|---|---|
| Server endpoint `/api/query/<name>?format=csv` | Needs a CSV shape per query in Rust, and it ignores the filter and sort on the page. Scripts can already read the JSON query API. |
| XLSX | Needs a new dependency. CSV opens in every spreadsheet. |
| Copy to clipboard | If people ask for it, it is a small addition later. A file covers tickets and audits. |
| Export on chosen pages only | The shared component makes every table cost the same. Choosing pages adds a decision per table and no saving. |

## Decisions

- Logs page: the export covers the whole time range of the search, up
  to `logs.max_export_rows` (user, 2026-11-03).
- Map page: no export. The Infrastructure page has the same guests in
  a table (user, 2026-11-03).

## Testing and rollout

- **Tests:** a unit test for the CSV writer (quoting of commas, quotes
  and new lines, the formula guard, columns without an accessor left
  out). Server tests for `/api/logs/export`: paging across a
  timestamp boundary without repeats, the cap and its last line, the
  `429` for a second export, `403` without the Logs page. The web
  build covers the types. A manual check that a filtered
  and sorted certificates table exports the same rows in the same
  order.
- **Rollout:** the next minor version. No upgrade steps. The logs
  export is on by default with the cap above.
- **Off switch:** `logs.max_export_rows = 0` for the logs export.
  Table export has no switch: roll back to the earlier image.

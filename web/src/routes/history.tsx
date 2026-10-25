// Who did what and when (RFC 0027): actions, consoles, logins and logouts,
// newest first. A role sees its own entries and those on targets in its
// view, everyone else as "another user".

import { useMeta, useQ, type HistoryRow } from "../api";
import { Badge, DataTable, Source } from "../ui";

const exportLink = "rounded-control border border-hairline-strong px-2 py-0.5 text-xs text-ink-muted hover:text-ink";

export function History() {
  const m = useMeta().data;
  const q = useQ<HistoryRow[]>("history", "_");
  if (!m?.history) return null;
  return (
    <Source title="History" query={q} right={
        <>
          {!m.history.kept && <Badge tone="muted">not kept across restarts: no state_dir</Badge>}
          <a className={exportLink} href="/api/history?format=json" download>
            Export JSON
          </a>
          <a className={exportLink} href="/api/history?format=csv" download>
            Export CSV
          </a>
        </>
      }
    >
      {(rows) => (
        <DataTable
          data={rows}
          empty="Nothing done yet"
          columns={[
            { accessorKey: "time", header: "Time", cell: ({ row }) => <span className="text-xs whitespace-nowrap">{new Date(row.original.time).toLocaleString()}</span> },
            { id: "user", accessorFn: (r) => r.user ?? "unknown", header: "User" },
            { accessorKey: "action", header: "Action", cell: ({ row }) => <code className="text-xs">{row.original.action}</code> },
            { accessorKey: "target", header: "Target", cell: ({ row }) => <code className="text-xs">{row.original.target}</code> },
            {
              accessorKey: "result",
              header: "Result",
              cell: ({ row }) => (
                <span className="flex flex-wrap items-center gap-1">
                  <Badge tone={row.original.result === "ok" ? "ok" : "bad"}>{row.original.result}</Badge>
                  {row.original.error && <span className="text-xs whitespace-normal text-ink-subtle">{row.original.error}</span>}
                </span>
              ),
            },
          ]}
        />
      )}
    </Source>
  );
}

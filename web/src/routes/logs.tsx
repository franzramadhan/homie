// Log search over the configured log datasources (RFC 0008). The URL keeps
// source, query and range, so pod and node links can open a prefilled search.

import { useEffect, useRef, useState, type FormEvent } from "react";
import { act, useMeta, type LogRow } from "../api";
import { DataTable, Section } from "../ui";

const RANGES: [number, string][] = [
  [15, "15 minutes"],
  [60, "1 hour"],
  [360, "6 hours"],
  [1440, "24 hours"],
  [10080, "7 days"],
];

export function Logs() {
  const m = useMeta().data;
  const params = new URLSearchParams(window.location.search);
  const [source, setSource] = useState(params.get("source") ?? "");
  const [query, setQuery] = useState(params.get("q") ?? "");
  const [minutes, setMinutes] = useState(Number(params.get("range") ?? 60));
  const [rows, setRows] = useState<LogRow[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const autorun = useRef(Boolean(params.get("q")));
  // a prefilled link (from a pod or node) runs its search once the sources are known
  useEffect(() => {
    if (m && autorun.current) {
      autorun.current = false;
      void run();
    }
  });
  if (!m) return null;
  const sources = m.logs;
  if (!sources.length) return <Section title="Logs">No log datasources are configured (`[logs] datasources`).</Section>;
  const current = source || sources[0].label;
  const kind = sources.find((s) => s.label === current)?.type;

  async function run(e?: FormEvent) {
    e?.preventDefault();
    if (!query.trim()) return;
    setBusy(true);
    setError("");
    window.history.replaceState(null, "", `/logs?${new URLSearchParams({ source: current, q: query, range: String(minutes) })}`);
    try {
      const r = await act<{ rows: LogRow[]; truncated: boolean }>("/api/logs", { source: current, query, minutes });
      setRows(r.rows);
      setTruncated(r.truncated);
    } catch (err) {
      setError((err as Error).message);
      setRows(null);
    } finally {
      setBusy(false);
    }
  }

  const control = "rounded-control border border-hairline-strong bg-surface-2 px-2 py-1 text-sm";
  return (
    <Section title="Logs">
      <form onSubmit={run} className="mb-3 flex flex-wrap items-center gap-2">
        <select aria-label="Source" className={control} value={current} onChange={(e) => setSource(e.target.value)}>
          {sources.map((s) => (
            <option key={s.label} value={s.label}>
              {s.label} ({s.type === "loki" ? "LogQL" : "LogsQL"})
            </option>
          ))}
        </select>
        <input
          aria-label="Query"
          className={`${control} min-w-80 flex-1 font-mono placeholder:text-ink-faint`}
          placeholder={kind === "loki" ? '{namespace="default"} |= "error"' : "k8s.namespace.name:default error"}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <select aria-label="Time range" className={control} value={minutes} onChange={(e) => setMinutes(Number(e.target.value))}>
          {RANGES.map(([v, l]) => (
            <option key={v} value={v}>
              last {l}
            </option>
          ))}
        </select>
        <button type="submit" disabled={busy || !query.trim()} className="rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-40">
          {busy ? "Searching" : "Search"}
        </button>
      </form>
      {error && <p className="mb-2 text-sm text-bad">{error}</p>}
      {rows && (
        <>
          <p className="mb-2 text-xs text-ink-subtle">
            {rows.length} lines, newest first{truncated ? " (more matched: narrow the query or the range)" : ""}
          </p>
          <DataTable
            data={rows}
            empty="No lines matched"
            columns={[
              { accessorKey: "time", header: "Time", cell: ({ row }) => <span className="text-xs">{new Date(row.original.time).toLocaleString()}</span> },
              { accessorKey: "message", header: "Message", cell: ({ row }) => <span className="block min-w-96 font-mono text-xs break-all whitespace-pre-wrap">{row.original.message}</span> },
              { accessorKey: "labels", header: "Stream", cell: ({ row }) => <span className="block max-w-sm text-xs break-words whitespace-normal text-ink-subtle">{row.original.labels}</span> },
            ]}
          />
        </>
      )}
    </Section>
  );
}

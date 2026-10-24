// Log search over the configured log datasources (RFC 0008). The URL keeps
// source, query and range, so pod and node links can open a prefilled search.
// Follow (RFC 0022) polls for lines newer than the newest one shown.

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

type Search = { source: string; query: string; minutes: number };
type Result = { rows: LogRow[]; truncated: boolean };

const key = (r: LogRow) => `${r.time}|${r.labels}|${r.message}`;

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
  const [follow, setFollow] = useState(false);
  const [paused, setPaused] = useState(false);
  const [pending, setPending] = useState<LogRow[]>([]);
  const [skipped, setSkipped] = useState(false);
  const [stopped, setStopped] = useState(false);
  const autorun = useRef(Boolean(params.get("q")));
  const autofollow = useRef(params.get("follow") === "1");
  // the search a tail continues, and the newest line it holds
  const tail = useRef<Search | null>(null);
  const newest = useRef<string | null>(null);
  const seen = useRef(new Set<string>());
  const pausedRef = useRef(paused);
  pausedRef.current = paused;

  // a prefilled link (from a pod or node) runs its search once the sources are known
  useEffect(() => {
    if (m && autorun.current) {
      autorun.current = false;
      void run(undefined, autofollow.current);
    }
  });

  const tailCfg = m?.log_tail;
  useEffect(() => {
    if (!follow || !tailCfg) return;
    const max = tailCfg.max_rows;
    const poll = async () => {
      const s = tail.current;
      if (!s || document.hidden) return;
      try {
        const r = await act<Result>("/api/logs", { ...s, since: newest.current });
        // lines at exactly `since` come back again
        const fresh = r.rows.filter((row) => !seen.current.has(key(row)));
        if (r.truncated) setSkipped(true);
        if (!fresh.length) return;
        fresh.forEach((row) => seen.current.add(key(row)));
        newest.current = fresh[0].time;
        if (pausedRef.current) setPending((p) => [...fresh, ...p].slice(0, max));
        else setRows((old) => [...fresh, ...(old ?? [])].slice(0, max));
      } catch (err) {
        setError((err as Error).message);
      }
    };
    const timer = window.setInterval(() => void poll(), tailCfg.seconds * 1000);
    const limit = window.setTimeout(() => {
      setFollow(false);
      setStopped(true);
    }, tailCfg.max_minutes * 60_000);
    return () => {
      window.clearInterval(timer);
      window.clearTimeout(limit);
    };
  }, [follow, tailCfg]);

  if (!m) return null;
  const sources = m.logs;
  if (!sources.length) return <Section title="Logs">No log datasources are configured (`[logs] datasources`).</Section>;
  const current = source || sources[0].label;
  const kind = sources.find((s) => s.label === current)?.type;
  const canFollow = m.log_tail.max_minutes > 0;

  async function run(e?: FormEvent, following = follow) {
    e?.preventDefault();
    if (!query.trim()) return;
    setBusy(true);
    setError("");
    const s: Search = { source: current, query, minutes };
    const url = new URLSearchParams({ source: current, q: query, range: String(minutes) });
    if (following) url.set("follow", "1");
    window.history.replaceState(null, "", `/logs?${url}`);
    try {
      const r = await act<Result>("/api/logs", s);
      setRows(r.rows);
      setTruncated(r.truncated);
      tail.current = s;
      // with nothing matched yet, a tail starts from now
      newest.current = r.rows[0]?.time ?? new Date().toISOString();
      seen.current = new Set(r.rows.map(key));
      setPending([]);
      setSkipped(false);
      setStopped(false);
      setFollow(following);
    } catch (err) {
      setError((err as Error).message);
      setRows(null);
      setFollow(false);
    } finally {
      setBusy(false);
    }
  }

  function toggleFollow() {
    if (follow) {
      setFollow(false);
      setPaused(false);
      const url = new URLSearchParams(window.location.search);
      url.delete("follow");
      window.history.replaceState(null, "", `/logs?${url}`);
    } else void run(undefined, true);
  }

  function resume() {
    setRows((old) => [...pending, ...(old ?? [])].slice(0, m!.log_tail.max_rows));
    setPending([]);
    setPaused(false);
  }

  const control = "rounded-control border border-hairline-strong bg-surface-2 px-2 py-1 text-sm";
  const ghost = "rounded-control border border-hairline-strong px-3 py-1 text-sm hover:bg-surface-2 disabled:opacity-40";
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
        {canFollow && (
          <button type="button" aria-pressed={follow} disabled={busy || !query.trim()} onClick={toggleFollow} className={ghost}>
            {follow ? "Stop following" : "Follow"}
          </button>
        )}
        {follow && (
          <button type="button" aria-pressed={paused} onClick={() => (paused ? resume() : setPaused(true))} className={ghost}>
            {paused ? `Resume${pending.length ? ` (${pending.length} new)` : ""}` : "Pause"}
          </button>
        )}
      </form>
      {error && <p className="mb-2 text-sm text-bad">{error}</p>}
      {stopped && <p className="mb-2 text-sm text-ink-subtle">Follow stopped after {m.log_tail.max_minutes} minutes. Press Follow to go on.</p>}
      {rows && (
        <>
          <p className="mb-2 text-xs text-ink-subtle">
            {rows.length} lines, newest first
            {follow ? `, following every ${m.log_tail.seconds} s` : ""}
            {truncated ? " (more matched: narrow the query or the range)" : ""}
            {skipped ? " (lines arrived faster than one poll returns: some were skipped)" : ""}
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

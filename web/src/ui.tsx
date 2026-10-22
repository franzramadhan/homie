// Shared building blocks: status badges, data age, usage bars, the source
// card that renders one query envelope, a sortable filterable table, and a
// confirm dialog on the native <dialog> element.

import {
  columnFilteringFeature,
  createFilteredRowModel,
  createSortedRowModel,
  filterFn_includesString,
  globalFilteringFeature,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  tableFeatures,
  useTable,
  type ColumnDef,
  type RowData,
} from "@tanstack/react-table";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Envelope } from "./api";

export type Tone = "ok" | "warn" | "bad" | "quiet" | "muted";

const toneClass: Record<Tone, string> = {
  ok: "bg-ok/12 text-ok",
  warn: "bg-warn/12 text-warn",
  bad: "bg-bad/12 text-bad",
  quiet: "bg-quiet/12 text-quiet",
  muted: "bg-surface-3 text-ink-subtle",
};

export function Badge({ tone, children }: { tone: Tone; children: ReactNode }) {
  return <span className={`inline-flex items-center rounded-control px-1.5 py-0.5 text-xs font-medium whitespace-nowrap ${toneClass[tone]}`}>{children}</span>;
}

export function ago(iso: string | null | undefined): string {
  if (!iso) return "never";
  const s = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}

export function bytes(n: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(n >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}

export function duration(seconds: number): string {
  if (seconds <= 0) return "";
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  return d > 0 ? `${d}d ${h}h` : `${h}h ${Math.floor((seconds % 3600) / 60)}m`;
}

/** A usage bar: amber above 80%, rose above 90%. */
export function Bar({ value, max, label }: { value: number; max: number; label?: string }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  const color = pct > 90 ? "bg-bad" : pct > 80 ? "bg-warn" : "bg-ok";
  return (
    <div className="flex min-w-28 items-center gap-2" title={label}>
      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-3" role="meter" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
        <div className={`h-full ${color}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="w-9 text-right text-xs tabular-nums text-ink-subtle">{Math.round(pct)}%</span>
    </div>
  );
}

/**
 * Sparkline: the last hour as an SVG polyline. With `max` (ratios, max 1)
 * the line takes the meter colors; without it the series scales to its own
 * range in a neutral color.
 */
export function Spark({ values, max, label }: { values: number[] | undefined; max?: number; label: string }) {
  if (!values || values.length < 2) return <span className="inline-block w-16" />;
  const top = max ?? Math.max(...values);
  const bottom = max === undefined ? Math.min(...values) : 0;
  const span = top - bottom || 1;
  const w = 64;
  const h = 16;
  const points = values.map((v, i) => `${((i / (values.length - 1)) * w).toFixed(1)},${(h - 1 - ((v - bottom) / span) * (h - 2)).toFixed(1)}`).join(" ");
  const last = values[values.length - 1];
  const color = max === undefined ? "text-ink-subtle" : last / max > 0.9 ? "text-bad" : last / max > 0.8 ? "text-warn" : "text-ok";
  const fmt = (v: number) => (max === 1 ? `${Math.round(v * 100)}%` : v.toPrecision(3));
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  return (
    <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} className={`shrink-0 ${color}`} role="img" aria-label={`${label}, last hour: min ${fmt(lo)}, max ${fmt(hi)}, now ${fmt(last)}`}>
      <title>{`${label}, last hour: min ${fmt(lo)}, max ${fmt(hi)}, now ${fmt(last)}`}</title>
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.25" strokeLinejoin="round" />
    </svg>
  );
}

/** The state of one query: fresh, stale, scheduled off, or failing. */
export function sourceTone(env: Envelope<unknown> | undefined): { tone: Tone; label: string } {
  if (!env) return { tone: "muted", label: "loading" };
  if (env.error && env.quiet) return { tone: "quiet", label: "scheduled off" };
  if (env.error && env.stale) return { tone: "warn", label: `stale, ${ago(env.fetched_at)}` };
  if (env.error) return { tone: "bad", label: "unreachable" };
  return { tone: "ok", label: ago(env.fetched_at) };
}

export function Section({ title, right, children }: { title: ReactNode; right?: ReactNode; children: ReactNode }) {
  return (
    <section className="rounded-card border border-hairline bg-surface-1 p-4">
      <header className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-base font-semibold tracking-[-0.01em]">{title}</h2>
        <div className="flex flex-wrap items-center gap-2 text-xs">{right}</div>
      </header>
      {children}
    </section>
  );
}

/** Renders a query: its state in the header, the error when there is no data. */
export function Source<T>({
  title,
  query,
  right,
  children,
}: {
  title: ReactNode;
  query: { data?: Envelope<T>; error: Error | null; isLoading: boolean };
  right?: ReactNode;
  children: (data: T) => ReactNode;
}) {
  const env = query.data;
  const st = sourceTone(env);
  return (
    <Section
      title={title}
      right={
        <>
          {right}
          <Badge tone={st.tone}>{st.label}</Badge>
        </>
      }
    >
      {query.error && <p className="text-sm text-bad">{query.error.message}</p>}
      {env?.error && <p className={`mb-2 text-sm break-words ${env.quiet ? "text-ink-subtle" : "text-bad"}`}>{env.quiet ? "Inside quiet hours: " : ""}{env.error}</p>}
      {env?.data != null ? children(env.data) : !env?.error && query.isLoading && <p className="text-sm text-ink-subtle">Loading</p>}
    </Section>
  );
}

const features = tableFeatures({
  columnFilteringFeature,
  rowSortingFeature,
  globalFilteringFeature,
  sortedRowModel: createSortedRowModel(),
  filteredRowModel: createFilteredRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, basic: sortFn_basic },
  filterFns: { includesString: filterFn_includesString },
});

export type Col<T extends RowData> = ColumnDef<typeof features, T>;

/** A table with click-to-sort headers and an optional text filter. */
export function DataTable<T extends RowData>({ columns, data, filter = true, empty = "Nothing to show" }: { columns: Col<T>[]; data: T[]; filter?: boolean; empty?: string }) {
  const [globalFilter, setGlobalFilter] = useState("");
  const table = useTable(
    {
      features,
      columns,
      data,
      state: { globalFilter },
      onGlobalFilterChange: setGlobalFilter,
      globalFilterFn: "includesString",
    },
    (s) => ({ sorting: s.sorting, globalFilter: s.globalFilter }),
  );
  const rows = table.getRowModel().rows;
  return (
    <div>
      {filter && data.length > 8 && (
        <input
          className="mb-2 w-full max-w-xs rounded-control border border-hairline-strong bg-surface-2 px-2 py-1 text-sm placeholder:text-ink-faint"
          placeholder="Filter"
          aria-label="Filter rows"
          value={globalFilter}
          onChange={(e) => setGlobalFilter(e.target.value)}
        />
      )}
      <div className="overflow-x-auto">
        <table className="w-full text-left text-[13px] tabular-nums">
          <thead>
            {table.getHeaderGroups().map((hg) => (
              <tr key={hg.id} className="sticky top-0 border-b border-hairline bg-surface-1 text-xs text-ink-subtle">
                {hg.headers.map((h) => {
                  const sorted = h.column.getIsSorted();
                  return (
                    <th key={h.id} className="py-1.5 pr-3 font-medium whitespace-nowrap">
                      {h.column.getCanSort() ? (
                        <button className="hover:text-ink" onClick={h.column.getToggleSortingHandler()}>
                          <table.FlexRender header={h} />
                          {sorted === "asc" ? " ▲" : sorted === "desc" ? " ▼" : ""}
                        </button>
                      ) : (
                        <table.FlexRender header={h} />
                      )}
                    </th>
                  );
                })}
              </tr>
            ))}
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id} className="border-b border-hairline align-top last:border-0 hover:bg-surface-2">
                {r.getAllCells().map((c) => (
                  <td key={c.id} className="py-[7px] pr-3 whitespace-nowrap">
                    <table.FlexRender cell={c} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {rows.length === 0 && <p className="py-2 text-sm text-ink-subtle">{empty}</p>}
      </div>
    </div>
  );
}

/** External link. Only http(s) URLs become links, so a `javascript:` URL from
 * config or a backend can never run. */
export function Ext({ href, children }: { href: string | null | undefined; children: ReactNode }) {
  if (!href || !/^https?:\/\//i.test(href)) return <>{children}</>;
  return (
    <a className="underline decoration-ink-faint underline-offset-2 hover:decoration-ink" href={href} target="_blank" rel="noreferrer">
      {children}
    </a>
  );
}

export function Button({ children, onClick, disabled, tone = "default" }: { children: ReactNode; onClick: () => void; disabled?: boolean; tone?: "default" | "danger" }) {
  const c =
    tone === "danger"
      ? "border-bad/40 text-bad hover:bg-bad/12"
      : "border-hairline-strong text-ink-muted hover:bg-surface-2 hover:text-ink";
  return (
    <button className={`rounded-control border px-2 py-0.5 text-xs disabled:opacity-50 ${c}`} onClick={onClick} disabled={disabled}>
      {children}
    </button>
  );
}

/**
 * Confirm dialog. With `typeToConfirm`, the action stays disabled until the
 * exact text is typed (used for protected guests).
 */
export function Confirm({
  open,
  title,
  body,
  typeToConfirm,
  confirmLabel,
  onConfirm,
  onClose,
}: {
  open: boolean;
  title: string;
  body: ReactNode;
  typeToConfirm?: string;
  confirmLabel: string;
  onConfirm: (typed: string) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const [typed, setTyped] = useState("");
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      setTyped("");
      d.showModal();
    }
    if (!open && d.open) d.close();
  }, [open]);
  const ready = !typeToConfirm || typed === typeToConfirm;
  return (
    <dialog ref={ref} onClose={onClose} className="m-auto w-[min(28rem,90vw)] rounded-card border border-hairline bg-surface-1 p-5 text-ink backdrop:bg-black/50">
      <form
        method="dialog"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) onConfirm(typed);
        }}
      >
        <h3 className="mb-2 font-semibold">{title}</h3>
        <div className="mb-4 text-sm text-ink-subtle">{body}</div>
        {typeToConfirm && (
          <label className="mb-4 block text-sm">
            Type <code className="font-mono">{typeToConfirm}</code> to confirm
            <input autoFocus className="mt-1 w-full rounded-control border border-hairline-strong bg-surface-2 px-2 py-1 font-mono" value={typed} onChange={(e) => setTyped(e.target.value)} />
          </label>
        )}
        <div className="flex justify-end gap-2">
          <button type="button" className="rounded-control px-3 py-1 text-sm hover:bg-surface-2" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" disabled={!ready} className="rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-40">
            {confirmLabel}
          </button>
        </div>
      </form>
    </dialog>
  );
}

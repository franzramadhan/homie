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
  ok: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
  warn: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300",
  bad: "bg-rose-100 text-rose-800 dark:bg-rose-950 dark:text-rose-300",
  quiet: "bg-zinc-200 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400",
  muted: "bg-zinc-100 text-zinc-600 dark:bg-zinc-900 dark:text-zinc-400",
};

export function Badge({ tone, children }: { tone: Tone; children: ReactNode }) {
  return <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-xs font-medium whitespace-nowrap ${toneClass[tone]}`}>{children}</span>;
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
  const color = pct > 90 ? "bg-rose-500" : pct > 80 ? "bg-amber-500" : "bg-emerald-500";
  return (
    <div className="flex min-w-28 items-center gap-2" title={label}>
      <div className="h-1.5 flex-1 overflow-hidden rounded bg-zinc-200 dark:bg-zinc-800" role="meter" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
        <div className={`h-full ${color}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="w-9 text-right text-xs tabular-nums text-zinc-500">{Math.round(pct)}%</span>
    </div>
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
    <section className="rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-950">
      <header className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">{title}</h2>
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
      {query.error && <p className="text-sm text-rose-600">{query.error.message}</p>}
      {env?.error && <p className={`mb-2 text-sm break-words ${env.quiet ? "text-zinc-500" : "text-rose-600"}`}>{env.quiet ? "Inside quiet hours: " : ""}{env.error}</p>}
      {env?.data != null ? children(env.data) : !env?.error && query.isLoading && <p className="text-sm text-zinc-500">Loading</p>}
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
          className="mb-2 w-full max-w-xs rounded border border-zinc-300 bg-transparent px-2 py-1 text-sm dark:border-zinc-700"
          placeholder="Filter"
          aria-label="Filter rows"
          value={globalFilter}
          onChange={(e) => setGlobalFilter(e.target.value)}
        />
      )}
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead>
            {table.getHeaderGroups().map((hg) => (
              <tr key={hg.id} className="border-b border-zinc-200 text-xs text-zinc-500 dark:border-zinc-800">
                {hg.headers.map((h) => {
                  const sorted = h.column.getIsSorted();
                  return (
                    <th key={h.id} className="py-1.5 pr-3 font-medium whitespace-nowrap">
                      {h.column.getCanSort() ? (
                        <button className="hover:text-zinc-900 dark:hover:text-zinc-100" onClick={h.column.getToggleSortingHandler()}>
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
              <tr key={r.id} className="border-b border-zinc-100 align-top last:border-0 dark:border-zinc-900">
                {r.getAllCells().map((c) => (
                  <td key={c.id} className="py-1.5 pr-3">
                    <table.FlexRender cell={c} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {rows.length === 0 && <p className="py-2 text-sm text-zinc-500">{empty}</p>}
      </div>
    </div>
  );
}

/** External link. Only http(s) URLs become links, so a `javascript:` URL from
 * config or a backend can never run. */
export function Ext({ href, children }: { href: string | null | undefined; children: ReactNode }) {
  if (!href || !/^https?:\/\//i.test(href)) return <>{children}</>;
  return (
    <a className="underline decoration-zinc-400 underline-offset-2 hover:decoration-zinc-900 dark:hover:decoration-zinc-100" href={href} target="_blank" rel="noreferrer">
      {children}
    </a>
  );
}

export function Button({ children, onClick, disabled, tone = "default" }: { children: ReactNode; onClick: () => void; disabled?: boolean; tone?: "default" | "danger" }) {
  const c =
    tone === "danger"
      ? "border-rose-300 text-rose-700 hover:bg-rose-50 dark:border-rose-900 dark:text-rose-300 dark:hover:bg-rose-950"
      : "border-zinc-300 hover:bg-zinc-100 dark:border-zinc-700 dark:hover:bg-zinc-900";
  return (
    <button className={`rounded border px-2 py-0.5 text-xs disabled:opacity-50 ${c}`} onClick={onClick} disabled={disabled}>
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
    <dialog ref={ref} onClose={onClose} className="m-auto w-[min(28rem,90vw)] rounded-lg border border-zinc-200 bg-white p-5 text-zinc-900 backdrop:bg-black/40 dark:border-zinc-800 dark:bg-zinc-950 dark:text-zinc-100">
      <form
        method="dialog"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) onConfirm(typed);
        }}
      >
        <h3 className="mb-2 font-semibold">{title}</h3>
        <div className="mb-4 text-sm text-zinc-600 dark:text-zinc-400">{body}</div>
        {typeToConfirm && (
          <label className="mb-4 block text-sm">
            Type <code className="font-mono">{typeToConfirm}</code> to confirm
            <input autoFocus className="mt-1 w-full rounded border border-zinc-300 bg-transparent px-2 py-1 font-mono dark:border-zinc-700" value={typed} onChange={(e) => setTyped(e.target.value)} />
          </label>
        )}
        <div className="flex justify-end gap-2">
          <button type="button" className="rounded px-3 py-1 text-sm hover:bg-zinc-100 dark:hover:bg-zinc-900" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" disabled={!ready} className="rounded bg-zinc-900 px-3 py-1 text-sm text-white disabled:opacity-40 dark:bg-zinc-100 dark:text-zinc-900">
            {confirmLabel}
          </button>
        </div>
      </form>
    </dialog>
  );
}

// Pending updates (RFC 0020): Proxmox packages and reboots, Kubernetes,
// distribution and Flux versions, and chart versions. Read-only: Homie never
// installs anything. "Available" comes from opt-in upstream lookups; release
// notes and the security hint from GitHub releases (RFC 0026).

import { useQ, type ChartUpdate, type ClusterUpdate, type ImageUpdate, type Latest, type Notes, type PveUpdate, type Updates as Report } from "../api";
import { Badge, DataTable, Ext, Source, type Col } from "../ui";

/** "1.27.5 / 1.29.0": the newest patch, then the newest overall. */
export function available(l: Latest | undefined): string {
  return [...new Set([l?.patch, l?.newest].filter(Boolean))].join(" / ");
}

/** Upstream text is untrusted: every part of it is rendered as text. */
function NotesCell({ n }: { n: Notes | null }) {
  if (!n) return null;
  const count = n.releases.length;
  const label = !n.link ? "repository" : n.link.includes("/compare/") ? "compare" : "release list";
  return (
    <div className="flex min-w-48 items-start gap-1.5 text-xs">
      {n.security && (
        <span title="A hint, not a status: a release note in between mentions CVE, GHSA or security. Read the notes to be sure.">
          <Badge tone="muted">security</Badge>
        </span>
      )}
      {count > 0 && (
        <details>
          <summary className="cursor-pointer">{count === 1 ? "1 release" : `${count} releases`}</summary>
          <ul className="mt-1 max-w-md space-y-2 whitespace-normal">
            {n.releases.map((r) => (
              <li key={r.tag}>
                <Ext href={r.url}>{r.name}</Ext> <span className="text-ink-subtle">{r.published?.slice(0, 10)}</span>
                {r.excerpt && <p className="mt-0.5 break-words whitespace-pre-line text-ink-muted">{r.excerpt}</p>}
              </li>
            ))}
          </ul>
        </details>
      )}
      <Ext href={n.link ?? n.repository}>{label}</Ext>
    </div>
  );
}

const notesColumn = <T extends { notes: Notes | null }>(): Col<T> => ({
  id: "notes",
  header: "release notes",
  accessorFn: (r) => (r.notes ? `${r.notes.security ? "security " : ""}${r.notes.releases.length}` : ""),
  cell: ({ row }) => <NotesCell n={row.original.notes} />,
});

/** Updates whose notes mention a security fix first, the rest in order. */
function securityFirst<T extends { notes: Notes | null }>(rows: T[]): T[] {
  return [...rows].sort((a, b) => Number(!!b.notes?.security) - Number(!!a.notes?.security));
}

const pveColumns: Col<PveUpdate>[] = [
  { id: "node", header: "node", accessorFn: (r) => `${r.cluster}/${r.node}` },
  {
    id: "updates",
    header: "package updates",
    accessorFn: (r) => r.updates ?? -1,
    cell: ({ row }) =>
      row.original.updates === null ? (
        <span className="text-ink-subtle">unknown</span>
      ) : row.original.updates === 0 ? (
        "none"
      ) : (
        <details className="text-xs">
          <summary className="cursor-pointer">{row.original.updates}</summary>
          <ul className="mt-1 space-y-0.5">
            {row.original.packages.map((p) => (
              <li key={p.package}>
                {p.package} <span className="text-ink-subtle">{p.installed} → {p.new}</span>
              </li>
            ))}
          </ul>
        </details>
      ),
  },
  { id: "kernel", header: "running kernel", accessorFn: (r) => r.running_kernel },
  { id: "newest", header: "newest installed", accessorFn: (r) => r.newest_kernel ?? "" },
  { id: "reboot", header: "reboot", accessorFn: (r) => (r.reboot ? 0 : 1), cell: ({ row }) => (row.original.reboot ? <Badge tone="warn">pending</Badge> : <Badge tone="ok">not needed</Badge>) },
];

const clusterColumns: Col<ClusterUpdate>[] = [
  { id: "cluster", header: "cluster", accessorFn: (r) => r.cluster },
  { id: "distro", header: "distribution", accessorFn: (r) => `${r.distro} ${r.distro_version}`.trim() },
  { id: "distro-available", header: "available", accessorFn: (r) => available(r.distro_latest) || available(r.kubernetes_latest) },
  {
    id: "nodes",
    header: "kubelets",
    accessorFn: (r) => [...new Set(r.nodes.map((n) => n.kubelet))].join(", "),
    cell: ({ row }) => (
      <span>
        {[...new Set(row.original.nodes.map((n) => n.kubelet))].join(", ")} {row.original.skew && <Badge tone="warn">version skew</Badge>}
      </span>
    ),
  },
  { id: "flux", header: "Flux", accessorFn: (r) => r.flux ?? "" },
  { id: "flux-available", header: "available", accessorFn: (r) => available(r.flux_latest) },
];

const chartColumns: Col<ChartUpdate>[] = [
  { id: "chart", header: "chart", accessorFn: (r) => r.chart },
  { id: "release", header: "release", accessorFn: (r) => `${r.cluster}/${r.namespace}/${r.release}` },
  { id: "version", header: "version", accessorFn: (r) => r.version },
  { id: "available", header: "available", accessorFn: (r) => available(r.latest) },
  notesColumn<ChartUpdate>(),
  { id: "by", header: "managed by", accessorFn: (r) => r.managed_by },
  { id: "source", header: "repository", accessorFn: (r) => r.source ?? "", cell: ({ row }) => <span className="text-xs">{row.original.source ?? "unknown"}</span> },
];

const imageColumns: Col<ImageUpdate>[] = [
  { id: "image", header: "image", accessorFn: (r) => `${r.registry}/${r.repository}` },
  {
    id: "tag",
    header: "tag",
    accessorFn: (r) => (r.unversioned ? `unversioned ${r.tag ?? ""}` : r.pinned ? "pinned by digest" : (r.tag ?? "")),
    cell: ({ row }) => {
      const r = row.original;
      if (r.pinned) return <span className="text-ink-subtle">pinned by digest</span>;
      return r.unversioned ? <Badge tone="quiet">{`unversioned${r.tag ? `: ${r.tag}` : ""}`}</Badge> : r.tag;
    },
  },
  {
    id: "available",
    header: "available",
    accessorFn: (r) => available(r.latest),
    cell: ({ row }) => {
      const r = row.original;
      const a = available(r.latest);
      return r.unversioned && a ? <span>pin to {a}</span> : a;
    },
  },
  notesColumn<ImageUpdate>(),
  { id: "used", header: "used by", accessorFn: (r) => r.used_by.join(" "), cell: ({ row }) => <span className="block min-w-48 text-xs whitespace-normal">{row.original.used_by.join(", ")}</span> },
];

export function Updates() {
  const q = useQ<Report>("updates", "_");
  return (
    <Source title="Updates" query={q}>
      {(d) => (
        <div className="space-y-5">
          {d.errors.map((e) => (
            <p key={e} className="text-sm text-warn">
              {e}
            </p>
          ))}
          {d.proxmox.length > 0 && (
            <div>
              <h3 className="mb-1 text-sm font-semibold">Proxmox nodes</h3>
              <DataTable columns={pveColumns} data={d.proxmox} />
            </div>
          )}
          {d.kubernetes.length > 0 && (
            <div>
              <h3 className="mb-1 text-sm font-semibold">Kubernetes</h3>
              <DataTable columns={clusterColumns} data={d.kubernetes} filter={false} />
            </div>
          )}
          {d.charts.length > 0 && (
            <div>
              <h3 className="mb-1 text-sm font-semibold">Charts</h3>
              <DataTable columns={chartColumns} data={securityFirst(d.charts)} />
            </div>
          )}
          {d.images.length > 0 && (
            <div>
              <h3 className="mb-1 text-sm font-semibold">Images</h3>
              <DataTable columns={imageColumns} data={securityFirst(d.images)} empty="No images" />
            </div>
          )}
          <p className="text-xs text-ink-subtle">"Available" needs [updates.upstream] lookups turned on; package lists are what each node's daily pveupdate found. Release notes are GitHub releases; the security mark is a hint from their text.</p>
        </div>
      )}
    </Source>
  );
}

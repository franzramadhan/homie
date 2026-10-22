// Flux objects with reconcile (optionally with the source first).

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { act, useMeta, useQ, type ArgoApp, type FluxItem } from "../api";
import { Badge, Button, Confirm, DataTable, Ext, Source, ago, type Tone } from "../ui";

function shortRevision(r: string) {
  // "main@sha1:454db09a..." -> "main@454db09"
  const m = r.match(/^(.*?)@?sha\d+:([0-9a-f]{7})/);
  return m ? `${m[1] ? `${m[1]}@` : ""}${m[2]}` : r;
}

function Cluster({ name }: { name: string }) {
  const q = useQ<FluxItem[]>("flux", name);
  const readOnly = !!useMeta().data?.role;
  const qc = useQueryClient();
  const [onlyProblems, setOnlyProblems] = useState(false);
  const [target, setTarget] = useState<FluxItem | null>(null);
  const [withSource, setWithSource] = useState(true);
  const [status, setStatus] = useState("");
  if (q.data?.data && q.data.data.length === 0) return null;

  async function reconcile(item: FluxItem) {
    setTarget(null);
    setStatus(`reconciling ${item.kind}/${item.name}`);
    try {
      const hasSource = item.kind === "Kustomization" || item.kind === "HelmRelease";
      const r = await act<{ reconciled: string }>(`/api/act/flux/${encodeURIComponent(name)}/reconcile`, {
        kind: item.kind,
        namespace: item.namespace,
        name: item.name,
        with_source: hasSource && withSource,
      });
      setStatus(`requested reconcile of ${r.reconciled}`);
    } catch (e) {
      setStatus(`reconcile failed: ${(e as Error).message}`);
    }
    qc.invalidateQueries({ queryKey: ["q", "flux", name] });
  }

  return (
    <Source
      title={`Flux ${name}`}
      query={q}
      right={
        <label className="flex items-center gap-1">
          <input type="checkbox" checked={onlyProblems} onChange={(e) => setOnlyProblems(e.target.checked)} /> only not ready or suspended
        </label>
      }
    >
      {(d) => (
        <div className="space-y-2">
          {status && <p className="rounded-control bg-surface-2 px-2 py-1 text-sm" role="status">{status}</p>}
          <DataTable
            data={onlyProblems ? d.filter((i) => i.ready !== "True" || i.suspended) : d}
            columns={[
              ...(readOnly
                ? []
                : [{ id: "act", header: "", enableSorting: false, cell: ({ row }: { row: { original: FluxItem } }) => <Button onClick={() => setTarget(row.original)}>Reconcile</Button> }]),
              { accessorKey: "kind", header: "Kind" },
              { accessorKey: "namespace", header: "Namespace", cell: ({ row }) => <span className="whitespace-nowrap">{row.original.namespace}</span> },
              { accessorKey: "name", header: "Name", cell: ({ row }) => <Ext href={row.original.link}>{row.original.name}</Ext> },
              {
                accessorKey: "ready",
                header: "Ready",
                cell: ({ row }) => (
                  <span className="flex gap-1">
                    <Badge tone={row.original.ready === "True" ? "ok" : row.original.ready === "False" ? "bad" : "warn"}>{row.original.reason || row.original.ready}</Badge>
                    {row.original.suspended && <Badge tone="warn">suspended</Badge>}
                  </span>
                ),
              },
              { id: "revision", accessorFn: (i) => i.revision, header: "Revision", cell: ({ row }) => <code className="text-xs" title={row.original.revision}>{shortRevision(row.original.revision)}</code> },
              { id: "since", accessorFn: (i) => i.since, header: "Since", cell: ({ row }) => ago(row.original.since) },
              { accessorKey: "message", header: "Message", cell: ({ row }) => <span className="line-clamp-2 block max-w-md min-w-64 text-xs whitespace-normal text-ink-subtle" title={row.original.message}>{row.original.message}</span> },
            ]}
          />
          <Confirm
            open={target !== null}
            title={`Reconcile ${target?.kind}/${target?.name}?`}
            body={
              <>
                Flux re-applies it from its source now instead of at the next interval. Desired state does not change.
                {(target?.kind === "Kustomization" || target?.kind === "HelmRelease") && (
                  <label className="mt-3 flex items-center gap-2">
                    <input type="checkbox" checked={withSource} onChange={(e) => setWithSource(e.target.checked)} /> fetch the source first (like --with-source)
                  </label>
                )}
              </>
            }
            confirmLabel="Reconcile"
            onConfirm={() => target && reconcile(target)}
            onClose={() => setTarget(null)}
          />
        </div>
      )}
    </Source>
  );
}

const syncTone = (s: string): Tone => (s === "Synced" ? "ok" : s === "OutOfSync" ? "warn" : "muted");
const healthTone = (h: string): Tone =>
  h === "Healthy" ? "ok" : h === "Progressing" ? "warn" : h === "Suspended" ? "quiet" : h === "Degraded" || h === "Missing" ? "bad" : "muted";

function ArgoCd({ name, url }: { name: string; url: string | null }) {
  const readOnly = !!useMeta().data?.role;
  const q = useQ<ArgoApp[]>("argocd", name);
  const qc = useQueryClient();
  const [target, setTarget] = useState<ArgoApp | null>(null);
  const [hard, setHard] = useState(false);
  const [status, setStatus] = useState("");
  // clusters without Argo CD show nothing
  if (q.data?.data && q.data.data.length === 0) return null;

  async function refresh(app: ArgoApp) {
    setTarget(null);
    setStatus(`refreshing ${app.name}`);
    try {
      const r = await act<{ refreshed: string }>(`/api/act/argocd/${encodeURIComponent(name)}/refresh`, {
        namespace: app.namespace,
        name: app.name,
        hard,
      });
      setStatus(`requested ${r.refreshed}`);
    } catch (e) {
      setStatus(`refresh failed: ${(e as Error).message}`);
    }
    qc.invalidateQueries({ queryKey: ["q", "argocd", name] });
  }

  return (
    <Source title={`Argo CD ${name}`} query={q} right={url && <Ext href={url}>open Argo CD</Ext>}>
      {(d) => (
        <div className="space-y-2">
          {status && (
            <p className="rounded-control bg-surface-2 px-2 py-1 text-sm" role="status">
              {status}
            </p>
          )}
          <DataTable
            data={d}
            columns={[
              ...(readOnly
                ? []
                : [{ id: "act", header: "", enableSorting: false, cell: ({ row }: { row: { original: ArgoApp } }) => <Button onClick={() => setTarget(row.original)}>Refresh</Button> }]),
              { accessorKey: "name", header: "Application", cell: ({ row }) => <Ext href={row.original.link}>{row.original.name}</Ext> },
              { accessorKey: "project", header: "Project" },
              { accessorKey: "sync", header: "Sync", cell: ({ row }) => <Badge tone={syncTone(row.original.sync)}>{row.original.sync}</Badge> },
              { accessorKey: "health", header: "Health", cell: ({ row }) => <Badge tone={healthTone(row.original.health)}>{row.original.health}</Badge> },
              { id: "revision", accessorFn: (a) => a.revision, header: "Revision", cell: ({ row }) => <code className="text-xs" title={row.original.revision}>{row.original.revision.slice(0, 7)}</code> },
              { accessorKey: "destination", header: "Destination" },
              { id: "op", accessorFn: (a) => a.operation, header: "Last operation", cell: ({ row }) => (row.original.operation ? <Badge tone={row.original.operation === "Succeeded" ? "ok" : row.original.operation === "Running" ? "warn" : "bad"}>{row.original.operation}</Badge> : null) },
              { accessorKey: "message", header: "Message", cell: ({ row }) => <span className="line-clamp-2 block max-w-md min-w-64 text-xs whitespace-normal text-ink-subtle" title={row.original.message}>{row.original.message}</span> },
              { id: "at", accessorFn: (a) => a.reconciled_at, header: "Compared", cell: ({ row }) => ago(row.original.reconciled_at || null) },
            ]}
          />
          <Confirm
            open={target !== null}
            title={`Refresh ${target?.name}?`}
            body={
              <>
                Argo CD compares the application with git now instead of at the next poll. Nothing is synced.
                <label className="mt-3 flex items-center gap-2">
                  <input type="checkbox" checked={hard} onChange={(e) => setHard(e.target.checked)} /> hard refresh (also invalidates the manifest cache)
                </label>
              </>
            }
            confirmLabel="Refresh"
            onConfirm={() => target && refresh(target)}
            onClose={() => setTarget(null)}
          />
        </div>
      )}
    </Source>
  );
}

export function GitOps() {
  const m = useMeta().data;
  if (!m) return null;
  return (
    <div className="space-y-4">
      {m.kubernetes.map((k) => (
        <Cluster key={k.name} name={k.name} />
      ))}
      {m.kubernetes.map((k) => (
        <ArgoCd key={`argo-${k.name}`} name={k.name} url={k.argocd_url} />
      ))}
    </div>
  );
}

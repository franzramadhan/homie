// Kubernetes: nodes, unhealthy pods, workloads, warning events, Helm releases.

import { useMeta, useQ, type HelmRelease, type K8sOverview, type Sparks } from "../api";
import { Badge, Bar, DataTable, Ext, Source, Spark, ago, bytes } from "../ui";

function Cluster({ name, metrics }: { name: string; metrics: boolean }) {
  const q = useQ<K8sOverview>("k8s", name);
  const sparks = useQ<Sparks>("spark-k8s", name, metrics).data?.data;
  const helm = useQ<HelmRelease[]>("helm", name);
  return (
    <div className="space-y-4">
      <Source title={`Kubernetes ${name}`} query={q}>
        {(d) => (
          <div className="space-y-5">
            <DataTable
              filter={false}
              data={d.nodes}
              columns={[
                { accessorKey: "name", header: "Node", cell: ({ row }) => <Ext href={row.original.link}>{row.original.name}</Ext> },
                {
                  id: "ready",
                  accessorFn: (n) => n.ready,
                  header: "Status",
                  cell: ({ row }) => (
                    <span className="flex gap-1">
                      <Badge tone={row.original.ready ? "ok" : "bad"}>{row.original.ready ? "Ready" : "NotReady"}</Badge>
                      {row.original.unschedulable && <Badge tone="warn">cordoned</Badge>}
                    </span>
                  ),
                },
                { id: "roles", accessorFn: (n) => n.roles.join(","), header: "Roles" },
                { accessorKey: "version", header: "Kubelet" },
                { id: "cpu", accessorFn: (n) => n.cpu_requested / (n.cpu_allocatable || 1), header: "CPU requested", cell: ({ row }) => <Bar value={row.original.cpu_requested} max={row.original.cpu_allocatable} label={`${row.original.cpu_requested.toFixed(2)} / ${row.original.cpu_allocatable} cores`} /> },
                { id: "mem", accessorFn: (n) => n.mem_requested / (n.mem_allocatable || 1), header: "Memory requested", cell: ({ row }) => <Bar value={row.original.mem_requested} max={row.original.mem_allocatable} label={`${bytes(row.original.mem_requested)} / ${bytes(row.original.mem_allocatable)}`} /> },
                {
                  id: "usage",
                  header: metrics ? "Usage, 1 h" : "",
                  enableSorting: false,
                  cell: ({ row }) =>
                    metrics ? (
                      <span className="flex items-center gap-2 text-xs text-ink-subtle">
                        CPU <Spark values={sparks?.nodes[row.original.name]?.cpu} label="CPU" />
                        Mem <Spark values={sparks?.nodes[row.original.name]?.mem} label="Memory" />
                      </span>
                    ) : null,
                },
              ]}
            />
            <div>
              <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">Unhealthy pods</h3>
              <DataTable
                data={d.unhealthy_pods}
                empty="Every pod is running and ready"
                columns={[
                  { accessorKey: "namespace", header: "Namespace" },
                  { accessorKey: "name", header: "Pod", cell: ({ row }) => <Ext href={row.original.link}>{row.original.name}</Ext> },
                  { accessorKey: "phase", header: "Phase" },
                  { accessorKey: "reason", header: "Reason", cell: ({ row }) => <Badge tone="bad">{row.original.reason}</Badge> },
                  { accessorKey: "restarts", header: "Restarts" },
                  { accessorKey: "node", header: "Node" },
                ]}
              />
            </div>
            <div>
              <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">Workloads ({d.namespaces} namespaces)</h3>
              <DataTable
                data={d.workloads}
                columns={[
                  { accessorKey: "namespace", header: "Namespace" },
                  { accessorKey: "name", header: "Name", cell: ({ row }) => <Ext href={row.original.link}>{row.original.name}</Ext> },
                  { accessorKey: "kind", header: "Kind" },
                  {
                    id: "ready",
                    accessorFn: (w) => w.desired - w.ready,
                    header: "Ready",
                    cell: ({ row }) => (
                      <Badge tone={row.original.ready >= row.original.desired ? "ok" : row.original.ready > 0 ? "warn" : "bad"}>
                        {row.original.ready}/{row.original.desired}
                      </Badge>
                    ),
                  },
                ]}
              />
            </div>
            <div>
              <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">Warning events, last hour</h3>
              <DataTable
                data={d.events}
                empty="No warnings"
                columns={[
                  { id: "last", accessorFn: (e) => e.last, header: "When", cell: ({ row }) => ago(row.original.last) },
                  { accessorKey: "namespace", header: "Namespace" },
                  { accessorKey: "object", header: "Object" },
                  { accessorKey: "reason", header: "Reason" },
                  { accessorKey: "message", header: "Message", cell: ({ row }) => <span className="block min-w-64 text-xs whitespace-normal">{row.original.message}</span> },
                  { accessorKey: "count", header: "Count" },
                ]}
              />
            </div>
          </div>
        )}
      </Source>
      <Source title={`Helm releases ${name}`} query={helm}>
        {(d) => (
          <DataTable
            data={d}
            columns={[
              { accessorKey: "namespace", header: "Namespace" },
              { accessorKey: "name", header: "Release" },
              { accessorKey: "chart", header: "Chart" },
              { accessorKey: "chart_version", header: "Version" },
              { accessorKey: "app_version", header: "App" },
              { accessorKey: "revision", header: "Rev" },
              { accessorKey: "status", header: "Status", cell: ({ row }) => <Badge tone={row.original.status === "deployed" ? "ok" : "warn"}>{row.original.status}</Badge> },
              { id: "updated", accessorFn: (r) => r.updated, header: "Updated", cell: ({ row }) => ago(row.original.updated) },
            ]}
          />
        )}
      </Source>
    </div>
  );
}

export function Workloads() {
  const m = useMeta().data;
  if (!m) return null;
  return (
    <div className="space-y-6">
      {m.kubernetes.map((k) => (
        <Cluster key={k.name} name={k.name} metrics={k.metrics} />
      ))}
    </div>
  );
}

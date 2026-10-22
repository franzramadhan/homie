// Firing and pending alerts from every Grafana, with links into each one.

import { useMeta, useQ, type Alerts } from "../api";
import { Badge, DataTable, Ext, Source, ago } from "../ui";

function Instance({ name, url }: { name: string; url: string }) {
  const q = useQ<Alerts>("alerts", name);
  return (
    <Source
      title={`Grafana ${name}`}
      query={q}
      right={
        <>
          <Ext href={url}>open Grafana</Ext>
          {q.data?.data && <Ext href={q.data.data.link}>alert rules</Ext>}
        </>
      }
    >
      {(d) => (
        <DataTable
          data={d.alerts}
          empty="No firing or pending alerts"
          columns={[
            { accessorKey: "state", header: "State", cell: ({ row }) => <Badge tone={row.original.state === "pending" ? "warn" : "bad"}>{row.original.state}</Badge> },
            { accessorKey: "name", header: "Alert" },
            { accessorKey: "severity", header: "Severity" },
            { accessorKey: "summary", header: "Summary", cell: ({ row }) => <span className="block min-w-64 text-xs whitespace-normal">{row.original.summary}</span> },
            {
              id: "labels",
              accessorFn: (a) => Object.entries(a.labels).map(([k, v]) => `${k}=${v}`).join(" "),
              header: "Labels",
              cell: ({ row }) => (
                <span className="text-xs whitespace-normal text-ink-subtle">
                  {Object.entries(row.original.labels)
                    .filter(([k]) => !["alertname", "severity", "__alert_rule_uid__", "grafana_folder"].includes(k))
                    .map(([k, v]) => `${k}=${v}`)
                    .join(" ")}
                </span>
              ),
            },
            { id: "since", accessorFn: (a) => a.active_at, header: "Since", cell: ({ row }) => ago(row.original.active_at) },
          ]}
        />
      )}
    </Source>
  );
}

export function Observability() {
  const m = useMeta().data;
  if (!m) return null;
  return (
    <div className="space-y-4">
      {m.grafana.map((g) => (
        <Instance key={g.name} name={g.name} url={g.url} />
      ))}
    </div>
  );
}

// Security presets. Every preset is one table from the server, so a new
// preset needs no UI change.

import { useMeta, useQ, type SecurityTable } from "../api";
import { Badge, DataTable, Ext, Source, type Col } from "../ui";

type Row = (string | number | null)[];

const titles: Record<string, string> = {
  kyverno: "Kyverno policy reports",
  tetragon: "Tetragon runtime events, last 24 h",
  wazuh: "Wazuh alerts, last 24 h",
  debsecan: "Host CVEs (debsecan)",
};

function Preset({ preset }: { preset: string }) {
  const q = useQ<SecurityTable>("security", preset);
  return (
    <Source
      title={titles[preset] ?? preset}
      query={q}
      right={
        q.data?.data && (
          <>
            <Badge tone={q.data.data.total > 0 ? "warn" : "ok"}>
              {q.data.data.total} {q.data.data.total_label}
            </Badge>
            {q.data.data.link && <Ext href={q.data.data.link}>explore</Ext>}
          </>
        )
      }
    >
      {(d) => {
        const columns: Col<Row>[] = d.columns.map((c, i) => ({
          id: c,
          header: c.replace(/_/g, " "),
          accessorFn: (r: Row) => r[i] ?? "",
          cell: ({ row }) => {
            const v = row.original[i];
            return typeof v === "string" && v.length > 80 ? <span className="text-xs">{v}</span> : String(v ?? "");
          },
        }));
        return <DataTable columns={columns} data={d.rows} empty="Nothing found" />;
      }}
    </Source>
  );
}

export function Security() {
  const m = useMeta().data;
  if (!m) return null;
  return (
    <div className="space-y-4">
      {m.security.map((p) => (
        <Preset key={p} preset={p} />
      ))}
    </div>
  );
}

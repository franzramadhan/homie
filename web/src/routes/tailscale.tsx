// Every device in the tailnet. Watched hosts (config `hosts`) are the ones
// the overview counts; the rest are listed for reference.

import { useState } from "react";
import { useMeta, useQ, type Device } from "../api";
import { Badge, DataTable, Source, ago } from "../ui";

const DAY = 86_400_000;

function expiry(d: Device) {
  if (!d.key_expiry) return <span className="text-xs text-zinc-500">never</span>;
  const left = new Date(d.key_expiry).getTime() - Date.now();
  const tone = left < 0 ? "bad" : left < 14 * DAY ? "warn" : "muted";
  return <Badge tone={tone}>{left < 0 ? "expired" : `${Math.round(left / DAY)}d`}</Badge>;
}

export function Tailscale() {
  const m = useMeta().data;
  const q = useQ<Device[]>("tailscale", "_");
  const [filter, setFilter] = useState<"all" | "watched" | "offline">("all");
  if (!m?.tailscale) return null;
  return (
    <Source
      title="Tailscale devices"
      query={q}
      right={
        <select
          aria-label="Show"
          className="rounded border border-zinc-300 bg-transparent px-1 py-0.5 dark:border-zinc-700"
          value={filter}
          onChange={(e) => setFilter(e.target.value as typeof filter)}
        >
          <option value="all">all devices</option>
          <option value="watched">watched hosts</option>
          <option value="offline">offline</option>
        </select>
      }
    >
      {(all) => {
        const data = all.filter((d) => (filter === "watched" ? d.watched : filter === "offline" ? !d.online : true));
        return (
          <DataTable
            data={data}
            empty="No devices match"
            columns={[
              {
                accessorKey: "name",
                header: "Device",
                cell: ({ row }) => (
                  <span className="flex flex-wrap items-center gap-1">
                    {row.original.name}
                    {row.original.watched && <Badge tone="muted">watched</Badge>}
                    {row.original.external && <Badge tone="muted">shared in</Badge>}
                  </span>
                ),
              },
              {
                id: "status",
                accessorFn: (d) => (d.online ? 0 : d.quiet ? 1 : 2),
                header: "Status",
                cell: ({ row }) =>
                  row.original.online ? (
                    <Badge tone="ok">online</Badge>
                  ) : (
                    <Badge tone={row.original.quiet ? "quiet" : row.original.watched ? "bad" : "muted"}>
                      {row.original.quiet ? "scheduled off" : `seen ${ago(row.original.last_seen)}`}
                    </Badge>
                  ),
              },
              { accessorKey: "os", header: "OS" },
              { id: "ip", accessorFn: (d) => d.addresses[0] ?? "", header: "Address", cell: ({ row }) => <code className="text-xs">{row.original.addresses[0] ?? ""}</code> },
              { id: "tags", accessorFn: (d) => d.tags.join(" "), header: "Tags", cell: ({ row }) => <span className="text-xs text-zinc-500">{row.original.tags.join(", ")}</span> },
              { accessorKey: "user", header: "User", cell: ({ row }) => <span className="text-xs">{row.original.tags.length ? "" : row.original.user}</span> },
              {
                id: "version",
                accessorFn: (d) => d.client_version,
                header: "Client",
                cell: ({ row }) => (
                  <span className="flex items-center gap-1 text-xs">
                    {row.original.client_version}
                    {row.original.update_available && <Badge tone="warn">update</Badge>}
                  </span>
                ),
              },
              { id: "expiry", accessorFn: (d) => d.key_expiry ?? "9999", header: "Key expiry", cell: ({ row }) => expiry(row.original) },
            ]}
          />
        );
      }}
    </Source>
  );
}

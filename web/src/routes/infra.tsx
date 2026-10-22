// Proxmox: nodes, guests with power actions, storage, HA and failed tasks.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { act, get, useMeta, useQ, type Guest, type PveOverview } from "../api";
import { Badge, Bar, Button, Confirm, DataTable, Ext, Section, Source, bytes, duration, type Col } from "../ui";

type Action = "start" | "shutdown" | "reboot";

function PowerActions({ cluster, guest, onStatus }: { cluster: string; guest: Guest; onStatus: (s: string) => void }) {
  const qc = useQueryClient();
  const [pending, setPending] = useState<Action | null>(null);
  const [busy, setBusy] = useState(false);
  const running = guest.status === "running";

  async function run(action: Action, typed: string) {
    setPending(null);
    setBusy(true);
    onStatus(`${action} ${guest.name}: starting`);
    try {
      const { upid } = await act<{ upid: string }>(`/api/act/pve/${encodeURIComponent(cluster)}/power`, {
        node: guest.node,
        vmid: guest.vmid,
        type: guest.type,
        action,
        confirm: guest.protected ? typed : undefined,
      });
      const q = new URLSearchParams({ node: guest.node, upid });
      // poll the Proxmox task until it finishes (shutdown waits for the guest OS)
      for (let i = 0; i < 120; i++) {
        await new Promise((r) => setTimeout(r, 2000));
        const t = await get<{ status: string; exitstatus: string | null }>(`/api/act/pve/${encodeURIComponent(cluster)}/task?${q}`);
        if (t.status === "stopped") {
          onStatus(`${action} ${guest.name}: ${t.exitstatus ?? "done"}`);
          break;
        }
        onStatus(`${action} ${guest.name}: running (${(i + 1) * 2}s)`);
      }
    } catch (e) {
      onStatus(`${action} ${guest.name} failed: ${(e as Error).message}`);
    } finally {
      setBusy(false);
      qc.invalidateQueries({ queryKey: ["q", "pve", cluster] });
    }
  }

  return (
    <div className="flex gap-1">
      {running ? (
        <>
          <Button disabled={busy} onClick={() => setPending("shutdown")} tone="danger">
            Shut down
          </Button>
          <Button disabled={busy} onClick={() => setPending("reboot")}>
            Reboot
          </Button>
        </>
      ) : (
        <Button disabled={busy} onClick={() => setPending("start")}>
          Start
        </Button>
      )}
      <Confirm
        open={pending !== null}
        title={`${pending ?? ""} ${guest.name}?`}
        body={
          <>
            {guest.type === "qemu" ? "VM" : "Container"} {guest.vmid} on {guest.node}.
            {guest.protected && " This guest is protected."}
            {pending === "shutdown" && " A guest stopped here is not restarted by a nightly power schedule that only restarts what was running at stop time."}
          </>
        }
        typeToConfirm={guest.protected ? guest.name : undefined}
        confirmLabel={pending ?? ""}
        onConfirm={(typed) => pending && run(pending, typed)}
        onClose={() => setPending(null)}
      />
    </div>
  );
}

function Cluster({ name }: { name: string }) {
  const q = useQ<PveOverview>("pve", name);
  const [status, setStatus] = useState("");

  return (
    <Source
      title={`Proxmox ${name}`}
      query={q}
      right={q.data?.data && <Ext href={q.data.data.ui_url}>open Proxmox UI</Ext>}
    >
      {(d) => {
        const guestCols: Col<Guest>[] = [
          { accessorKey: "vmid", header: "ID" },
          {
            accessorKey: "name",
            header: "Name",
            cell: ({ row }) => (
              <span>
                {row.original.name}
                {row.original.protected && (
                  <>
                    {" "}
                    <Badge tone="muted">protected</Badge>
                  </>
                )}
              </span>
            ),
          },
          { accessorKey: "type", header: "Type" },
          { accessorKey: "node", header: "Node" },
          {
            accessorKey: "status",
            header: "Status",
            cell: ({ row }) => <Badge tone={row.original.status === "running" ? "ok" : "muted"}>{row.original.status}</Badge>,
          },
          { id: "cpu", accessorFn: (g) => g.cpu, header: "CPU", cell: ({ row }) => (row.original.status === "running" ? <Bar value={row.original.cpu} max={1} /> : null) },
          {
            id: "mem",
            accessorFn: (g) => (g.maxmem ? g.mem / g.maxmem : 0),
            header: "Memory",
            cell: ({ row }) => (row.original.status === "running" ? <Bar value={row.original.mem} max={row.original.maxmem} label={`${bytes(row.original.mem)} / ${bytes(row.original.maxmem)}`} /> : null),
          },
          { id: "uptime", accessorFn: (g) => g.uptime, header: "Uptime", cell: ({ row }) => duration(row.original.uptime) },
          { id: "tags", accessorFn: (g) => g.tags.join(" "), header: "Tags", cell: ({ row }) => <span className="text-xs text-zinc-500">{row.original.tags.join(", ")}</span> },
        ];
        if (d.can_power) {
          guestCols.push({
            id: "actions",
            header: "",
            enableSorting: false,
            cell: ({ row }) => (row.original.template ? null : <PowerActions cluster={name} guest={row.original} onStatus={setStatus} />),
          });
        }
        return (
          <div className="space-y-5">
            <p className="text-sm text-zinc-600 dark:text-zinc-400">
              {d.cluster.name ? `Cluster ${d.cluster.name}, ` : ""}
              {d.cluster.quorate === null ? "standalone node" : d.cluster.quorate ? "quorate" : "NOT quorate"}, {d.cluster.nodes_online}/{d.cluster.nodes_total || d.nodes.length} nodes online
              {!d.can_power && ". Power actions are off: the token has no VM.PowerMgmt."}
            </p>
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              {d.nodes.map((n) => (
                <div key={n.name} className="rounded border border-zinc-200 p-3 text-sm dark:border-zinc-800">
                  <div className="mb-2 flex items-center justify-between">
                    <span className="font-medium">{n.name}</span>
                    <Badge tone={n.online ? "ok" : "bad"}>{n.online ? `up ${duration(n.uptime)}` : "offline"}</Badge>
                  </div>
                  {n.online && (
                    <div className="space-y-1 text-xs">
                      <div className="flex items-center gap-2"><span className="w-12 text-zinc-500">CPU</span><Bar value={n.cpu} max={1} label={`${n.maxcpu} cores`} /></div>
                      <div className="flex items-center gap-2"><span className="w-12 text-zinc-500">Memory</span><Bar value={n.mem} max={n.maxmem} label={`${bytes(n.mem)} / ${bytes(n.maxmem)}`} /></div>
                      <div className="flex items-center gap-2"><span className="w-12 text-zinc-500">Root</span><Bar value={n.disk} max={n.maxdisk} label={`${bytes(n.disk)} / ${bytes(n.maxdisk)}`} /></div>
                    </div>
                  )}
                </div>
              ))}
            </div>
            {status && <p className="rounded bg-zinc-100 px-2 py-1 text-sm dark:bg-zinc-900" role="status">{status}</p>}
            <div>
              <h3 className="mb-1 text-xs font-semibold text-zinc-500 uppercase">Guests</h3>
              <DataTable columns={guestCols} data={d.guests.filter((g) => !g.template)} />
            </div>
            <div className="grid gap-5 lg:grid-cols-2">
              <div>
                <h3 className="mb-1 text-xs font-semibold text-zinc-500 uppercase">Storage</h3>
                <DataTable
                  filter={false}
                  data={d.storage}
                  columns={[
                    { accessorKey: "storage", header: "Storage" },
                    { accessorKey: "node", header: "Node" },
                    { accessorKey: "plugintype", header: "Type" },
                    { id: "used", accessorFn: (s) => (s.maxdisk ? s.disk / s.maxdisk : 0), header: "Used", cell: ({ row }) => <Bar value={row.original.disk} max={row.original.maxdisk} label={`${bytes(row.original.disk)} / ${bytes(row.original.maxdisk)}`} /> },
                  ]}
                />
              </div>
              <div className="space-y-5">
                {d.ha.length > 0 && (
                  <div>
                    <h3 className="mb-1 text-xs font-semibold text-zinc-500 uppercase">HA</h3>
                    <DataTable filter={false} data={d.ha} columns={[{ accessorKey: "sid", header: "Resource" }, { accessorKey: "node", header: "Node" }, { accessorKey: "state", header: "State" }]} />
                  </div>
                )}
                <div>
                  <h3 className="mb-1 text-xs font-semibold text-zinc-500 uppercase">Recent failed tasks</h3>
                  <DataTable
                    filter={false}
                    data={d.failed_tasks}
                    empty="No failed tasks"
                    columns={[
                      { id: "when", accessorFn: (t) => t.starttime, header: "When", cell: ({ row }) => new Date(row.original.starttime * 1000).toLocaleString() },
                      { accessorKey: "node", header: "Node" },
                      { accessorKey: "type", header: "Task" },
                      { accessorKey: "id", header: "Target" },
                      { accessorKey: "status", header: "Status", cell: ({ row }) => <span className="text-rose-600">{row.original.status}</span> },
                    ]}
                  />
                </div>
              </div>
            </div>
          </div>
        );
      }}
    </Source>
  );
}

export function Infra() {
  const m = useMeta().data;
  if (!m) return null;
  if (!m.proxmox.length) return <Section title="Infrastructure">No [proxmox.*] section is configured.</Section>;
  return (
    <div className="space-y-4">
      {m.proxmox.map((p) => (
        <Cluster key={p.name} name={p.name} />
      ))}
    </div>
  );
}

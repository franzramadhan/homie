// Proxmox: nodes, guests with power actions, storage, HA and failed tasks.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { act, get, useMeta, useQ, type Guest, type PveOverview } from "../api";
import { Badge, Bar, Button, Confirm, DataTable, Ext, Section, Source, bytes, duration, type Col } from "../ui";

type Action = "start" | "shutdown" | "reboot" | "stop";

type Status = { text: string; tone: "info" | "ok" | "bad" };

// Guests with an action in flight, kept outside React so a table re-render
// cannot re-enable the buttons halfway through a task.
const inFlight = new Set<string>();

const describe: Record<Action, string> = {
  start: "Starts the guest.",
  shutdown: "Asks the guest OS to shut down (ACPI, or the guest agent when enabled). A guest that ignores it keeps running: use Stop then.",
  reboot: "Asks the guest OS to reboot.",
  stop: "Hard power-off, like pulling the plug. Unsaved data in the guest is lost.",
};

function PowerActions({ cluster, guest, onStatus }: { cluster: string; guest: Guest; onStatus: (s: Status) => void }) {
  const qc = useQueryClient();
  const key = `${cluster}/${guest.node}/${guest.vmid}`;
  const [pending, setPending] = useState<Action | null>(null);
  const [, rerender] = useState(0);
  const busy = inFlight.has(key);
  const running = guest.status === "running";

  async function run(action: Action, typed: string) {
    setPending(null);
    inFlight.add(key);
    rerender((n) => n + 1);
    onStatus({ text: `${action} ${guest.name}: requested`, tone: "info" });
    try {
      const { upid } = await act<{ upid: string }>(`/api/act/pve/${encodeURIComponent(cluster)}/power`, {
        node: guest.node,
        vmid: guest.vmid,
        type: guest.type,
        action,
        confirm: typed || undefined,
      });
      const q = new URLSearchParams({ node: guest.node, upid });
      // poll the Proxmox task until it finishes (shutdown waits for the guest OS)
      for (let i = 0; ; i++) {
        await new Promise((r) => setTimeout(r, 2000));
        const t = await get<{ status: string; exitstatus: string | null }>(`/api/act/pve/${encodeURIComponent(cluster)}/task?${q}`);
        if (t.status === "stopped") {
          const ok = t.exitstatus === "OK";
          onStatus({
            text: ok
              ? `${action} ${guest.name}: done`
              : `${action} ${guest.name} failed: ${t.exitstatus ?? "unknown"}${action === "shutdown" ? ". The guest did not power off; use Stop for a hard power-off." : ""}`,
            tone: ok ? "ok" : "bad",
          });
          break;
        }
        if (i >= 300) {
          onStatus({ text: `${action} ${guest.name}: still running after 10 minutes, check the Proxmox task log`, tone: "bad" });
          break;
        }
        onStatus({ text: `${action} ${guest.name}: running (${(i + 1) * 2}s)`, tone: "info" });
      }
    } catch (e) {
      onStatus({ text: `${action} ${guest.name} failed: ${(e as Error).message}`, tone: "bad" });
    } finally {
      inFlight.delete(key);
      rerender((n) => n + 1);
      qc.invalidateQueries({ queryKey: ["q", "pve", cluster] });
    }
  }

  const needsName = pending === "stop" || guest.protected;
  return (
    <div className="flex gap-1">
      {running ? (
        <>
          <Button disabled={busy} onClick={() => setPending("shutdown")}>
            Shut down
          </Button>
          <Button disabled={busy} onClick={() => setPending("reboot")}>
            Reboot
          </Button>
          <Button disabled={busy} onClick={() => setPending("stop")} tone="danger">
            Stop
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
            {guest.type === "qemu" ? "VM" : "Container"} {guest.vmid} on {guest.node}. {pending && describe[pending]}
            {guest.protected && " This guest is protected."}
            {(pending === "shutdown" || pending === "stop") &&
              " A guest stopped here is not restarted by a nightly power schedule that only restarts what was running at stop time."}
          </>
        }
        typeToConfirm={needsName ? guest.name : undefined}
        confirmLabel={pending ?? ""}
        onConfirm={(typed) => pending && run(pending, typed)}
        onClose={() => setPending(null)}
      />
    </div>
  );
}

function Cluster({ name }: { name: string }) {
  const q = useQ<PveOverview>("pve", name);
  const [status, setStatus] = useState<Status | null>(null);

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
          { id: "tags", accessorFn: (g) => g.tags.join(" "), header: "Tags", cell: ({ row }) => <span className="text-xs whitespace-normal text-ink-subtle">{row.original.tags.join(", ")}</span> },
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
            <p className="text-sm text-ink-subtle">
              {d.cluster.name ? `Cluster ${d.cluster.name}, ` : ""}
              {d.cluster.quorate === null ? "standalone node" : d.cluster.quorate ? "quorate" : "NOT quorate"}, {d.cluster.nodes_online}/{d.cluster.nodes_total || d.nodes.length} nodes online
              {!d.can_power && ". Power actions are off: the token has no VM.PowerMgmt."}
            </p>
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              {d.nodes.map((n) => (
                <div key={n.name} className="rounded-control border border-hairline p-3 text-sm">
                  <div className="mb-2 flex items-center justify-between">
                    <span className="font-medium">{n.name}</span>
                    <Badge tone={n.online ? "ok" : "bad"}>{n.online ? `up ${duration(n.uptime)}` : "offline"}</Badge>
                  </div>
                  {n.online && (
                    <div className="space-y-1 text-xs">
                      <div className="flex items-center gap-2"><span className="w-12 text-ink-subtle">CPU</span><Bar value={n.cpu} max={1} label={`${n.maxcpu} cores`} /></div>
                      <div className="flex items-center gap-2"><span className="w-12 text-ink-subtle">Memory</span><Bar value={n.mem} max={n.maxmem} label={`${bytes(n.mem)} / ${bytes(n.maxmem)}`} /></div>
                      <div className="flex items-center gap-2"><span className="w-12 text-ink-subtle">Root</span><Bar value={n.disk} max={n.maxdisk} label={`${bytes(n.disk)} / ${bytes(n.maxdisk)}`} /></div>
                    </div>
                  )}
                </div>
              ))}
            </div>
            {status && (
              <p
                role="status"
                className={`rounded-control px-2 py-1 text-sm ${status.tone === "bad" ? "bg-bad/12 text-bad" : status.tone === "ok" ? "bg-ok/12 text-ok" : "bg-surface-2"}`}
              >
                {status.text}
              </p>
            )}
            <div>
              <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">Guests</h3>
              <DataTable columns={guestCols} data={d.guests.filter((g) => !g.template)} />
            </div>
            <div className="grid gap-5 lg:grid-cols-2">
              <div>
                <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">Storage</h3>
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
                    <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">HA</h3>
                    <DataTable filter={false} data={d.ha} columns={[{ accessorKey: "sid", header: "Resource" }, { accessorKey: "node", header: "Node" }, { accessorKey: "state", header: "State" }]} />
                  </div>
                )}
                <div>
                  <h3 className="mb-1 text-xs font-semibold text-ink-subtle uppercase">Recent failed tasks</h3>
                  <DataTable
                    filter={false}
                    data={d.failed_tasks}
                    empty="No failed tasks"
                    columns={[
                      { id: "when", accessorFn: (t) => t.starttime, header: "When", cell: ({ row }) => new Date(row.original.starttime * 1000).toLocaleString() },
                      { accessorKey: "node", header: "Node" },
                      { accessorKey: "type", header: "Task" },
                      { accessorKey: "id", header: "Target" },
                      { accessorKey: "status", header: "Status", cell: ({ row }) => <span className="text-bad">{row.original.status}</span> },
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

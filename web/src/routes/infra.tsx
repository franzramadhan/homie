// Proxmox: nodes, guests with power actions, storage, HA and failed tasks.
// Certificates (RFC 0018): every TLS chain Homie has seen or probed.

import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { act, get, useMeta, useQ, type CertRow, type Certs, type Guest, type PveOverview, type Sparks } from "../api";
import { Badge, Bar, Button, Confirm, DataTable, Ext, Section, Source, Spark, bytes, duration, type Col } from "../ui";

type Action = "start" | "shutdown" | "reboot" | "stop";

type Status = { text: string; tone: "info" | "ok" | "bad" };

/** The Proxmox UI's own console page (RFC 0023), with the user's Proxmox login. */
function pveConsole(ui: string, node: string, guest?: Guest): string {
  const q: Record<string, string> = guest
    ? guest.type === "lxc"
      ? { console: "lxc", xtermjs: "1", vmid: String(guest.vmid), vmname: guest.name, node }
      : { console: "kvm", novnc: "1", vmid: String(guest.vmid), vmname: guest.name, node, resize: "scale" }
    : { console: "shell", xtermjs: "1", node };
  return `${ui.replace(/\/$/, "")}/?${new URLSearchParams(q)}`;
}

/** The guest's page in the Proxmox UI. */
function pveGuest(ui: string, guest: Guest): string {
  return `${ui.replace(/\/$/, "")}/#v1:0:=${encodeURIComponent(`${guest.type}/${guest.vmid}`)}`;
}

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
  const m = useMeta().data;
  const q = useQ<PveOverview>("pve", name);
  const sparks = useQ<Sparks>("spark-pve", name).data?.data;
  const [status, setStatus] = useState<Status | null>(null);

  return (
    <Source
      title={`Proxmox ${name}`}
      query={q}
      right={q.data?.data?.ui_url && <Ext href={q.data.data.ui_url}>open Proxmox UI</Ext>}
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
          {
            id: "cpu",
            accessorFn: (g) => g.cpu,
            header: "CPU",
            cell: ({ row }) =>
              row.original.status === "running" ? (
                <span className="flex items-center gap-2">
                  <Bar value={row.original.cpu} max={1} />
                  <Spark values={sparks?.guests?.[row.original.vmid]?.cpu} max={1} label="CPU" />
                </span>
              ) : null,
          },
          {
            id: "mem",
            accessorFn: (g) => (g.maxmem ? g.mem / g.maxmem : 0),
            header: "Memory",
            cell: ({ row }) =>
              row.original.status === "running" ? (
                <span className="flex items-center gap-2">
                  <Bar value={row.original.mem} max={row.original.maxmem} label={`${bytes(row.original.mem)} / ${bytes(row.original.maxmem)}`} />
                  <Spark values={sparks?.guests?.[row.original.vmid]?.mem} max={1} label="Memory" />
                </span>
              ) : null,
          },
          { id: "uptime", accessorFn: (g) => g.uptime, header: "Uptime", cell: ({ row }) => duration(row.original.uptime) },
          { id: "tags", accessorFn: (g) => g.tags.join(" "), header: "Tags", cell: ({ row }) => <span className="text-xs whitespace-normal text-ink-subtle">{row.original.tags.join(", ")}</span> },
        ];
        // links need a Proxmox login and leak names, so never for a role (RFC 0016)
        const ui = m?.role ? "" : d.ui_url;
        const embedded = d.can_console && !m?.role;
        if (ui || embedded) {
          guestCols.push({
            id: "console",
            header: "Console",
            enableSorting: false,
            cell: ({ row }) => {
              const g = row.original;
              if (g.status !== "running") return null;
              return (
                <span className="flex gap-2 text-xs">
                  {embedded && (
                    <Link to="/console/$pve/$node/$type/$vmid" params={{ pve: name, node: g.node, type: g.type, vmid: String(g.vmid) }} className="underline decoration-ink-faint underline-offset-2 hover:decoration-ink">
                      open
                    </Link>
                  )}
                  {ui && (embedded ? <Ext href={pveGuest(ui, g)} title="Open in Proxmox">↗</Ext> : <Ext href={pveConsole(ui, g.node, g)}>console</Ext>)}
                </span>
              );
            },
          });
        }
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
              {!d.can_power && (m?.role ? ". Read-only view." : ". Power actions are off: the token has no VM.PowerMgmt.")}
            </p>
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              {d.nodes.map((n) => (
                <div key={n.name} className="rounded-control border border-hairline p-3 text-sm">
                  <div className="mb-2 flex items-center justify-between">
                    <span className="font-medium">
                      {n.name}
                      {ui && n.online && (
                        <span className="ml-2 text-xs font-normal">
                          <Ext href={pveConsole(ui, n.name)}>shell</Ext>
                        </span>
                      )}
                    </span>
                    <Badge tone={n.online ? "ok" : "bad"}>{n.online ? `up ${duration(n.uptime)}` : "offline"}</Badge>
                  </div>
                  {n.online && (
                    <div className="space-y-1 text-xs">
                      <div className="flex items-center gap-2"><span className="w-12 text-ink-subtle">CPU</span><Bar value={n.cpu} max={1} label={`${n.maxcpu} cores`} /><Spark values={sparks?.nodes[n.name]?.cpu} max={1} label="CPU" /></div>
                      <div className="flex items-center gap-2"><span className="w-12 text-ink-subtle">Memory</span><Bar value={n.mem} max={n.maxmem} label={`${bytes(n.mem)} / ${bytes(n.maxmem)}`} /><Spark values={sparks?.nodes[n.name]?.mem} max={1} label="Memory" /></div>
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

/** "in 9 days", "2 days ago" from now. */
export function untilText(iso: string | null): string {
  if (!iso) return "";
  const days = Math.floor((new Date(iso).getTime() - Date.now()) / 86_400_000);
  return days < 0 ? `${-days} days ago` : days === 0 ? "today" : `in ${days} days`;
}

const certColumns: Col<CertRow>[] = [
  {
    id: "status",
    header: "status",
    accessorFn: (r) => ({ bad: 0, warn: 1, ok: 2 })[r.status],
    cell: ({ row }) => <Badge tone={row.original.status}>{{ ok: "fine", warn: "check", bad: "broken" }[row.original.status]}</Badge>,
  },
  { id: "host", header: "host", accessorFn: (r) => `${r.host}${r.port === 443 ? "" : `:${r.port}`}` },
  {
    id: "expires",
    header: "expires",
    accessorFn: (r) => r.expires ?? "",
    cell: ({ row }) => {
      const r = row.original;
      if (!r.expires) return "";
      return (
        <span title={new Date(r.expires).toLocaleString()}>
          {untilText(r.expires)}
          {r.expires_cert && r.expires_cert !== "leaf" && <span className="text-ink-subtle"> ({r.expires_cert})</span>}
        </span>
      );
    },
  },
  { id: "issuer", header: "issuer", accessorFn: (r) => r.issuer },
  {
    id: "why",
    header: "why",
    accessorFn: (r) => r.reasons.join(", "),
    cell: ({ row }) => <span className="block min-w-48 text-xs whitespace-normal">{row.original.reasons.join(", ")}</span>,
  },
  {
    id: "chain",
    header: "chain",
    enableSorting: false,
    accessorFn: (r) => r.chain.map((c) => c.subject).join(" "),
    cell: ({ row }) => (
      <details className="text-xs">
        <summary className="cursor-pointer text-ink-subtle">{row.original.chain.length === 1 ? "1 certificate" : `${row.original.chain.length} certificates`}</summary>
        <ul className="mt-1 space-y-0.5">
          {row.original.chain.map((c, i) => (
            <li key={i}>
              <span className="text-ink-subtle">{c.role}</span> {c.subject}, until {new Date(c.not_after * 1000).toLocaleDateString()}
              {c.public && <span className="text-ink-subtle"> (bundled public root)</span>}
              {c.names?.length ? <span className="text-ink-subtle"> for {c.names.join(", ")}</span> : null}
            </li>
          ))}
        </ul>
      </details>
    ),
  },
  { id: "source", header: "found by", accessorFn: (r) => r.sources.join(", "), cell: ({ row }) => <span className="text-xs">{row.original.sources.join(", ")}</span> },
];

function Certificates() {
  const q = useQ<Certs>("certs", "_");
  return (
    <Source title="Certificates" query={q}>
      {(d) => (
        <>
          {d.errors.map((e) => (
            <p key={e} className="mb-2 text-sm text-warn">
              {e}
            </p>
          ))}
          <DataTable columns={certColumns} data={d.rows} empty="No TLS endpoints seen yet" />
        </>
      )}
    </Source>
  );
}

export function Infra() {
  const m = useMeta().data;
  if (!m) return null;
  if (!m.proxmox.length && !m.certificates) return <Section title="Infrastructure">No [proxmox.*] section is configured.</Section>;
  return (
    <div className="space-y-4">
      {m.proxmox.map((p) => (
        <Cluster key={p.name} name={p.name} />
      ))}
      {m.certificates && <Certificates />}
    </div>
  );
}

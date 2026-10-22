// One card per configured layer, each answering "is this part healthy?".

import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { useMeta, useQ, type Alerts, type Check, type Device, type Envelope, type FluxItem, type K8sOverview, type PveOverview, type SecurityTable } from "../api";
import { Badge, sourceTone, type Tone } from "../ui";

function Card<T>({ title, to, env, summary }: { title: string; to?: string; env: Envelope<T> | undefined; summary: (d: T) => { tone: Tone; lines: ReactNode[] } }) {
  const st = sourceTone(env);
  const s = env?.data != null ? summary(env.data) : null;
  // a failing source with no data is the headline; stale data still counts but is flagged
  const tone: Tone = st.tone === "quiet" ? "quiet" : !s ? (st.tone === "muted" ? "muted" : "bad") : st.tone === "warn" && s.tone === "ok" ? "warn" : s.tone;
  const border: Record<Tone, string> = {
    ok: "border-l-ok",
    warn: "border-l-warn",
    bad: "border-l-bad",
    quiet: "border-l-quiet",
    muted: "border-l-hairline-strong",
  };
  const body = (
    <div className={`h-full rounded-card border border-l-[3px] border-hairline bg-surface-1 p-4 ${border[tone]}`}>
      <div className="mb-2 flex items-start justify-between gap-2">
        <h2 className="text-sm font-semibold">{title}</h2>
        <Badge tone={st.tone}>{st.label}</Badge>
      </div>
      {s ? (
        <ul className="space-y-0.5 text-sm text-ink-muted">
          {s.lines.map((l, i) => (
            <li key={i}>{l}</li>
          ))}
        </ul>
      ) : (
        <p className={`text-sm ${st.tone === "quiet" ? "text-ink-subtle" : "text-bad"}`}>{env?.error ?? "Loading"}</p>
      )}
    </div>
  );
  return to ? (
    <Link to={to} className="block hover:opacity-90">
      {body}
    </Link>
  ) : (
    body
  );
}

function PveCard({ name }: { name: string }) {
  const q = useQ<PveOverview>("pve", name);
  return (
    <Card
      title={`Proxmox ${name}`}
      to="/infra"
      env={q.data}
      summary={(d) => {
        const running = d.guests.filter((g) => g.status === "running" && !g.template).length;
        const guests = d.guests.filter((g) => !g.template).length;
        const fullStorage = d.storage.filter((s) => s.maxdisk > 0 && s.disk / s.maxdisk > 0.85);
        const down = d.cluster.nodes_online < d.cluster.nodes_total || d.cluster.quorate === false;
        return {
          tone: down ? "bad" : fullStorage.length || d.failed_tasks.length ? "warn" : "ok",
          lines: [
            `${d.cluster.nodes_online}/${d.cluster.nodes_total || d.nodes.length} nodes online${d.cluster.quorate === false ? ", NOT quorate" : ""}`,
            `${running}/${guests} guests running`,
            fullStorage.length ? `storage above 85%: ${fullStorage.map((s) => `${s.storage}@${s.node}`).join(", ")}` : "storage below 85%",
            `${d.failed_tasks.length} recent failed tasks`,
          ],
        };
      }}
    />
  );
}

function K8sCard({ name }: { name: string }) {
  const q = useQ<K8sOverview>("k8s", name);
  return (
    <Card
      title={`Kubernetes ${name}`}
      to="/workloads"
      env={q.data}
      summary={(d) => {
        const ready = d.nodes.filter((n) => n.ready).length;
        const degraded = d.workloads.filter((w) => w.ready < w.desired).length;
        return {
          tone: ready < d.nodes.length ? "bad" : d.unhealthy_pods.length || degraded ? "warn" : "ok",
          lines: [
            `${ready}/${d.nodes.length} nodes ready`,
            `${d.unhealthy_pods.length} unhealthy pods, ${degraded} degraded workloads`,
            `${d.events.length} warning events in the last hour`,
          ],
        };
      }}
    />
  );
}

function FluxCard({ name }: { name: string }) {
  const q = useQ<FluxItem[]>("flux", name);
  return (
    <Card
      title={`Flux ${name}`}
      to="/gitops"
      env={q.data}
      summary={(d) => {
        const failing = d.filter((i) => i.ready === "False");
        const suspended = d.filter((i) => i.suspended).length;
        return {
          tone: failing.length ? "bad" : suspended ? "warn" : "ok",
          lines: [
            `${d.length - failing.length}/${d.length} objects ready`,
            failing.length ? `failing: ${failing.slice(0, 3).map((i) => i.name).join(", ")}` : "nothing failing",
            `${suspended} suspended`,
          ],
        };
      }}
    />
  );
}

function AlertsCard({ name }: { name: string }) {
  const q = useQ<Alerts>("alerts", name);
  return (
    <Card
      title={`Alerts ${name}`}
      to="/observability"
      env={q.data}
      summary={(d) => {
        const firing = d.alerts.filter((a) => a.state === "firing");
        return {
          tone: firing.length ? "bad" : d.alerts.length ? "warn" : "ok",
          lines: [
            `${firing.length} firing, ${d.alerts.length - firing.length} pending or erroring`,
            ...firing.slice(0, 3).map((a) => a.name),
          ],
        };
      }}
    />
  );
}

function SecurityCard({ preset }: { preset: string }) {
  const q = useQ<SecurityTable>("security", preset);
  return (
    <Card
      title={`Security ${preset}`}
      to="/security"
      env={q.data}
      summary={(d) => ({ tone: d.total > 0 ? "warn" : "ok", lines: [`${d.total} ${d.total_label}`, `${d.rows.length} rows`] })}
    />
  );
}

function TailscaleCard() {
  const q = useQ<Device[]>("tailscale", "_");
  return (
    <Card
      title="Tailscale"
      to="/tailscale"
      env={q.data}
      summary={(all) => {
        // only watched hosts (config `hosts`) count toward health
        const d = all.filter((x) => x.watched);
        const off = d.filter((x) => !x.online);
        const loud = off.filter((x) => !x.quiet);
        return {
          tone: loud.length ? "bad" : "ok",
          lines: [
            `${d.length - off.length}/${d.length} watched hosts online`,
            ...loud.map((x) => `offline: ${x.name} (seen ${new Date(x.last_seen).toLocaleString()})`),
            ...(off.length > loud.length ? [`${off.length - loud.length} scheduled off`] : []),
            `${all.filter((x) => x.online).length}/${all.length} devices online in the tailnet`,
          ],
        };
      }}
    />
  );
}

function HealthchecksCard() {
  const q = useQ<Check[]>("healthchecks", "_");
  return (
    <Card
      title="healthchecks"
      env={q.data}
      summary={(d) => {
        const down = d.filter((c) => c.status === "down");
        const late = d.filter((c) => c.status === "grace");
        return {
          tone: down.length ? "bad" : late.length ? "warn" : "ok",
          lines: [`${d.length - down.length - late.length}/${d.length} up`, ...down.map((c) => `down: ${c.name}`), ...late.map((c) => `late: ${c.name}`)],
        };
      }}
    />
  );
}

export function Overview() {
  const m = useMeta().data;
  if (!m) return null;
  const nothing = !m.proxmox.length && !m.kubernetes.length && !m.grafana.length && !m.tailscale && !m.healthchecks;
  if (nothing) {
    return <p className="text-sm text-ink-subtle">Nothing is configured yet. Add a [proxmox.*], [kubernetes.*] or [grafana.*] section and restart, then run `homie check`.</p>;
  }
  return (
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {m.proxmox.map((p) => (
        <PveCard key={p.name} name={p.name} />
      ))}
      {m.kubernetes.map((k) => (
        <K8sCard key={k.name} name={k.name} />
      ))}
      {m.kubernetes.map((k) => (
        <FluxCard key={k.name} name={k.name} />
      ))}
      {m.grafana.map((g) => (
        <AlertsCard key={g.name} name={g.name} />
      ))}
      {m.security.map((s) => (
        <SecurityCard key={s} preset={s} />
      ))}
      {m.tailscale && <TailscaleCard />}
      {m.healthchecks && <HealthchecksCard />}
    </div>
  );
}

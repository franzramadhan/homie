// One card per configured layer, each answering "is this part healthy?".

import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { useMeta, useQ, workflowHealth, type Alerts, type Certs, type Updates, type ArgoApp, type GhWorkflow, type BaoHealth, type Check, type PgCluster, type Device, type Envelope, type FluxItem, type K8sOverview, type PveOverview, type SecurityTable } from "../api";
import { Badge, sourceTone, type Tone } from "../ui";
import { untilText } from "./infra";
import { available } from "./updates";

function Card<T>({ title, to, env, summary }: { title: string; to?: string; env: Envelope<T> | undefined; summary: (d: T) => ReactNode[] }) {
  const st = sourceTone(env);
  const lines = env?.data != null ? summary(env.data) : null;
  // the server decides the tone (src/status.rs), so notifications agree with the page
  const tone: Tone = env?.tone ?? "muted";
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
      {lines ? (
        <ul className="space-y-0.5 text-sm text-ink-muted">
          {lines.map((l, i) => (
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
        return [
          `${d.cluster.nodes_online}/${d.cluster.nodes_total || d.nodes.length} nodes online${d.cluster.quorate === false ? ", NOT quorate" : ""}`,
          `${running}/${guests} guests running`,
          fullStorage.length ? `storage above 85%: ${fullStorage.map((s) => `${s.storage}@${s.node}`).join(", ")}` : "storage below 85%",
          `${d.failed_tasks.length} recent failed tasks`,
        ];
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
        return [
          `${ready}/${d.nodes.length} nodes ready`,
          `${d.unhealthy_pods.length} unhealthy pods, ${degraded} degraded workloads`,
          `${d.events.length} warning events in the last hour`,
        ];
      }}
    />
  );
}

function FluxCard({ name }: { name: string }) {
  const q = useQ<FluxItem[]>("flux", name);
  // clusters without Flux show nothing
  if (q.data?.data && q.data.data.length === 0) return null;
  return (
    <Card
      title={`Flux ${name}`}
      to="/gitops"
      env={q.data}
      summary={(d) => {
        const failing = d.filter((i) => i.ready === "False");
        const suspended = d.filter((i) => i.suspended).length;
        return [
          `${d.length - failing.length}/${d.length} objects ready`,
          failing.length ? `failing: ${failing.slice(0, 3).map((i) => i.name).join(", ")}` : "nothing failing",
          `${suspended} suspended`,
        ];
      }}
    />
  );
}

function ArgoCard({ name }: { name: string }) {
  const q = useQ<ArgoApp[]>("argocd", name);
  if (q.data?.data && q.data.data.length === 0) return null;
  return (
    <Card
      title={`Argo CD ${name}`}
      to="/gitops"
      env={q.data}
      summary={(d) => {
        const bad = d.filter((a) => a.health === "Degraded" || a.health === "Missing");
        const out = d.filter((a) => a.sync === "OutOfSync");
        return [
          `${d.length - out.length}/${d.length} applications synced`,
          bad.length ? `degraded: ${bad.slice(0, 3).map((a) => a.name).join(", ")}` : "none degraded",
        ];
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
        return [
          `${firing.length} firing, ${d.alerts.length - firing.length} pending or erroring`,
          ...firing.slice(0, 3).map((a) => a.name),
        ];
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
      summary={(d) => [`${d.total} ${d.total_label}`, `${d.rows.length} rows`]}
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
        return [
          `${d.length - off.length}/${d.length} watched hosts online`,
          ...loud.map((x) => `offline: ${x.name} (seen ${new Date(x.last_seen).toLocaleString()})`),
          ...(off.length > loud.length ? [`${off.length - loud.length} scheduled off`] : []),
          `${all.filter((x) => x.online).length}/${all.length} devices online in the tailnet`,
        ];
      }}
    />
  );
}

function OpenBaoCard({ name }: { name: string }) {
  const q = useQ<BaoHealth>("openbao", name);
  return (
    <Card
      title={`OpenBao ${name}`}
      env={q.data}
      summary={(d) => [d.sealed ? "SEALED" : !d.initialized ? "not initialized" : `unsealed, ${d.state}`, `version ${d.version}`]}
    />
  );
}

function CnpgCard({ name }: { name: string }) {
  const q = useQ<PgCluster[]>("cnpg", name);
  // clusters without CloudNativePG show nothing
  if (q.data?.data && q.data.data.length === 0) return null;
  return (
    <Card
      title={`PostgreSQL ${name}`}
      env={q.data}
      summary={(d) =>
        d.flatMap((c) => [
          `${c.namespace}/${c.name}: ${c.ready}/${c.instances} ready, primary ${c.primary}`,
          c.backup_age_hours === null
            ? "no completed backup"
            : `last backup ${c.backup_age_hours < 1 ? "under an hour" : `${Math.round(c.backup_age_hours)}h`} ago${c.backup_stale ? " (stale)" : ""}${c.archiving ? "" : ", WAL archiving off"}`,
        ])
      }
    />
  );
}

function CertificatesCard() {
  const q = useQ<Certs>("certs", "_");
  return (
    <Card
      title="Certificates"
      to="/infra"
      env={q.data}
      summary={(d) => {
        const loud = d.rows.filter((r) => r.status !== "ok");
        const now = new Date().toISOString();
        const next = d.rows.filter((r) => r.expires && r.expires > now).sort((a, b) => (a.expires! < b.expires! ? -1 : 1))[0];
        return [
          `${d.rows.length - loud.length}/${d.rows.length} fine`,
          ...loud.slice(0, 3).map((r) => `${r.host}: ${r.reasons[0] ?? r.status}`),
          ...(next ? [`next expiry: ${next.host}, ${untilText(next.expires)}`] : []),
          ...d.errors.slice(0, 2),
        ];
      }}
    />
  );
}

function UpdatesCard() {
  const q = useQ<Updates>("updates", "_");
  return (
    <Card
      title="Updates"
      to="/updates"
      env={q.data}
      summary={(d) => {
        const pkgs = d.proxmox.reduce((n, p) => n + p.updates, 0);
        const behind = d.charts.filter((c) => available(c.latest)).length;
        return [
          ...(d.proxmox.length ? [`${pkgs} package updates on ${d.proxmox.filter((p) => p.updates).length}/${d.proxmox.length} nodes`] : []),
          ...d.proxmox.filter((p) => p.reboot).map((p) => `${p.node}: reboot pending`),
          ...d.kubernetes.map((c) => {
            const a = available(c.distro_latest) || available(c.kubernetes_latest);
            return `${c.cluster}: ${c.distro} ${c.distro_version}${a ? `, ${a} available` : ""}${c.skew ? ", version skew" : ""}`;
          }),
          ...(d.charts.length ? [`${behind}/${d.charts.length} charts with a newer version`] : []),
          ...(d.images.length
            ? [`${d.images.filter((i) => !i.unversioned && available(i.latest)).length}/${d.images.length} images with a newer version, ${d.images.filter((i) => i.unversioned).length} unversioned`]
            : []),
        ];
      }}
    />
  );
}

function WorkflowsCard() {
  const q = useQ<GhWorkflow[]>("github", "_");
  return (
    <Card
      title="Workflows"
      to="/workflows"
      env={q.data}
      summary={(d) => {
        const states = d.map((w) => ({ w, h: workflowHealth(w) }));
        const failed = states.filter((s) => s.h === "failed" || s.w.error);
        const stuck = states.filter((s) => s.h === "stuck");
        return [
          `${states.filter((s) => s.h === "ok").length}/${d.length} workflows: last run succeeded`,
          ...failed.map((s) => `failed: ${s.w.name}`),
          ...stuck.map((s) => `queued over 10 min: ${s.w.name}`),
        ];
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
        return [`${d.length - down.length - late.length}/${d.length} up`, ...down.map((c) => `down: ${c.name}`), ...late.map((c) => `late: ${c.name}`)];
      }}
    />
  );
}

export function Overview() {
  const m = useMeta().data;
  if (!m) return null;
  // a role (RFC 0016) gets the cards of its pages only
  const page = (p: string) => !m.pages || m.pages.includes(p);
  const nothing = !m.proxmox.length && !m.kubernetes.length && !m.grafana.length && !m.tailscale && !m.healthchecks && !m.certificates && !m.updates;
  if (nothing) {
    return <p className="text-sm text-ink-subtle">Nothing is configured yet. Add a [proxmox.*], [kubernetes.*] or [grafana.*] section and restart, then run `homie check`.</p>;
  }
  return (
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {m.proxmox.map((p) => (
        <PveCard key={p.name} name={p.name} />
      ))}
      {page("workloads") && m.kubernetes.map((k) => <K8sCard key={k.name} name={k.name} />)}
      {page("gitops") && m.kubernetes.map((k) => <FluxCard key={k.name} name={k.name} />)}
      {!m.role && m.kubernetes.map((k) => <ArgoCard key={`argo-${k.name}`} name={k.name} />)}
      {m.grafana.map((g) => (
        <AlertsCard key={g.name} name={g.name} />
      ))}
      {m.openbao.map((b) => (
        <OpenBaoCard key={b.name} name={b.name} />
      ))}
      {page("services") && m.kubernetes.map((k) => <CnpgCard key={`pg-${k.name}`} name={k.name} />)}
      {m.certificates && <CertificatesCard />}
      {m.updates && <UpdatesCard />}
      {m.security.map((s) => (
        <SecurityCard key={s} preset={s} />
      ))}
      {m.tailscale && <TailscaleCard />}
      {m.github && <WorkflowsCard />}
      {m.healthchecks && <HealthchecksCard />}
    </div>
  );
}

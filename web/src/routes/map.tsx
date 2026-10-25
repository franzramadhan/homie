// The lab as a pixel village (RFC 0028). Every building reads the same
// queries, server tone and summary lines as its Overview card; the canvas in
// ../mapdraw.ts only draws them.

import { Link } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { useMeta, useQ, type Envelope, type HistoryRow, type K8sOverview, type PveOverview, type Updates } from "../api";
import { MAP_H, MAP_W, PLOTS, startMap, type Building, type Looks, type MapHandle } from "../mapdraw";
import { ago, Badge, sourceTone, type Tone } from "../ui";
import { summaries } from "./overview";
import { available } from "./updates";

const PLACES: Record<Building, { name: string; page: string; to: string }> = {
  pve: { name: "Barn", page: "Infrastructure", to: "/infra" },
  hist: { name: "homie's house", page: "History", to: "/history" },
  k8s: { name: "Greenhouse", page: "Workloads", to: "/workloads" },
  flux: { name: "Water tower", page: "GitOps", to: "/gitops" },
  graf: { name: "Weather vane", page: "Alerts", to: "/observability" },
  bell: { name: "Bell", page: "Overview", to: "/" },
  gh: { name: "Workshop", page: "Workflows", to: "/workflows" },
  logs: { name: "Library", page: "Logs", to: "/logs" },
  well: { name: "Well", page: "Overview", to: "/" },
  upd: { name: "Shipping bin", page: "Updates", to: "/updates" },
  sec: { name: "Guard post", page: "Security", to: "/security" },
  cert: { name: "Notice board", page: "Infrastructure", to: "/infra" },
  bao: { name: "Vault", page: "Overview", to: "/" },
  ts: { name: "Bridge", page: "Tailscale", to: "/tailscale" },
};
const WORD: Record<Tone, string> = { ok: "ok", warn: "warn", bad: "failing", quiet: "scheduled off", muted: "loading" };
const RANK: Record<Tone, number> = { muted: 0, quiet: 1, ok: 2, warn: 3, bad: 4 };
const DOT: Record<Tone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", quiet: "bg-quiet", muted: "bg-ink-faint" };

/** One instance's state: what the Overview card for it would show. */
type Spot = { label: string; tone: Tone; word: string; lines: string[]; n: number };
type Report = (key: string, s: Spot | null) => void;

function Feed<T>(props: { at: Building; kind: string; name: string; label: string; summary: (d: T) => string[]; count?: (d: T) => number; hideEmpty?: boolean; report: Report }) {
  const { at, kind, name, label, summary, count, hideEmpty, report } = props;
  const env: Envelope<T> | undefined = useQ<T>(kind, name).data;
  const key = `${at}/${kind}/${name}`;
  useEffect(() => {
    // like the Overview: clusters without Flux, Argo CD or CloudNativePG show nothing
    if (hideEmpty && Array.isArray(env?.data) && env.data.length === 0) return report(key, null);
    const src = sourceTone(env);
    const tone: Tone = env ? (env.tone ?? src.tone) : "muted";
    report(key, {
      label,
      tone,
      word: env?.error ? src.label : WORD[tone],
      lines: env?.data != null ? summary(env.data) : [env?.error ?? "Loading"],
      n: env?.data != null && count ? count(env.data) : 0,
    });
  }, [env, key]);
  useEffect(() => () => report(key, null), [key, report]);
  return null;
}

const history = (rows: HistoryRow[]) => [
  `${rows.length} recent entries`,
  ...rows.slice(0, 1).map((r) => `last: ${r.user ?? "local"} ${r.action} ${r.target}, ${ago(r.time)}`),
];
const running = (d: PveOverview) => d.guests.filter((g) => g.status === "running" && !g.template).length;
const pending = (d: Updates) =>
  d.proxmox.filter((p) => p.updates).length + d.charts.filter((c) => available(c.latest)).length + d.images.filter((i) => !i.unversioned && available(i.latest)).length;

export function MapPage() {
  const m = useMeta().data;
  const [spots, setSpots] = useState<Record<string, Spot>>({});
  const [active, setActive] = useState<Building | null>(null);
  const [popup, setPopup] = useState<Building | null>(null);
  const cv = useRef<HTMLCanvasElement>(null);
  const map = useRef<MapHandle | null>(null);
  const go = useRef<HTMLAnchorElement>(null);
  // walking away from the door closes its popup
  useEffect(() => setPopup((p) => (p === active ? p : null)), [active]);
  useEffect(() => go.current?.focus(), [popup]);
  const close = () => {
    setPopup(null);
    cv.current?.focus();
  };
  const reduce = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  const report = useCallback<Report>(
    (k, s) =>
      setSpots((prev) => {
        if (s) return { ...prev, [k]: s };
        if (!(k in prev)) return prev;
        const next = { ...prev };
        delete next[k];
        return next;
      }),
    [],
  );

  // one building per kind: the worst tone of its instances (RFC 0028, decision 2)
  const groups = new Map<Building, Spot[]>();
  for (const [k, s] of Object.entries(spots)) {
    const b = k.split("/")[0] as Building;
    groups.set(b, [...(groups.get(b) ?? []), s]);
  }
  if (m?.logs.length) groups.set("logs", [{ label: "Logs", tone: "muted", word: `${m.logs.length} sources`, lines: m.logs.map((l) => `${l.label} (${l.type})`), n: 0 }]);
  const looks: Looks = {};
  for (const [b, list] of groups) {
    const tone = list.reduce<Tone>((w, s) => (RANK[s.tone] > RANK[w] ? s.tone : w), "muted");
    looks[b] = { tone, n: list.reduce((n, s) => n + s.n, 0) };
  }
  const quiet = !!m?.quiet?.active;
  const shape = JSON.stringify(looks);

  useEffect(() => {
    if (!cv.current) return;
    const h = startMap(cv.current, { near: setActive, open: setPopup }, reduce);
    map.current = h;
    return () => h.stop();
  }, [reduce, !!m]);
  useEffect(() => map.current?.set(looks, quiet), [shape, quiet, !!m]);

  if (!m) return null;
  const page = (p: string) => !m.pages || m.pages.includes(p);
  const order = (Object.keys(PLACES) as Building[]).filter((b) => groups.has(b));
  const tone = (b: Building) => looks[b]!.tone;

  const feeds = (
    <>
      {m.proxmox.map((p) => (
        <Feed key={`pve-${p.name}`} at="pve" kind="pve" name={p.name} label={`Proxmox ${p.name}`} summary={summaries.pve} count={running} report={report} />
      ))}
      {page("workloads") &&
        m.kubernetes.map((k) => (
          <Feed key={`k8s-${k.name}`} at="k8s" kind="k8s" name={k.name} label={`Kubernetes ${k.name}`} summary={summaries.k8s} count={(d: K8sOverview) => d.nodes.length} report={report} />
        ))}
      {page("gitops") && m.kubernetes.map((k) => <Feed key={`flux-${k.name}`} at="flux" kind="flux" name={k.name} label={`Flux ${k.name}`} summary={summaries.flux} hideEmpty report={report} />)}
      {!m.role && m.kubernetes.map((k) => <Feed key={`argo-${k.name}`} at="flux" kind="argocd" name={k.name} label={`Argo CD ${k.name}`} summary={summaries.argocd} hideEmpty report={report} />)}
      {m.grafana.map((g) => (
        <Feed key={`graf-${g.name}`} at="graf" kind="alerts" name={g.name} label={`Alerts ${g.name}`} summary={summaries.alerts} report={report} />
      ))}
      {m.history && <Feed at="hist" kind="history" name="_" label="History" summary={history} report={report} />}
      {m.github && <Feed at="gh" kind="github" name="_" label="Workflows" summary={summaries.github} report={report} />}
      {m.openbao.map((b) => (
        <Feed key={`bao-${b.name}`} at="bao" kind="openbao" name={b.name} label={`OpenBao / Vault ${b.name}`} summary={summaries.openbao} report={report} />
      ))}
      {page("services") && m.kubernetes.map((k) => <Feed key={`pg-${k.name}`} at="well" kind="cnpg" name={k.name} label={`PostgreSQL ${k.name}`} summary={summaries.cnpg} hideEmpty report={report} />)}
      {m.certificates && <Feed at="cert" kind="certs" name="_" label="Certificates" summary={summaries.certs} report={report} />}
      {m.updates && <Feed at="upd" kind="updates" name="_" label="Updates" summary={summaries.updates} count={pending} report={report} />}
      {m.security.map((s) => (
        <Feed key={`sec-${s}`} at="sec" kind="security" name={s} label={`Security ${s}`} summary={summaries.security} report={report} />
      ))}
      {m.tailscale && <Feed at="ts" kind="tailscale" name="_" label="Tailscale" summary={summaries.tailscale} report={report} />}
      {m.healthchecks && <Feed at="bell" kind="healthchecks" name="_" label="healthchecks" summary={summaries.healthchecks} report={report} />}
    </>
  );

  // the list twin: what a screen reader or reduced motion reads first
  const list = (
    <section className="rounded-card border border-hairline bg-surface-1 p-4">
      <h2 className="mb-2 text-sm font-semibold">Buildings</h2>
      <ul className="space-y-1.5 text-sm">
        {order.map((b) => (
          <li key={b} className="flex flex-wrap items-baseline gap-x-2">
            <Link to={PLACES[b].to} className="font-medium hover:text-accent">
              {PLACES[b].name}
            </Link>
            {groups.get(b)!.map((s) => (
              <span key={s.label} className="inline-flex items-baseline gap-1.5 text-ink-muted">
                {s.label} <Badge tone={s.tone}>{s.word}</Badge> <span className="text-ink-subtle">{s.lines[0]}</span>
              </span>
            ))}
          </li>
        ))}
      </ul>
    </section>
  );

  return (
    <div className="space-y-3">
      {feeds}
      {reduce && list}
      <div className="mx-auto overflow-hidden rounded-card border border-hairline bg-surface-1" style={{ width: "min(100%, calc((100dvh - 230px) * 1.6))" }}>
        <div className="relative">
          <canvas
            ref={cv}
            tabIndex={0}
            className="block h-auto w-full outline-none [image-rendering:pixelated] focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
            style={{ aspectRatio: `${MAP_W} / ${MAP_H}` }}
            aria-label="Map of the lab: one building per component. The same buildings and states are listed as links on this page."
          />
          <div className="pointer-events-none absolute inset-0" aria-hidden="true">
            {order.map((b) => {
              const p = PLOTS[b];
              const top = (p.y * 16 - (b === "ts" ? 16 : 20)) / MAP_H;
              const left = ((p.x + p.w / 2) * 16) / MAP_W;
              return (
                <div
                  key={b}
                  className={`absolute flex -translate-y-full ${left > 0.9 ? "-translate-x-full" : "-translate-x-1/2"} items-center gap-1 rounded-control border border-hairline bg-surface-1/90 px-1.5 py-0.5 text-[11px] leading-none whitespace-nowrap text-ink`}
                  style={{ left: `${(left > 0.9 ? (p.x + p.w) * 16 / MAP_W : left) * 100}%`, top: `${top * 100}%` }}
                >
                  <i className={`inline-block size-1.5 rounded-full ${DOT[tone(b)]}`} />
                  <span className="hidden text-ink-muted md:inline">{PLACES[b].name}</span>
                  <span>{b === "logs" ? groups.get(b)![0].word : WORD[tone(b)]}</span>
                </div>
              );
            })}
          </div>
          {popup && groups.has(popup) && <Popup b={popup} spots={groups.get(popup)!} go={go} close={close} />}
        </div>
        <p className="border-t border-hairline px-3 py-2 text-xs text-ink-subtle" aria-live="polite">
          {!order.length
            ? "Nothing is configured yet, so the village is empty. Add a section to the config and restart."
            : popup
              ? `Enter opens ${PLACES[popup].page}, Esc closes.`
              : active
                ? `${PLACES[active].name}: press Enter.`
              : "Click the map, then walk with the arrow keys or WASD. Enter at a door shows what is inside; tapping a building walks homie there."}
        </p>
      </div>
      {!reduce && list}
    </div>
  );
}

/** What is inside a building: each instance and a link to its page. Esc closes it. */
function Popup({ b, spots, go, close }: { b: Building; spots: Spot[]; go: RefObject<HTMLAnchorElement | null>; close: () => void }) {
  const [dx, dy] = PLOTS[b].door;
  const left = ((dx + 0.5) * 16) / MAP_W;
  const shift = left < 0.25 ? "-translate-x-[10%]" : left > 0.75 ? "-translate-x-[90%]" : "-translate-x-1/2";
  return (
    <div
      role="dialog"
      aria-label={PLACES[b].name}
      className={`absolute z-10 w-72 max-w-[90%] -translate-y-full ${shift} rounded-card border border-hairline bg-surface-1 p-3 text-sm shadow-lg`}
      style={{ left: `${left * 100}%`, top: `${((dy * 16) / MAP_H) * 100}%` }}
      onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), close())}
    >
      <b className="font-semibold">{PLACES[b].name}</b>
      <ul className="mt-1 space-y-0.5 text-ink-muted">
        {spots.map((s) => (
          <li key={s.label}>
            <span className="text-ink">{s.label}</span> <Badge tone={s.tone}>{s.word}</Badge> {s.lines.slice(0, 3).join(", ")}
          </li>
        ))}
      </ul>
      <div className="mt-2 flex items-center gap-2">
        <Link ref={go} to={PLACES[b].to} className="rounded-control bg-accent px-2.5 py-0.5 text-xs text-on-accent hover:bg-accent-hover focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-1">
          Open {PLACES[b].page}
        </Link>
        <button className="text-xs text-ink-subtle hover:text-ink" onClick={close}>
          Close (Esc)
        </button>
      </div>
    </div>
  );
}

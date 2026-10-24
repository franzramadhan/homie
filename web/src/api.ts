// Types and fetch helpers for the Homie API. The shapes mirror the Rust
// structs in src/integrations and src/server/api.rs.

import { useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";

export type Envelope<T> = {
  data: T | null;
  fetched_at: string | null;
  stale: boolean;
  quiet: boolean;
  error: string | null;
  /** the overview card's status, computed by the server (RFC 0019) */
  tone?: "ok" | "warn" | "bad" | "quiet";
  reasons?: string[];
};

export type Meta = {
  version: string;
  auth: boolean;
  user: string | null;
  /** a read-only role (RFC 0016) and its pages; null for full access */
  role: string | null;
  pages: string[] | null;
  refresh_seconds: number;
  live_updates: boolean;
  quiet: { window: string; timezone: string; active: boolean } | null;
  proxmox: { name: string; quiet: boolean; ui_url: string; insecure: string | null }[];
  kubernetes: { name: string; quiet: boolean; headlamp_url: string | null; forward_token: boolean; metrics: boolean; argocd_url: string | null }[];
  grafana: { name: string; quiet: boolean; url: string; insecure: string | null }[];
  openbao: { name: string; quiet: boolean; url: string; insecure: string | null }[];
  logs: { label: string; type: "victorialogs" | "loki"; pod_query: string | null; node_query: string | null }[];
  security: string[];
  tailscale: boolean;
  github: { watch: number; helper: boolean; store: "configmap" | "file" | "session" | null; install_url: string | null } | null;
  healthchecks: { url: string } | null;
  /** certificate expiry (RFC 0018); never for a role */
  certificates: boolean;
  /** pending updates (RFC 0020); never for a role */
  updates: boolean;
};

export type PveNode = {
  name: string;
  online: boolean;
  cpu: number;
  maxcpu: number;
  mem: number;
  maxmem: number;
  disk: number;
  maxdisk: number;
  uptime: number;
};

export type Guest = {
  vmid: number;
  name: string;
  type: "qemu" | "lxc";
  node: string;
  status: string;
  cpu: number;
  maxcpu: number;
  mem: number;
  maxmem: number;
  uptime: number;
  tags: string[];
  protected: boolean;
  template: boolean;
};

export type PveOverview = {
  cluster: { name: string | null; quorate: boolean | null; nodes_online: number; nodes_total: number };
  nodes: PveNode[];
  guests: Guest[];
  storage: { storage: string; node: string; status: string; plugintype: string; shared: boolean; disk: number; maxdisk: number }[];
  ha: { sid: string; node: string; state: string }[];
  failed_tasks: { node: string; upid: string; type: string; id: string; user: string; starttime: number; status: string }[];
  can_power: boolean;
  ui_url: string;
};

export type K8sOverview = {
  nodes: {
    name: string;
    ready: boolean;
    unschedulable: boolean;
    version: string;
    roles: string[];
    cpu_allocatable: number;
    cpu_requested: number;
    mem_allocatable: number;
    mem_requested: number;
    link: string | null;
  }[];
  namespaces: number;
  workloads: { kind: string; namespace: string; name: string; ready: number; desired: number; link: string | null }[];
  unhealthy_pods: { namespace: string; name: string; phase: string; reason: string; restarts: number; node: string; link: string | null }[];
  events: { namespace: string; object: string; reason: string; message: string; count: number; last: string }[];
};

export type HelmRelease = {
  name: string;
  namespace: string;
  revision: number;
  status: string;
  chart: string;
  chart_version: string;
  app_version: string;
  updated: string;
};

export type FluxItem = {
  kind: string;
  namespace: string;
  name: string;
  ready: string;
  reason: string;
  message: string;
  revision: string;
  suspended: boolean;
  since: string;
  link: string | null;
};

export type ArgoApp = {
  namespace: string;
  name: string;
  project: string;
  source: string;
  target_revision: string;
  destination: string;
  sync: string;
  health: string;
  revision: string;
  operation: string;
  message: string;
  reconciled_at: string;
  link: string | null;
};

export type Alerts = {
  alerts: { name: string; state: "firing" | "pending" | "error"; severity: string; summary: string; active_at: string; labels: Record<string, string> }[];
  link: string;
};

export type SecurityTable = {
  columns: string[];
  rows: (string | number | null)[][];
  total: number;
  total_label: string;
  link: string | null;
};

export type Device = {
  name: string;
  hostname: string;
  online: boolean;
  last_seen: string;
  os: string;
  addresses: string[];
  tags: string[];
  user: string;
  client_version: string;
  update_available: boolean;
  key_expiry: string | null;
  external: boolean;
  watched: boolean;
  quiet: boolean;
};
export type Check = { name: string; status: string; last_ping: string | null; next_ping: string | null };

export type Latest = { patch: string | null; newest: string | null };
export type PveUpdate = {
  cluster: string;
  node: string;
  updates: number;
  packages: { package: string; installed: string; new: string }[];
  running_kernel: string;
  newest_kernel: string | null;
  reboot: boolean;
};
export type ClusterUpdate = {
  cluster: string;
  server: string;
  nodes: { name: string; kubelet: string }[];
  skew: boolean;
  distro: string;
  distro_version: string;
  kubernetes_latest: Latest;
  distro_latest: Latest;
  flux: string | null;
  flux_latest: Latest;
};
export type ChartUpdate = {
  cluster: string;
  namespace: string;
  release: string;
  chart: string;
  version: string;
  managed_by: "flux" | "argocd" | "helm";
  source: string | null;
  latest: Latest;
};
export type ImageUpdate = {
  cluster: string;
  image: string;
  registry: string;
  repository: string;
  tag: string | null;
  pinned: boolean;
  /** latest, stable, no tag: only the version to pin to is shown (RFC 0021) */
  unversioned: boolean;
  used_by: string[];
  latest: Latest;
};
export type Updates = { proxmox: PveUpdate[]; kubernetes: ClusterUpdate[]; charts: ChartUpdate[]; images: ImageUpdate[]; errors: string[] };
export type ChainCert = {
  role: "leaf" | "intermediate" | "root" | "unused";
  subject: string;
  issuer: string;
  not_before: number;
  not_after: number;
  names?: string[];
  /** a bundled public root: replaced by a Homie upgrade, never amber */
  public?: boolean;
};
export type CertRow = {
  host: string;
  port: number;
  sources: string[];
  status: "ok" | "warn" | "bad";
  reasons: string[];
  expires: string | null;
  expires_cert: string | null;
  issuer: string;
  chain: ChainCert[];
};
export type Certs = { rows: CertRow[]; errors: string[] };
export type BaoHealth = { state: string; initialized: boolean; sealed: boolean; version: string; cluster_name: string };
export type PgCluster = {
  namespace: string;
  name: string;
  phase: string;
  healthy: boolean;
  instances: number;
  ready: number;
  primary: string;
  archiving: boolean;
  last_backup: string | null;
  backup_age_hours: number | null;
  backup_stale: boolean;
};

export type GhRun = {
  number: number;
  title: string;
  status: string;
  conclusion: string | null;
  event: string;
  branch: string;
  actor: string;
  started: string;
  seconds: number | null;
  sha: string;
  commit_link: string;
  link: string;
};
export type GhWorkflow = { repo: string; workflow: string; source: "config" | "saved" | "session"; name: string; link: string; runs: GhRun[]; error: string | null };

/** The run that decides health: the newest completed one, and whether a run waits too long. */
export function workflowHealth(w: GhWorkflow): "failed" | "stuck" | "ok" | "unknown" {
  const queued = w.runs.find((r) => r.status === "queued" || r.status === "waiting" || r.status === "pending");
  if (queued && Date.now() - new Date(queued.started).getTime() > 10 * 60_000) return "stuck";
  const last = w.runs.find((r) => r.status === "completed" && r.conclusion !== "skipped" && r.conclusion !== "cancelled");
  if (!last) return "unknown";
  return last.conclusion === "success" ? "ok" : "failed";
}

export type LogRow = { time: string; message: string; labels: string };

/** Link to the log page with a source's template filled in. */
export function logsLink(m: Meta | undefined, kind: "pod_query" | "node_query", vars: Record<string, string>): string | null {
  const src = m?.logs.find((s) => s[kind]);
  const tpl = src?.[kind];
  if (!src || !tpl) return null;
  const q = Object.entries(vars).reduce((t, [k, v]) => t.replaceAll(`{${k}}`, v), tpl);
  return `/logs?${new URLSearchParams({ source: src.label, q })}`;
}

export type Series = { cpu: number[]; mem: number[] };
export type Sparks = { nodes: Record<string, Series>; guests?: Record<string, Series> };

export class ApiError extends Error {}

async function http<T>(path: string, init?: RequestInit): Promise<T> {
  const r = await fetch(path, { credentials: "same-origin", ...init });
  if (r.status === 401) {
    if (window.location.pathname !== "/login") window.location.href = "/login";
    throw new ApiError("login required");
  }
  const body = r.status === 204 ? null : await r.json().catch(() => null);
  if (!r.ok) throw new ApiError(body?.error ?? `${r.status} ${r.statusText}`);
  return body as T;
}

export function useMeta() {
  return useQuery({ queryKey: ["meta"], queryFn: () => http<Meta>("/api/meta"), refetchInterval: 60_000 });
}

// Live updates (RFC 0009): true while this tab's event stream is open.
let live = false;
const liveListeners = new Set<() => void>();
function setLive(v: boolean) {
  live = v;
  liveListeners.forEach((l) => l());
}
function useLive() {
  return useSyncExternalStore(
    (l) => (liveListeners.add(l), () => liveListeners.delete(l)),
    () => live,
  );
}

/**
 * One event stream per tab for the named queries on screen. The server sends
 * an envelope whenever one changes; polling takes over while it is down.
 */
export function useLiveStream() {
  const meta = useMeta();
  const qc = useQueryClient();
  const [keys, setKeys] = useState("");
  useEffect(() => {
    const cache = qc.getQueryCache();
    const update = () =>
      setKeys(
        cache
          .getAll()
          .filter((q) => q.queryKey[0] === "q" && q.getObserversCount() > 0)
          .map((q) => `${q.queryKey[1]}/${q.queryKey[2]}`)
          .sort()
          .join(","),
      );
    update();
    return cache.subscribe(update);
  }, [qc]);
  const enabled = !!meta.data?.live_updates;
  useEffect(() => {
    if (!enabled || !keys) return;
    const es = new EventSource(`/api/stream?q=${encodeURIComponent(keys)}`);
    es.onopen = () => setLive(true);
    // EventSource reconnects by itself; polling covers the gap
    es.onerror = () => setLive(false);
    es.onmessage = (m) => {
      const { key, envelope } = JSON.parse(m.data) as { key: string; envelope: Envelope<unknown> };
      const i = key.indexOf("/");
      qc.setQueryData(["q", key.slice(0, i), key.slice(i + 1)], envelope);
    };
    return () => {
      es.close();
      setLive(false);
    };
  }, [enabled, keys, qc]);
}

/** A named query: pushed by the live stream when it is open, else polled every refresh_seconds. */
export function useQ<T>(kind: string, name: string, enabled = true): UseQueryResult<Envelope<T>> {
  const meta = useMeta();
  const isLive = useLive();
  return useQuery({
    queryKey: ["q", kind, name],
    queryFn: () => http<Envelope<T>>(`/api/q/${kind}/${encodeURIComponent(name)}`),
    refetchInterval: isLive ? false : (meta.data?.refresh_seconds ?? 15) * 1000,
    enabled,
  });
}

/** POST an action. The X-Homie header is what the server's CSRF check expects. */
export function act<T>(path: string, body?: unknown): Promise<T> {
  return http<T>(path, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Homie": "1" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

export function get<T>(path: string): Promise<T> {
  return http<T>(path);
}

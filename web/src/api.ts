// Types and fetch helpers for the Homie API. The shapes mirror the Rust
// structs in src/integrations and src/server/api.rs.

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

export type Envelope<T> = {
  data: T | null;
  fetched_at: string | null;
  stale: boolean;
  quiet: boolean;
  error: string | null;
};

export type Meta = {
  version: string;
  auth: boolean;
  user: string | null;
  refresh_seconds: number;
  quiet: { window: string; timezone: string; active: boolean } | null;
  proxmox: { name: string; quiet: boolean; ui_url: string; insecure: string | null }[];
  kubernetes: { name: string; quiet: boolean; headlamp_url: string | null; forward_token: boolean }[];
  grafana: { name: string; quiet: boolean; url: string; insecure: string | null }[];
  security: string[];
  tailscale: boolean;
  healthchecks: { url: string } | null;
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

/** A named query, polled every refresh_seconds while the tab is visible. */
export function useQ<T>(kind: string, name: string): UseQueryResult<Envelope<T>> {
  const meta = useMeta();
  return useQuery({
    queryKey: ["q", kind, name],
    queryFn: () => http<Envelope<T>>(`/api/q/${kind}/${encodeURIComponent(name)}`),
    refetchInterval: (meta.data?.refresh_seconds ?? 15) * 1000,
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

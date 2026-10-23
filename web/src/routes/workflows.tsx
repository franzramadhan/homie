// GitHub Actions workflows from the shared watch list (RFC 0014), and the
// picker: it saves the watch list (RFC 0015), or prints `[[github.watch]]`
// config lines when saving is off.

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { act, get, useMeta, useQ, workflowHealth, type GhRun, type GhWorkflow } from "../api";
import { Badge, DataTable, Ext, Section, Source, ago, type Tone } from "../ui";

function runTone(r: GhRun): Tone {
  if (r.status !== "completed") return "warn";
  if (r.conclusion === "success") return "ok";
  if (r.conclusion === "skipped" || r.conclusion === "cancelled") return "muted";
  return "bad";
}

function duration(s: number | null) {
  if (s === null) return "";
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

// Session mode (RFC 0017): the browser remembers the last picks, names only,
// so a returning visitor can re-add them after signing in again.
const PICKS = "homie.github.picks";
function lastPicks(): Record<string, string[]> {
  try {
    return JSON.parse(localStorage.getItem(PICKS) ?? "{}");
  } catch {
    return {};
  }
}
function rememberPicks(repo: string, files: string[]) {
  try {
    const all = lastPicks();
    if (files.length) all[repo] = files;
    else delete all[repo];
    localStorage.setItem(PICKS, JSON.stringify(all));
  } catch {
    // private windows may refuse storage; the picks just are not remembered
  }
}

function Picker({ store, installUrl, watched }: { store: string | null; installUrl: string | null; watched: GhWorkflow[] }) {
  const qc = useQueryClient();
  const session = store === "session";
  const [saved, setSaved] = useState("");
  const [repos, setRepos] = useState<string[] | null>(null);
  const [repo, setRepo] = useState("");
  const [flows, setFlows] = useState<{ name: string; file: string; state: string }[]>([]);
  const [picked, setPicked] = useState<string[]>([]);
  const [error, setError] = useState("");

  // without a GitHub sign-in (or after it expired) the server answers 428: sign in again
  async function open() {
    setError("");
    try {
      setRepos(await get<string[]>("/api/github/repos"));
    } catch (e) {
      if ((e as Error).message === "connect GitHub first") window.location.href = "/github/login";
      else setError((e as Error).message);
    }
  }
  // back from the GitHub sign-in (/github/callback redirects here with ?picker),
  // or from installing the app (its Setup URL adds ?setup_action=install|update)
  useEffect(() => {
    const q = new URLSearchParams(window.location.search);
    if (!q.has("picker") && !q.has("setup_action")) return;
    window.history.replaceState(null, "", window.location.pathname);
    open();
  }, []);
  function close() {
    setRepos(null);
    setRepo("");
    setFlows([]);
    setPicked([]);
  }
  const fromConfig = (f: string) => watched.some((w) => w.repo === repo && w.workflow === f && w.source === "config");
  async function pickRepo(r: string) {
    setRepo(r);
    // start from what is watched now
    setPicked(watched.filter((w) => w.repo === r).map((w) => w.workflow));
    try {
      setFlows(await get<{ name: string; file: string; state: string }[]>(`/api/github/workflows?repo=${encodeURIComponent(r)}`));
    } catch (e) {
      setError((e as Error).message);
    }
  }
  async function save() {
    setError("");
    try {
      await act("/api/github/watch", { repo, workflows: picked });
      if (session) rememberPicks(repo, picked);
      await qc.invalidateQueries({ queryKey: ["q", "github", "_"] });
      setSaved(`Saved: ${picked.length} watched in ${repo}.`);
      close();
    } catch (e) {
      setError((e as Error).message);
    }
  }
  // remembered picks for repositories this sign-in can still see
  const remembered = Object.entries(lastPicks()).filter(([r]) => repos?.includes(r));
  const rememberedCount = remembered.reduce((n, [, f]) => n + f.length, 0);
  async function readd() {
    setError("");
    try {
      for (const [r, files] of remembered) await act("/api/github/watch", { repo: r, workflows: files });
      await qc.invalidateQueries({ queryKey: ["q", "github", "_"] });
      setSaved(`Re-added ${rememberedCount} workflows.`);
      close();
    } catch (e) {
      setError((e as Error).message);
    }
  }
  const toml = picked.map((f) => `[[github.watch]]\nrepo = "${repo}"\nworkflow = "${f}"`).join("\n\n");
  const control = "rounded-control border border-hairline-strong bg-surface-2 px-2 py-1 text-sm";

  return (
    <Section title="Add workflows">
      <p className="mb-3 text-sm text-ink-subtle">
        {session ? (
          <>
            Sign in with GitHub and pick workflows from your own repositories: install the Homie GitHub App on the ones you want to
            see. Your picks and your GitHub sign-in live only in this session and are gone when you log out or the session ends.
          </>
        ) : store ? (
          <>
            Sign in with GitHub to browse the repositories the Homie GitHub App can read and pick the workflows to watch. The list is
            shared by everyone using Homie; workflows set in the config stay as they are.
          </>
        ) : (
          <>
            Sign in with GitHub to browse the repositories the Homie GitHub App can read, pick workflows, and copy the lines into the
            <code className="mx-1 font-mono">[github]</code>section of the config. The watch list is shared and lives in the config.
          </>
        )}
      </p>
      {!repos ? (
        <div className="flex items-center gap-3">
          <button className="rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover" onClick={open}>
            {session
              ? watched.length > 0
                ? "Change workflows"
                : "Connect GitHub and pick workflows"
              : store
                ? "Change watched workflows"
                : "Pick workflows"}
          </button>
          {saved && <span className="text-sm text-ink-subtle">{saved}</span>}
        </div>
      ) : (
        <div className="space-y-3">
          {repos && repos.length === 0 && (
            <div className="space-y-2">
              <p className="text-sm text-ink-subtle">
                No repositories yet: install the Homie GitHub App on the repositories you want to watch, then connect again.
              </p>
              {installUrl && (
                <a className="inline-block rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover" href={installUrl}>
                  Install the GitHub App
                </a>
              )}
            </div>
          )}
          {session && rememberedCount > 0 && watched.length === 0 && (
            <button className="rounded-control border border-hairline-strong px-3 py-1 text-sm" onClick={readd}>
              Re-add your last {rememberedCount} workflows
            </button>
          )}
          {repos && repos.length > 0 && (
            <select aria-label="Repository" className={control} value={repo} onChange={(e) => pickRepo(e.target.value)}>
              <option value="">choose a repository</option>
              {repos.map((r) => (
                <option key={r}>{r}</option>
              ))}
            </select>
          )}
          {flows.length > 0 && (
            <fieldset className="space-y-1 text-sm">
              {flows.map((f) => (
                <label key={f.file} className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={picked.includes(f.file)}
                    disabled={!!store && fromConfig(f.file)}
                    onChange={(e) => setPicked(e.target.checked ? [...picked, f.file] : picked.filter((x) => x !== f.file))}
                  />
                  {f.name} <code className="text-xs text-ink-subtle">{f.file}</code>
                  {store && fromConfig(f.file) && <Badge tone="muted">config</Badge>}
                  {f.state !== "active" && <Badge tone="muted">{f.state}</Badge>}
                </label>
              ))}
            </fieldset>
          )}
          {store && repo && (
            <div className="flex items-center gap-3">
              <button className="rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover" onClick={save}>
                Save
              </button>
              <button className="rounded-control border border-hairline-strong px-3 py-1 text-sm" onClick={close}>
                Cancel
              </button>
            </div>
          )}
          {!store && toml && (
            <div>
              <textarea readOnly aria-label="Config lines" className={`${control} h-32 w-full max-w-xl font-mono text-xs`} value={toml} />
              <button className="mt-1 rounded-control border border-hairline-strong px-2 py-0.5 text-xs" onClick={() => navigator.clipboard.writeText(toml)}>
                Copy
              </button>
            </div>
          )}
        </div>
      )}
      {error && <p className="mt-2 text-sm text-bad">{error}</p>}
    </Section>
  );
}

export function Workflows() {
  const m = useMeta().data;
  const q = useQ<GhWorkflow[]>("github", "_");
  if (!m?.github) return null;
  return (
    <div className="space-y-4">
      <Source title="GitHub Actions" query={q}>
        {(flows) =>
          flows.length === 0 ? (
            <p className="text-sm text-ink-subtle">
              {m.github?.store === "session"
                ? "No workflows yet: connect GitHub below and pick some from your own repositories."
                : `No workflows are watched yet: ${m.github?.helper && m.github?.store ? "pick some below." : "add `[[github.watch]]` entries to the config."}`}
            </p>
          ) : (
            <div className="space-y-6">
              {flows.map((w) => {
                const h = workflowHealth(w);
                return (
                  <div key={`${w.repo}/${w.workflow}`}>
                    <h3 className="mb-1 flex flex-wrap items-center gap-2 text-sm font-semibold">
                      <Ext href={w.link}>{w.name}</Ext>
                      <span className="text-xs font-normal text-ink-subtle">{w.repo}</span>
                      {m.github?.store && w.source === "config" && <Badge tone="muted">config</Badge>}
                      <Badge tone={h === "ok" ? "ok" : h === "failed" ? "bad" : h === "stuck" ? "warn" : "muted"}>
                        {h === "ok" ? "last run succeeded" : h === "failed" ? "last run failed" : h === "stuck" ? "queued over 10 min" : "no completed run"}
                      </Badge>
                    </h3>
                    {w.error && <p className="mb-1 text-sm text-bad">{w.error}</p>}
                    <DataTable
                      filter={false}
                      data={w.runs}
                      empty="No runs yet"
                      columns={[
                        { accessorKey: "number", header: "#", cell: ({ row }) => <Ext href={row.original.link}>{row.original.number}</Ext> },
                        {
                          id: "state",
                          accessorFn: (r) => r.conclusion ?? r.status,
                          header: "Result",
                          cell: ({ row }) => <Badge tone={runTone(row.original)}>{row.original.conclusion ?? row.original.status.replace("_", " ")}</Badge>,
                        },
                        { accessorKey: "title", header: "Title", cell: ({ row }) => <span className="block max-w-md min-w-48 truncate" title={row.original.title}>{row.original.title}</span> },
                        { accessorKey: "sha", header: "Commit", cell: ({ row }) => <Ext href={row.original.commit_link}><code className="text-xs">{row.original.sha}</code></Ext> },
                        { accessorKey: "branch", header: "Branch" },
                        { accessorKey: "event", header: "Event" },
                        { accessorKey: "actor", header: "Actor" },
                        { id: "started", accessorFn: (r) => r.started, header: "Started", cell: ({ row }) => ago(row.original.started) },
                        { id: "took", accessorFn: (r) => r.seconds ?? -1, header: "Took", cell: ({ row }) => duration(row.original.seconds) },
                      ]}
                    />
                  </div>
                );
              })}
            </div>
          )
        }
      </Source>
      {m.github.helper && <Picker store={m.github.store} installUrl={m.github.install_url} watched={q.data?.data ?? []} />}
    </div>
  );
}

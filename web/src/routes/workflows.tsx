// GitHub Actions workflows from the shared watch list (RFC 0014), and the
// picker that turns a GitHub sign-in into `[[github.watch]]` config lines.

import { useState } from "react";
import { get, useMeta, useQ, workflowHealth, type GhRun, type GhWorkflow } from "../api";
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

function Picker() {
  const [repos, setRepos] = useState<string[] | null>(null);
  const [repo, setRepo] = useState("");
  const [flows, setFlows] = useState<{ name: string; file: string; state: string }[]>([]);
  const [picked, setPicked] = useState<string[]>([]);
  const [error, setError] = useState("");
  const connected = new URLSearchParams(window.location.search).has("picker");

  async function loadRepos() {
    setError("");
    try {
      setRepos(await get<string[]>("/api/github/repos"));
    } catch (e) {
      setError((e as Error).message);
    }
  }
  async function pickRepo(r: string) {
    setRepo(r);
    setPicked([]);
    try {
      setFlows(await get<{ name: string; file: string; state: string }[]>(`/api/github/workflows?repo=${encodeURIComponent(r)}`));
    } catch (e) {
      setError((e as Error).message);
    }
  }
  const toml = picked.map((f) => `[[github.watch]]\nrepo = "${repo}"\nworkflow = "${f}"`).join("\n\n");
  const control = "rounded-control border border-hairline-strong bg-surface-2 px-2 py-1 text-sm";

  return (
    <Section title="Add workflows">
      <p className="mb-3 text-sm text-ink-subtle">
        Sign in with GitHub to browse the repositories the Homie GitHub App can read, pick workflows, and copy the lines into the
        <code className="mx-1 font-mono">[github]</code>section of the config. The watch list is shared and lives in the config.
      </p>
      {!connected && !repos ? (
        <a className="inline-block rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover" href="/github/login">
          Connect GitHub
        </a>
      ) : (
        <div className="space-y-3">
          {!repos && (
            <button className="rounded-control bg-accent px-3 py-1 text-sm text-on-accent hover:bg-accent-hover" onClick={loadRepos}>
              List repositories
            </button>
          )}
          {repos && (
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
                    onChange={(e) => setPicked(e.target.checked ? [...picked, f.file] : picked.filter((x) => x !== f.file))}
                  />
                  {f.name} <code className="text-xs text-ink-subtle">{f.file}</code>
                  {f.state !== "active" && <Badge tone="muted">{f.state}</Badge>}
                </label>
              ))}
            </fieldset>
          )}
          {toml && (
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
            <p className="text-sm text-ink-subtle">No workflows are watched yet: add `[[github.watch]]` entries.</p>
          ) : (
            <div className="space-y-6">
              {flows.map((w) => {
                const h = workflowHealth(w);
                return (
                  <div key={`${w.repo}/${w.workflow}`}>
                    <h3 className="mb-1 flex flex-wrap items-center gap-2 text-sm font-semibold">
                      <Ext href={w.link}>{w.name}</Ext>
                      <span className="text-xs font-normal text-ink-subtle">{w.repo}</span>
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
      {m.github.helper && <Picker />}
    </div>
  );
}

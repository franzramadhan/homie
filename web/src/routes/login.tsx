// The login page: username and password when [local_login] is configured, a
// single-sign-on button when [auth] (OIDC) is. With only OIDC, it goes
// straight to the provider. Rendered outside the router: it must work
// without a session.

import { useEffect, useState, type FormEvent } from "react";

type Options = { oidc: boolean; local: boolean };

export function Login() {
  const [options, setOptions] = useState<Options | null>(null);
  const [username, setUsername] = useState("homie");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    fetch("/auth/options")
      .then((r) => r.json() as Promise<Options>)
      .then((o) => {
        if (!o.local && o.oidc) window.location.href = "/auth/login";
        else if (!o.local && !o.oidc) window.location.href = "/";
        else setOptions(o);
      })
      .catch(() => setError("Cannot reach Homie"));
  }, []);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    const r = await fetch("/auth/local", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-Homie": "1" },
      body: JSON.stringify({ username, password }),
    }).catch(() => null);
    setBusy(false);
    if (r?.ok) {
      window.location.href = "/";
      return;
    }
    const body = await r?.json().catch(() => null);
    setError(body?.error ?? "Login failed");
    setPassword("");
  }

  return (
    <main className="flex min-h-screen items-center justify-center px-4">
      <div className="w-full max-w-sm rounded-card border border-hairline bg-surface-1 p-6">
        <h1 className="mb-5 text-lg font-semibold tracking-tight">homie</h1>
        {options?.local && (
          <form onSubmit={submit} className="space-y-3">
            <label className="block text-sm">
              Username
              <input
                className="mt-1 w-full rounded-control border border-hairline-strong bg-transparent px-2 py-1.5"
                autoComplete="username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            </label>
            <label className="block text-sm">
              Password
              <input
                className="mt-1 w-full rounded-control border border-hairline-strong bg-transparent px-2 py-1.5"
                type="password"
                autoComplete="current-password"
                autoFocus
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
            <button
              type="submit"
              disabled={busy || !password}
              className="w-full rounded-control bg-accent py-1.5 text-sm text-on-accent hover:bg-accent-hover disabled:opacity-40"
            >
              {busy ? "Signing in" : "Sign in"}
            </button>
          </form>
        )}
        {options?.local && options.oidc && <p className="my-4 text-center text-xs text-ink-subtle">or</p>}
        {options?.oidc && (
          <a
            href="/auth/login"
            className="block w-full rounded-control border border-hairline-strong py-1.5 text-center text-sm hover:bg-surface-2"
          >
            Sign in with SSO
          </a>
        )}
        {error && (
          <p className="mt-3 text-sm text-bad" role="alert">
            {error}
          </p>
        )}
      </div>
    </main>
  );
}

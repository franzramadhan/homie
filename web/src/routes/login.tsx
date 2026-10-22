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
      <div className="w-full max-w-sm rounded-lg border border-zinc-200 bg-white p-6 dark:border-zinc-800 dark:bg-zinc-950">
        <h1 className="mb-5 text-lg font-semibold tracking-tight">homie</h1>
        {options?.local && (
          <form onSubmit={submit} className="space-y-3">
            <label className="block text-sm">
              Username
              <input
                className="mt-1 w-full rounded border border-zinc-300 bg-transparent px-2 py-1.5 dark:border-zinc-700"
                autoComplete="username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            </label>
            <label className="block text-sm">
              Password
              <input
                className="mt-1 w-full rounded border border-zinc-300 bg-transparent px-2 py-1.5 dark:border-zinc-700"
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
              className="w-full rounded bg-zinc-900 py-1.5 text-sm text-white disabled:opacity-40 dark:bg-zinc-100 dark:text-zinc-900"
            >
              {busy ? "Signing in" : "Sign in"}
            </button>
          </form>
        )}
        {options?.local && options.oidc && <p className="my-4 text-center text-xs text-zinc-500">or</p>}
        {options?.oidc && (
          <a
            href="/auth/login"
            className="block w-full rounded border border-zinc-300 py-1.5 text-center text-sm hover:bg-zinc-100 dark:border-zinc-700 dark:hover:bg-zinc-900"
          >
            Sign in with SSO
          </a>
        )}
        {error && (
          <p className="mt-3 text-sm text-rose-600" role="alert">
            {error}
          </p>
        )}
      </div>
    </main>
  );
}

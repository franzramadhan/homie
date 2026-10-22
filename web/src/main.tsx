import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Link, Outlet, RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { act, useMeta } from "./api";
import "./index.css";
import { GitOps } from "./routes/gitops";
import { Login } from "./routes/login";
import { Logs } from "./routes/logs";
import { Infra } from "./routes/infra";
import { Observability } from "./routes/observability";
import { Overview } from "./routes/overview";
import { Security } from "./routes/security";
import { Tailscale } from "./routes/tailscale";
import { Workflows } from "./routes/workflows";
import { Workloads } from "./routes/workloads";
import { Badge } from "./ui";

function Layout() {
  const meta = useMeta();
  const m = meta.data;
  const nav: [string, string, boolean][] = [
    ["/", "Overview", true],
    ["/infra", "Infrastructure", !!m?.proxmox.length],
    ["/workloads", "Workloads", !!m?.kubernetes.length],
    ["/gitops", "GitOps", !!m?.kubernetes.length],
    ["/workflows", "Workflows", !!m?.github],
    ["/observability", "Alerts", !!m?.grafana.length],
    ["/logs", "Logs", !!m?.logs.length],
    ["/security", "Security", !!m?.security.length],
    ["/tailscale", "Tailscale", !!m?.tailscale],
  ];
  return (
    <div className="mx-auto max-w-7xl px-4 py-4">
      <header className="mb-5 flex flex-wrap items-center gap-x-6 gap-y-2 border-b border-hairline">
        <Link to="/" className="py-2 text-xl font-semibold tracking-[-0.01em]">
          homie
        </Link>
        <nav className="flex flex-wrap gap-4 text-sm">
          {nav
            .filter(([, , on]) => on)
            .map(([to, label]) => (
              <Link
                key={to}
                to={to}
                className="-mb-px border-b-2 border-transparent py-2.5 text-ink-subtle hover:text-ink"
                activeProps={{ className: "border-accent! text-ink" }}
                activeOptions={{ exact: true }}
              >
                {label}
              </Link>
            ))}
        </nav>
        <div className="ml-auto flex items-center gap-3 text-xs text-ink-subtle">
          {m?.quiet && (
            <Badge tone={m.quiet.active ? "quiet" : "muted"}>
              quiet hours {m.quiet.window}
              {m.quiet.active ? " (now)" : ""}
            </Badge>
          )}
          {m?.user && <span>{m.user}</span>}
          {m?.auth && (
            <button
              className="underline decoration-ink-faint underline-offset-2 hover:text-ink"
              onClick={() => act("/auth/logout").finally(() => (window.location.href = "/login"))}
            >
              log out
            </button>
          )}
          {m && <span>v{m.version}</span>}
        </div>
      </header>
      {meta.error ? <p className="text-bad">Cannot reach Homie: {meta.error.message}</p> : <Outlet />}
    </div>
  );
}

const root = createRootRoute({ component: Layout });
const page = (path: string, component: () => React.ReactNode) => createRoute({ getParentRoute: () => root, path, component });
const router = createRouter({
  routeTree: root.addChildren([
    page("/", Overview),
    page("/infra", Infra),
    page("/workloads", Workloads),
    page("/gitops", GitOps),
    page("/workflows", Workflows),
    page("/observability", Observability),
    page("/security", Security),
    page("/logs", Logs),
    page("/tailscale", Tailscale),
  ]),
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 1, staleTime: 5_000 } } });

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {window.location.pathname === "/login" ? (
      <Login />
    ) : (
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    )}
  </StrictMode>,
);

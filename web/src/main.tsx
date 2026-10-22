import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Link, Outlet, RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { act, useMeta } from "./api";
import "./index.css";
import { GitOps } from "./routes/gitops";
import { Infra } from "./routes/infra";
import { Observability } from "./routes/observability";
import { Overview } from "./routes/overview";
import { Security } from "./routes/security";
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
    ["/observability", "Alerts", !!m?.grafana.length],
    ["/security", "Security", !!m?.security.length],
  ];
  return (
    <div className="mx-auto max-w-7xl px-4 py-4">
      <header className="mb-5 flex flex-wrap items-center gap-x-6 gap-y-2">
        <Link to="/" className="text-lg font-semibold tracking-tight">
          homie
        </Link>
        <nav className="flex flex-wrap gap-1 text-sm">
          {nav
            .filter(([, , on]) => on)
            .map(([to, label]) => (
              <Link
                key={to}
                to={to}
                className="rounded px-2 py-1 text-zinc-600 hover:bg-zinc-200 dark:text-zinc-400 dark:hover:bg-zinc-800"
                activeProps={{ className: "bg-zinc-200 text-zinc-900 dark:bg-zinc-800 dark:text-zinc-100" }}
                activeOptions={{ exact: true }}
              >
                {label}
              </Link>
            ))}
        </nav>
        <div className="ml-auto flex items-center gap-3 text-xs text-zinc-500">
          {m?.quiet && (
            <Badge tone={m.quiet.active ? "quiet" : "muted"}>
              quiet hours {m.quiet.window}
              {m.quiet.active ? " (now)" : ""}
            </Badge>
          )}
          {m?.user && <span>{m.user}</span>}
          {m?.auth && (
            <button
              className="underline"
              onClick={() => act("/auth/logout").finally(() => (window.location.href = "/auth/login"))}
            >
              log out
            </button>
          )}
          {m && <span>v{m.version}</span>}
        </div>
      </header>
      {meta.error ? <p className="text-rose-600">Cannot reach Homie: {meta.error.message}</p> : <Outlet />}
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
    page("/observability", Observability),
    page("/security", Security),
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
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);

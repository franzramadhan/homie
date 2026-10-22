# homie

One page for the whole homelab: hypervisor hosts and guests, Kubernetes
workloads, GitOps state, alerts and security findings, with a few safe
actions and deep links to the specialist UIs for everything else.

Homie is a single Rust binary with the web UI embedded. It has no database.
It runs:

- **in a cluster**, behind any OIDC provider (Authentik, Keycloak, Authelia,
  Pocket ID, Zitadel, Cloudflare Access, Google, ...)
- **on a laptop or LAN box**, bound to loopback, so the lab's state (or its
  last known state) is still visible when the cluster or the hosts are down

## Integrations

Every integration is optional. Configure only what your lab has.

| Layer | Integration | Notes |
|---|---|---|
| Hypervisor | Proxmox VE | One or more clusters, any node as endpoint, power actions |
| Kubernetes | Any conformant cluster | k0s, k3s, RKE2, Talos, kind, minikube, managed clouds. Auth by kubeconfig, in-cluster ServiceAccount, or the user's forwarded OIDC token. |
| GitOps | Flux v2 | Sources, Kustomizations, HelmReleases, reconcile |
| Helm | Helm 3 release secrets | Chart, version and status. Values are never shown. |
| Alerts and queries | Grafana (self-hosted or Grafana Cloud), any number of instances | Through the Grafana HTTP API with a Viewer token: Prometheus, Mimir, VictoriaMetrics, Loki and VictoriaLogs datasources |
| Security | Presets: Kyverno, Tetragon, Wazuh, debsecan | More presets later |
| Outage signals | Tailscale API, healthchecks.io (hosted or self-hosted) | Useful from outside when the lab is down |
| Deep links | Headlamp, Grafana, Proxmox UI | |

Behavior that suits a homelab:

- **Quiet hours.** Parts of the lab that are powered off on a schedule show
  as "scheduled off", not as an outage.
- **TLS options per integration.** Verify against a custom CA, verify
  against another name than the address, skip verification, or use plain
  HTTP. The last two are opt-in and flagged in the UI.
- **Configuration.** A TOML file, `HOMIE__*` environment variables, or both.
  Secrets come from env, files or a command (Keychain, `pass`, `op read`).

## Tested with

Kubernetes (k0s 1.36), Flux 2.9, Proxmox VE 9.2, Grafana 13 and Grafana
Cloud, Cloudflare Access as the OIDC provider. Other versions and
distributions are expected to work. Reports are welcome.

## Status

Design. See [RFC 0001](rfc/0001-homie-foundation.md) for the design and the
v0.1.0 milestone.

## License

MIT, see [LICENSE](LICENSE).

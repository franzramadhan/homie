# homie

One page for the whole homelab: Proxmox VE hosts and guests, the k0s
cluster, Flux, alerts from both Grafanas and security findings, with a few
safe actions and deep links to the specialist UIs.

It is a single Rust binary with the web UI embedded. Every integration is
optional and is configured by a TOML file, environment variables, or both.
It runs:

- **in a cluster**, behind any OIDC provider (Cloudflare Access, Authentik,
  Keycloak, Pocket ID, ...)
- **on a laptop or LAN box**, bound to loopback, so the lab's state (or its
  last known state) is still visible when the cluster or the hosts are down

Status: design. See [RFC 0001](rfc/0001-homie-foundation.md) for the design
and the v0.1.0 milestone.

## License

MIT, see [LICENSE](LICENSE).

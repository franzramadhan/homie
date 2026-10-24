# 0018: Certificate expiry

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-24 | - | - |

> Show when TLS certificates expire, read from the TLS handshake itself, so
> it works for any setup: cert-manager, an ingress controller's own ACME,
> Caddy, Tailscale certificates, a hand-made internal CA, a NAS or a router.

## Problem

An expired certificate breaks a lab quietly: a Proxmox node whose 90-day
certificate failed to renew, an Ingress whose ACME order is stuck, an
internal CA that nobody rotated. Each issuer reports failures in its own
place (a cert-manager condition, Traefik's log, a cron mail), and many
certificates have no issuer status at all.

## Proposal

The served certificate is the source of truth: Homie connects, completes or
fails the TLS handshake, and reads the leaf certificate. Issuer-specific
status is an optional extra.

- **Capture.** Homie's rustls config gets a `ServerCertVerifier` that
  records the leaf certificate, then delegates to the normal
  `WebPkiServerVerifier` (or to the configured CA, or accepts with
  `tls_verify = false`). The certificate is recorded even when verification
  fails, so an expired or untrusted certificate is still shown, with the
  verification error as the reason. Parsed with `x509-cert`: subject, SANs,
  issuer, `notBefore`, `notAfter`.
- **Targets**, three kinds, all read the same way:
  - **Integration endpoints**: every HTTPS URL Homie already uses (Proxmox
    nodes, OpenBao, Grafana, healthchecks, ...). Recorded on the normal
    requests, no extra connections.
  - **Listed targets**: `[[certificates.targets]]` with `host` (the name
    sent as SNI and checked against the SANs), optional `port` (443) and
    `address` (connect here instead of resolving `host`, like
    `tls_server_name`). For anything else on the network.
  - **Discovered targets** (opt-in per cluster): the hostnames in Ingress
    `spec.tls[].hosts` and Gateway API listeners with TLS, probed through
    `discover_address` (the ingress controller or gateway Service), with
    each hostname as SNI. Works for any controller without reading Secrets.
- A probe is a handshake only (no HTTP request), every 6 h per target, 5 s
  timeout, at most 4 at a time. A failed connection keeps the last known
  certificate and shows "unreachable".
- **Status**: amber when `notAfter` is within `warn_days` (default 14), the
  certificate is not yet valid, or the name does not match; red within 3
  days, expired, or not trusted by the configured roots. Every row shows
  the words ("expires in 9 days", "expired 2 days ago", "name mismatch").
- **cert-manager** (optional, off by default): when the CRD exists and
  `kubernetes.<name>.cert_manager = true`, a Certificate whose `Ready`
  condition is False adds "renewal failing: <message>" to the row of its
  DNS names. This is the early warning before the probe sees anything.
- **UI**: a "Certificates" card in the overview's Services section (counts
  by status, the soonest expiry) and a table on the Infrastructure page:
  host, source (integration, listed, `<cluster>/<namespace>/<ingress>`),
  issuer, expires, status.
- **Roles**: never shown to a role: hostnames and addresses (see the
  "roles never see IP addresses" fix).
- Named query `certs` from memory, kept in the snapshot so the last known
  state survives quiet hours and restarts.

### Configuration

| Key | Default | |
|---|---|---|
| `certificates.warn_days` | `14` | amber threshold |
| `certificates.interval` | `"6h"` | probe period for listed and discovered targets |
| `certificates.endpoints` | `true` | record integration endpoint certificates |
| `certificates.targets[].host` | - | name sent as SNI and checked |
| `certificates.targets[].port` | `443` | |
| `certificates.targets[].address` | `host` | connect here instead (IP or name) |
| `certificates.targets[].ca_file` | none | extra roots for this target |
| `kubernetes.<name>.certificates.discover` | `false` | probe Ingress and Gateway TLS hostnames |
| `kubernetes.<name>.certificates.discover_address` | - | `host:port` of the ingress controller or gateway |
| `kubernetes.<name>.cert_manager` | `false` | add "renewal failing" from Certificate conditions |

### Logs and metrics

| | |
|---|---|
| `msg="cert probe" host result` | debug, one per probe; warn when unreachable |

## Alternatives

| Option | Why not |
|---|---|
| cert-manager status as the source | Only cert-manager users; misses Proxmox, Caddy, Tailscale and manual certificates |
| Shelling out to `openssl s_client` | The image is `scratch` (no shell, no openssl); the rustls handshake is the same check |
| `reqwest` `tls_info` | Only after a successful handshake: misses exactly the expired certificates |
| Reading `kubernetes.io/tls` Secrets | Needs Secret read access across namespaces; the probe needs none |
| Blackbox exporter + Grafana alert | Works, but every target is configured twice and labs without Prometheus get nothing |
| Hand-written DER walk for `notAfter` | Parsing untrusted bytes; `x509-cert` (RustCrypto, `der` already in the tree) |

## Testing and rollout

- **Tests:** the recording verifier (valid, expired, untrusted, name
  mismatch, not yet valid) against local rustls servers with generated
  certificates; threshold mapping; target discovery from Ingress and
  Gateway fixtures; the cert-manager "renewal failing" merge; role hiding;
  unreachable keeps the last certificate.
- **Rollout:** integration endpoints on by default, nothing to configure.
  Listed and discovered targets need config and egress in network
  policies. Discovery needs `get, list` on Ingresses and Gateways (already
  readable in most setups); cert-manager needs `get, list` on Certificates.
- **Off switch:** `certificates.endpoints = false`, no `targets`,
  `discover = false`.

## Open questions

1. Discovery for LoadBalancer Services with TLS on a known port, or only
   Ingress and Gateway?
2. Probe intermediate certificates too (an expiring intermediate breaks
   every leaf under it)?

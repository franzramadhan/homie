# 0018: Certificate expiry

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-24 | #69 | v0.8.0 |

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

The served chain is the source of truth: Homie connects to a hostname,
completes or fails the TLS handshake, and reads every certificate in the
chain. Anything that answers TLS on a hostname is covered the same way
(Ingress, Gateway API, a LoadBalancer Service, a VM, an appliance).
Issuer-specific status is an optional extra.

- **Capture.** Homie's rustls config gets a `ServerCertVerifier` that
  records what the server sent, then verifies it. The record is kept even
  when verification fails, so an expired or untrusted certificate is still
  shown, with the verification error as the reason. Parsed with
  `x509-cert`: subject, SANs, issuer, `notBefore`, `notAfter`.
- **Whole chain.** Verification goes through `webpki::EndEntityCert::verify_for_usage`,
  which returns the path it built (`VerifiedPath`): the leaf, the
  intermediates it used and the trust anchor. A trust anchor carries no
  validity dates, so Homie maps it back to the full root certificate it
  came from (the bundled Mozilla roots in `webpki-root-certs` or the
  `ca_file`, matched by subject and public key) and reads that root's
  `notAfter`. The row shows the earliest expiry in the chain and which
  certificate it is ("intermediate R11 expires in 20 days"). Rows expand to
  the full chain.
- **Chain problems.** A server that omits an intermediate fails path
  building: "incomplete chain" (red), with the issuer name that was
  missing. Intermediates the server sent but the path did not need are
  listed as "unused" (no status). With `tls_verify = false` there is no
  path: only the certificates the server sent are shown, and the root is
  "unverified".
- **Targets**, three kinds, all read the same way:
  - **Integration endpoints**: every HTTPS URL Homie already uses (Proxmox
    nodes, OpenBao, Grafana, healthchecks, ...). Recorded on the normal
    requests, no extra connections.
  - **Listed targets**: `[[certificates.targets]]` with `host` (the name
    sent as SNI and checked against the SANs), optional `port` (443) and
    `address` (connect here instead of resolving `host`, like
    `tls_server_name`). For anything else on the network.
  - **Discovered targets** (opt-in per cluster). A source only yields
    hostnames; the check stays the generic one. Sources are picked in
    `sources`, so a lab turns on what it runs:
    - `ingress`: `spec.tls[].hosts`.
    - `gateway`: Gateway API listeners with `protocol: HTTPS` or `TLS`
      (`spec.listeners[].hostname`); a wildcard listener contributes the
      hostnames of its attached HTTPRoutes and TLSRoutes.
    - `annotations`: hostnames from annotations on Services (LoadBalancer
      and any other type), Ingresses and Gateways. The keys are
      configurable; the default is external-dns'
      `external-dns.alpha.kubernetes.io/hostname` (comma-separated, as
      external-dns reads it). A key like `homie.dev/tls-hosts` lets a lab
      mark anything by hand. The port comes from the Service's port named
      `https` or `tls`, else 443.
  - Each hostname is resolved and probed like a listed target.
    `discover_address` (optional) connects to one address instead, with
    each hostname as SNI, for clusters whose pods cannot reach the public
    path. No Secrets are read. A hostname found by several sources is
    probed once and lists all of them as its source.
  - In the code a source is one function from the cluster to a list of
    `(host, port, source)`; a new convention is a new function and a new
    `sources` value, nothing else changes.
- A probe is a handshake only (no HTTP request), every 6 h per target, 5 s
  timeout, at most 4 at a time. A failed connection keeps the last known
  certificate and shows "unreachable".
- **Status**: amber when `notAfter` is within `warn_days` (default 14), the
  certificate is not yet valid, or the name does not match; red within 3
  days, expired, an incomplete chain, or not trusted by the configured
  roots. Every row shows the words ("expires in 9 days", "expired 2 days
  ago", "name mismatch"). Thresholds per place in the chain, by who can act
  on them:

  | Certificate | Tone |
  |---|---|
  | Leaf, intermediate | amber within `warn_days` (14), red within 3 days or expired |
  | Private root (from any `ca_file`) | amber within `root_warn_days` (90): rotating a root means redistributing it to every client |
  | Public root (bundled `webpki-root-certs`) | neutral, date shown, never changes the tone |

  A public root is Homie's own trust store, replaced by a Homie upgrade;
  certificate authorities move to new roots years before the old ones
  expire. A chain the server sends through an old cross-signed root (the
  2021 DST Root CA X3 case) still shows, as an expiring intermediate.
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
| `certificates.warn_days` | `14` | amber threshold for leaf and intermediate certificates |
| `certificates.root_warn_days` | `90` | amber threshold for private roots |
| `certificates.interval` | `"6h"` | probe period for listed and discovered targets |
| `certificates.endpoints` | `true` | record integration endpoint certificates |
| `certificates.targets[].host` | - | name sent as SNI and checked |
| `certificates.targets[].port` | `443` | |
| `certificates.targets[].address` | `host` | connect here instead (IP or name) |
| `certificates.targets[].ca_file` | none | extra roots for this target |
| `kubernetes.<name>.certificates.discover` | `false` | probe discovered hostnames |
| `kubernetes.<name>.certificates.sources` | `["ingress", "gateway", "annotations"]` | hostname sources to use |
| `kubernetes.<name>.certificates.annotations` | `["external-dns.alpha.kubernetes.io/hostname"]` | annotation keys read by the `annotations` source |
| `kubernetes.<name>.certificates.discover_address` | none | connect here (`host:port`) instead of resolving each hostname |
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
| `WebPkiServerVerifier` from rustls | Does not return the path it built, so the intermediates used and the root are unknown |
| Discovering LoadBalancer Services by `status.loadBalancer.ingress` | That hostname is the cloud load balancer's name, which the served certificate rarely covers |
| One threshold for the whole chain | 14 days is too late to rotate a private root, and a public root's expiry is not the lab's to fix |
| A plugin interface (traits, dynamic loading) for sources | One function per source and a config list give the same choice without an extension API to keep stable |

## Testing and rollout

- **Tests:** the recording verifier (valid, expired, untrusted, name
  mismatch, not yet valid) against local rustls servers with generated
  certificates; chain cases (expiring intermediate, expiring private root
  from `ca_file`, missing intermediate, extra unused intermediate);
  anchor-to-root mapping; threshold mapping per chain position (public
  root never sets the tone, private root at `root_warn_days`); hostname discovery from
  Ingress, Gateway listener, wildcard-plus-route and annotation fixtures
  (comma-separated values, custom keys, port selection, a host found by
  two sources probed once); the cert-manager "renewal failing" merge; role hiding;
  unreachable keeps the last certificate.
- **Rollout:** integration endpoints on by default, nothing to configure.
  Listed and discovered targets need config and egress in network
  policies. Discovery needs `get, list` on the kinds its sources read
  (Ingresses, Gateways and routes, Services); cert-manager needs `get, list` on Certificates.
- **Off switch:** `certificates.endpoints = false`, no `targets`,
  `discover = false`.

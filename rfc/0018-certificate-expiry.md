# 0018: Certificate expiry

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-24 | - | - |

> Show when TLS certificates expire: cert-manager Certificates in each
> cluster, and the certificate of every HTTPS endpoint Homie already talks to.

## Problem

An expired certificate breaks a lab quietly: a Proxmox node whose 90-day
certificate failed to renew, an Ingress whose ACME order is stuck, an
internal CA that nobody rotated. cert-manager reports a failed renewal only
in a condition nobody reads, and certificates outside cert-manager (Proxmox,
OpenBao, a self-hosted Grafana) have no status anywhere.

## Proposal

- **cert-manager**: through the Kubernetes integration,
  `certificates.cert-manager.io/v1`: name, namespace, DNS names, issuer,
  `status.notAfter`, `status.renewalTime` and the `Ready` condition with its
  message. A cluster without the CRD shows nothing (404 is not an error).
- **Endpoints**: every HTTPS request an integration makes already completes
  a TLS handshake. The HTTP client is built with `tls_info(true)`, and the
  leaf certificate (`reqwest::tls::TlsInfo::peer_certificate`) of the last
  successful request per integration endpoint is kept: subject, SANs,
  issuer, `notAfter`. No extra connections. Plain `http` endpoints have no
  row; `tls_verify = false` endpoints are listed and flagged as unverified.
- **Status**: amber when `notAfter` is within `warn_days` (default 14) or
  `Ready` is False; red when expired, or within 3 days. Every row shows the
  words ("expires in 9 days", "renewal failing", "expired").
- **UI**: a "Certificates" card in the overview's Services section (counts
  by status, the soonest expiry), and a table on the Infrastructure page:
  name, source (`<cluster>/<namespace>/<name>` or the integration),
  names, issuer, expires, status. cert-manager rows deep-link to the
  Kubernetes UI template (RFC 0007).
- **Roles**: cert-manager rows are limited to the role's namespaces.
  Endpoint rows are never shown to a role (hostnames and addresses, see the
  "roles never see IP addresses" fix).
- Named queries `certs/<cluster>` (cached 5 min, refreshed by the
  Certificate watch from RFC 0009) and `certs-endpoints` (from memory, kept
  in the snapshot so a lab in quiet hours still shows its last known state).

### Configuration

| Key | Default | |
|---|---|---|
| `certificates.warn_days` | `14` | amber threshold |
| `certificates.endpoints` | `true` | record endpoint certificates; `false` turns the rows off |
| `kubernetes.<name>.cert_manager` | `true` | read Certificates in this cluster |

### Logs and metrics

| | |
|---|---|
| `msg="query" source="certs/<cluster>"` | as every query |

## Alternatives

| Option | Why not |
|---|---|
| A separate probe per endpoint | Duplicates connections Homie already makes |
| Hand-written DER walk for `notAfter` | Parsing untrusted bytes; use `x509-cert` (RustCrypto, `der` is already in the tree) |
| `x509-parser` | Pulls `nom` and more for one field |
| Reading `tls.crt` from Secrets | Needs Secret read access; Certificate status already has `notAfter` |
| Blackbox exporter + Grafana alert | Works, but each endpoint is configured twice and labs without Prometheus get nothing |

## Testing and rollout

- **Tests:** `notAfter` and SANs from fixture certificates (PEM in
  `tests/`), threshold mapping (ok, amber, red, expired), CRD absent,
  `Ready` False with a message, role namespace filtering, endpoint rows
  hidden from roles.
- **Rollout:** on by default; nothing to configure. The Helm chart and
  `deploy/kubernetes` ClusterRole get `get, list, watch` on
  `certificates.cert-manager.io` (`forward_token` users need it in their
  own RBAC).
- **Off switch:** `certificates.endpoints = false`,
  `kubernetes.<name>.cert_manager = false`.

## Open questions

1. Also read `CertificateRequest` and `Order` failures, or is the
   `Ready` message enough?

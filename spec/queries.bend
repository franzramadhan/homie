# A model of `allows` and `page_of` in src/roles.rs (RFC 0016): which named
# queries a role may run. The Rust test `roles::tests::allows_matches_spec`
# keeps the two in step.
import Base

type Query is Data:
  Pve{}
  SparkPve{}
  K8s{}
  SparkK8s{}
  Flux{}
  Alerts{}
  Security{}
  Tailscale{}
  Github{}
  Openbao{}
  Cnpg{}
  Healthchecks{}
  Helm{}
  Argocd{}
  Other{}  # any other kind

type Page is Data:
  Infra{}
  Workloads{}
  Gitops{}
  AlertsPage{}
  SecurityPage{}
  TailscalePage{}
  Workflows{}
  Services{}
  Outage{}
  NoPage{}  # not for roles at all

# the pages a role has
type Pages is Data:
  Pages{infra: Bool, workloads: Bool, gitops: Bool, alerts: Bool, security: Bool, tailscale: Bool, workflows: Bool, services: Bool, outage: Bool}

def page_of(k: Query) -> Page:
  match k:
    case Pve{}:
      Infra{}
    case SparkPve{}:
      Infra{}
    case K8s{}:
      Workloads{}
    case SparkK8s{}:
      Workloads{}
    case Flux{}:
      Gitops{}
    case Alerts{}:
      AlertsPage{}
    case Security{}:
      SecurityPage{}
    case Tailscale{}:
      TailscalePage{}
    case Github{}:
      Workflows{}
    case Openbao{}:
      Services{}
    case Cnpg{}:
      Services{}
    case Healthchecks{}:
      Outage{}
    case Helm{}:
      NoPage{}
    case Argocd{}:
      NoPage{}
    case Other{}:
      NoPage{}

def has(r: Pages, p: Page) -> Bool:
  match r:
    case Pages{i, w, g, a, s, t, wf, sv, o}:
      match p:
        case Infra{}:
          i
        case Workloads{}:
          w
        case Gitops{}:
          g
        case AlertsPage{}:
          a
        case SecurityPage{}:
          s
        case TailscalePage{}:
          t
        case Workflows{}:
          wf
        case Services{}:
          sv
        case Outage{}:
          o
        case NoPage{}:
          False{}

def allows(r: Pages, k: Query) -> Bool:
  has(r, page_of(k))

# A model of `role_route` in src/server/guard.rs (RFC 0016, RFC 0017): which
# routes a session with a role may use. Paths are classified into `Route`; the
# Rust test `guard::tests::role_routes_match_spec` keeps the two in step.
import Base

type Method is Data:
  Get{}
  Head{}
  Post{}
  Other{}

type Route is Data:
  Meta{}         # GET /api/meta
  Stream{}       # GET /api/stream
  Query{}        # GET /api/q/*
  Logout{}       # POST /auth/logout
  Logs{}         # POST /api/logs
  GhLogin{}      # GET /github/login
  GhCallback{}   # GET /github/callback
  GhRepos{}      # GET /api/github/repos
  GhWorkflows{}  # GET /api/github/workflows
  GhWatch{}      # POST /api/github/watch
  Act{}          # /api/act/*: power, reconcile, refresh
  Unknown{}      # anything else

# what of a role the routes depend on: whether it has the logs and the
# workflows pages
type Role is Data:
  Role{logs: Bool, workflows: Bool}

def Role.logs(role: Role) -> Bool:
  match role:
    case Role{l, _}:
      l

def Role.workflows(role: Role) -> Bool:
  match role:
    case Role{_, w}:
      w

def is_read(m: Method) -> Bool:
  match m:
    case Get{}:
      True{}
    case Head{}:
      True{}
    case Post{}:
      False{}
    case Other{}:
      False{}

def is_post(m: Method) -> Bool:
  match m:
    case Get{}:
      False{}
    case Head{}:
      False{}
    case Post{}:
      True{}
    case Other{}:
      False{}

# the GitHub picker opens for a role only with per-session watches (`ghs`,
# store = "session") and the workflows page
def picker(role: Role, ghs: Bool) -> Bool:
  Bool.and(ghs, Role.workflows(role))

def is_picker(r: Route) -> Bool:
  match r:
    case GhLogin{}:
      True{}
    case GhCallback{}:
      True{}
    case GhRepos{}:
      True{}
    case GhWorkflows{}:
      True{}
    case GhWatch{}:
      True{}
    case _:
      False{}

# The method is checked first on every route, as the Rust does.
def role_route(role: Role, m: Method, r: Route, ghs: Bool) -> Bool:
  match r:
    case Meta{}:
      is_read(m)
    case Stream{}:
      is_read(m)
    case Query{}:
      is_read(m)
    case Logout{}:
      is_post(m)
    case Logs{}:
      Bool.and(is_post(m), Role.logs(role))
    case GhLogin{}:
      Bool.and(is_read(m), picker(role, ghs))
    case GhCallback{}:
      Bool.and(is_read(m), picker(role, ghs))
    case GhRepos{}:
      Bool.and(is_read(m), picker(role, ghs))
    case GhWorkflows{}:
      Bool.and(is_read(m), picker(role, ghs))
    case GhWatch{}:
      Bool.and(is_post(m), picker(role, ghs))
    case Act{}:
      False{}
    case Unknown{}:
      False{}

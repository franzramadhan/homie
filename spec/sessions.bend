# Models of session handling in src/server/guard.rs (`session`: the role of a
# logged-in user) and src/auth/mod.rs (`demo_login`: the demo session cap).
# The Rust tests `server::tests::session_access_matches_spec` and
# `auth::tests::demo_cap_matches_spec` keep them in step.
import Base

# the role a session names
type RoleRef is Data:
  NoRole{}  # none: full access
  Known{}   # a role in the config
  Gone{}    # a role no longer in the config

type Access is Data:
  Full{}
  Scoped{}
  Refused{}

# with a login method configured: no session is refused, a role that left the
# config is refused, never widened to full access
def access(logged_in: Bool, r: RoleRef) -> Access:
  match logged_in:
    case False{}:
      Refused{}
    case True{}:
      match r:
        case NoRole{}:
          Full{}
        case Known{}:
          Scoped{}
        case Gone{}:
          Refused{}

# demo sessions after one more demo login, with `n` before and a cap of
# 1 + `m`: the oldest go until `m` are left, then the new one
def demo_after(n: Nat, m: Nat) -> Nat:
  1n+Nat.min(n, m)

# the role of a new demo session: always the configured demo role
def demo_role() -> RoleRef:
  Known{}

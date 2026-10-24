# A model of `take` and the Origin check in src/server/console.rs (RFC 0023):
# when a console id opens its WebSocket. The Rust test
# `console::tests::take_matches_spec` keeps the two in step.
import Base

# the Origin of the WebSocket request; a browser always sends one
type Origin is Data:
  Absent{}
  Same{}
  Foreign{}

# `take` removes the id first, then hands the console over only when the id
# was there, is younger than its TTL, and comes from the session and the
# Proxmox that opened it
def granted(present: Bool, fresh: Bool, same_session: Bool, same_pve: Bool) -> Bool:
  Bool.and(present, Bool.and(fresh, Bool.and(same_session, same_pve)))

# whether the id is still there after `take`, granted or not
def kept(present: Bool, fresh: Bool, same_session: Bool, same_pve: Bool) -> Bool:
  False{}

def origin_ok(o: Origin) -> Bool:
  match o:
    case Absent{}:
      False{}
    case Same{}:
      True{}
    case Foreign{}:
      False{}

# A model of the write and Host checks in src/server/guard.rs: `write_denied`
# with `cross_origin`, and `host_allowed`. Headers are classified into
# `Origin` and `Host`; the Rust tests `guard::tests::hosts_match_spec` and
# `server::tests::writes_match_spec` keep the two in step.
import Base

# the Origin header of a request
type Origin is Data:
  Absent{}   # no Origin header
  Same{}     # this site: public_url's origin with login, an allowed host without
  Foreign{}  # anything else, unparsable included

# the Host header, without login
type Host is Data:
  Loopback{}   # localhost, 127.0.0.1, [::1], any port
  Public{}     # the host of public_url
  Elsewhere{}  # anything else

def foreign(o: Origin) -> Bool:
  match o:
    case Absent{}:
      False{}
    case Same{}:
      False{}
    case Foreign{}:
      True{}

# a write passes with the X-Homie header and an Origin that is not foreign
def write_ok(x_homie: Bool, o: Origin) -> Bool:
  Bool.and(x_homie, Bool.not(foreign(o)))

# without login: loopback names always; otherwise the public_url host, or,
# with no public_url set, whatever allow_unauthenticated says
def host_allowed(h: Host, public_url: Bool, allow_unauthenticated: Bool) -> Bool:
  match h:
    case Loopback{}:
      True{}
    case Public{}:
      match public_url:
        case True{}:
          True{}
        case False{}:
          allow_unauthenticated
    case Elsewhere{}:
      match public_url:
        case True{}:
          False{}
        case False{}:
          allow_unauthenticated

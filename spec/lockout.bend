# A model of `Local::verify` in src/auth/local.rs: the local login lockout.
# The Rust test `auth::local::tests::lockout_matches_spec` keeps the two in
# step.
import Base

# failures in the current window, and whether a lockout is running
type Throttle is Data:
  Throttle{failures: Nat, locked: Bool}

def max_failures() -> Nat:
  10n

# a login with the right credentials (`ok`) succeeds only outside a lockout
def granted(t: Throttle, ok: Bool) -> Bool:
  match t:
    case Throttle{f, locked}:
      Bool.and(Bool.not(locked), ok)

def counted(+n: Nat) -> Throttle:
  Throttle{n, Nat.is_ge(n, max_failures())}

def failed(f: Nat, fresh: Bool) -> Throttle:
  match fresh:
    case True{}:
      counted(1n+f)
    case False{}:
      counted(1n)

# the throttle after an attempt; `fresh`: the window of the first failure is
# still open
def after(t: Throttle, ok: Bool, fresh: Bool) -> Throttle:
  match t:
    case Throttle{f, locked}:
      match locked:
        case True{}:
          Throttle{f, True{}}
        case False{}:
          match ok:
            case True{}:
              Throttle{0n, False{}}
            case False{}:
              failed(f, fresh)

def locked(t: Throttle) -> Bool:
  match t:
    case Throttle{f, l}:
      l

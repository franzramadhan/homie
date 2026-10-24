# A model of the row filters in `filter` (src/roles.rs, RFC 0016): what a
# role sees of Kubernetes rows, Flux objects and Proxmox guests. Three
# namespaces and three tags stand for all of them; `a`, `b`, `c` say which
# the role has. The Rust test `roles::tests::filter_matches_spec` keeps the
# two in step.
import Base

type Name is Data:
  N1{}
  N2{}
  N3{}

def ok(n: Name, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match n:
    case N1{}:
      a
    case N2{}:
      b
    case N3{}:
      c

# every name is the role's
def all_ok(xs: +List<Name>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match xs:
    case Nil{}:
      True{}
    case Con{h, t}:
      Bool.and(ok(h, a, b, c), all_ok(t, a, b, c))

# some name is the role's
def any_ok(xs: +List<Name>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match xs:
    case Nil{}:
      False{}
    case Con{h, t}:
      Bool.or(ok(h, a, b, c), any_ok(t, a, b, c))

def keep_row(keep: Bool, +h: Name, rest: +List<Name>) -> +List<Name>:
  match keep:
    case True{}:
      Con{h, rest}
    case False{}:
      rest

# Kubernetes rows (workloads, unhealthy pods, events), by their namespace:
# kept when the namespace is the role's
def rows(xs: +List<Name>, +a: Bool, +b: Bool, +c: Bool) -> +List<Name>:
  match xs:
    case Nil{}:
      Nil{}
    case Con{+h, t}:
      keep_row(ok(h, a, b, c), h, rows(t, a, b, c))

def keep_list(keep: Bool, +h: +List<Name>, rest: +List<+List<Name>>) -> +List<+List<Name>>:
  match keep:
    case True{}:
      Con{h, rest}
    case False{}:
      rest

# a Flux object, by the namespaces it deploys to: shown when it deploys
# somewhere and only to the role's namespaces
def flux_ok(ns: +List<Name>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match ns:
    case Nil{}:
      False{}
    case Con{h, t}:
      Bool.and(ok(h, a, b, c), all_ok(t, a, b, c))

def flux(xs: +List<+List<Name>>, +a: Bool, +b: Bool, +c: Bool) -> +List<+List<Name>>:
  match xs:
    case Nil{}:
      Nil{}
    case Con{+h, t}:
      keep_list(flux_ok(h, a, b, c), h, flux(t, a, b, c))

def all_flux_ok(xs: +List<+List<Name>>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match xs:
    case Nil{}:
      True{}
    case Con{h, t}:
      Bool.and(flux_ok(h, a, b, c), all_flux_ok(t, a, b, c))

# a Proxmox guest, by its tags: shown when one of them is the role's
def guests(xs: +List<+List<Name>>, +a: Bool, +b: Bool, +c: Bool) -> +List<+List<Name>>:
  match xs:
    case Nil{}:
      Nil{}
    case Con{+h, t}:
      keep_list(any_ok(h, a, b, c), h, guests(t, a, b, c))

def all_tagged(xs: +List<+List<Name>>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match xs:
    case Nil{}:
      True{}
    case Con{h, t}:
      Bool.and(any_ok(h, a, b, c), all_tagged(t, a, b, c))

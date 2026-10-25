# A model of the config drift check `config::tests::config_md_has_no_drift`
# (src/config.rs): the keys of src/config.rs that CONFIG.md does not name.
# Three keys stand for all of them; `a`, `b`, `c` say which CONFIG.md
# names. The Rust test `config::tests::drift_matches_spec` keeps the two in
# step.
import Base

type Key is Data:
  K1{}
  K2{}
  K3{}

def documented(k: Key, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match k:
    case K1{}:
      a
    case K2{}:
      b
    case K3{}:
      c

def drop_if(gone: Bool, +h: Key, rest: +List<Key>) -> +List<Key>:
  match gone:
    case True{}:
      rest
    case False{}:
      Con{h, rest}

# the keys CONFIG.md does not name, in source order
def missing(xs: +List<Key>, +a: Bool, +b: Bool, +c: Bool) -> +List<Key>:
  match xs:
    case Nil{}:
      Nil{}
    case Con{+h, t}:
      drop_if(documented(h, a, b, c), h, missing(t, a, b, c))

def is_empty(xs: +List<Key>) -> Bool:
  match xs:
    case Nil{}:
      True{}
    case Con{h, t}:
      False{}

# the test passes when nothing is missing
def passes(xs: +List<Key>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  is_empty(missing(xs, a, b, c))

# every key is named in CONFIG.md
def all_documented(xs: +List<Key>, +a: Bool, +b: Bool, +c: Bool) -> Bool:
  match xs:
    case Nil{}:
      True{}
    case Con{h, t}:
      Bool.and(documented(h, a, b, c), all_documented(t, a, b, c))

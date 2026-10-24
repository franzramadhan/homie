# Proofs of LAWS.bend, checked by `bend PROOF.bend` before every commit.
import Base
import ./LAWS.bend as Laws
import ./spec/roles.bend as R
import ./spec/guard.bend as G
import ./spec/console.bend as C
import ./spec/power.bend as P
import ./spec/queries.bend as Q
import ./spec/sessions.bend as S
import ./spec/lockout.bend as L
import ./spec/filter.bend as F

# PROOF: role_route answers False for Act before it looks at anything else.
def Laws.no_actions(role, m, ghs):
  {==}

# PROOF: the same for Unknown.
def Laws.no_unknown_routes(role, m, ghs):
  {==}

# PROOF: case analysis on the route, then on the method for picker routes: a
# read or a POST then asks picker(role, False), which is False at once.
def Laws.picker_needs_session(r, m, role):
  match r:
    case R.Meta{}:
      {==}
    case R.Stream{}:
      {==}
    case R.Query{}:
      {==}
    case R.Logout{}:
      {==}
    case R.Logs{}:
      {==}
    case R.GhLogin{}:
      match m:
        case R.Get{}:
          {==}
        case R.Head{}:
          {==}
        case R.Post{}:
          {==}
        case R.Other{}:
          {==}
    case R.GhCallback{}:
      match m:
        case R.Get{}:
          {==}
        case R.Head{}:
          {==}
        case R.Post{}:
          {==}
        case R.Other{}:
          {==}
    case R.GhRepos{}:
      match m:
        case R.Get{}:
          {==}
        case R.Head{}:
          {==}
        case R.Post{}:
          {==}
        case R.Other{}:
          {==}
    case R.GhWorkflows{}:
      match m:
        case R.Get{}:
          {==}
        case R.Head{}:
          {==}
        case R.Post{}:
          {==}
        case R.Other{}:
          {==}
    case R.GhWatch{}:
      match m:
        case R.Get{}:
          {==}
        case R.Head{}:
          {==}
        case R.Post{}:
          {==}
        case R.Other{}:
          {==}
    case R.Act{}:
      {==}
    case R.Unknown{}:
      {==}

# PROOF: case analysis on the route, the method and the store: picker() is
# Bool.and(ghs, False) for this role, False either way.
def Laws.picker_needs_workflows_page(r, m, ghs, logs):
  match r:
    case R.Meta{}:
      {==}
    case R.Stream{}:
      {==}
    case R.Query{}:
      {==}
    case R.Logout{}:
      {==}
    case R.Logs{}:
      {==}
    case R.GhLogin{}:
      match m:
        case R.Get{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Head{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Post{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Other{}:
          {==}
    case R.GhCallback{}:
      match m:
        case R.Get{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Head{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Post{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Other{}:
          {==}
    case R.GhRepos{}:
      match m:
        case R.Get{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Head{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Post{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Other{}:
          {==}
    case R.GhWorkflows{}:
      match m:
        case R.Get{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Head{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Post{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Other{}:
          {==}
    case R.GhWatch{}:
      match m:
        case R.Get{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Head{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Post{}:
          match ghs:
            case False{}:
              {==}
            case True{}:
              {==}
        case R.Other{}:
          {==}
    case R.Act{}:
      {==}
    case R.Unknown{}:
      {==}

# PROOF: case analysis on the route: every route checks the method first, and
# Other is neither a read nor POST.
def Laws.other_methods_refused(r, role, ghs):
  match r:
    case R.Meta{}:
      {==}
    case R.Stream{}:
      {==}
    case R.Query{}:
      {==}
    case R.Logout{}:
      {==}
    case R.Logs{}:
      {==}
    case R.GhLogin{}:
      {==}
    case R.GhCallback{}:
      {==}
    case R.GhRepos{}:
      {==}
    case R.GhWorkflows{}:
      {==}
    case R.GhWatch{}:
      {==}
    case R.Act{}:
      {==}
    case R.Unknown{}:
      {==}

# PROOF: Bool.and(False, _) is False at once.
def Laws.write_needs_header(o):
  {==}

# PROOF: case analysis on the header: foreign(Foreign) is True, so the second
# operand is False.
def Laws.write_refuses_foreign_origin(x):
  match x:
    case True{}:
      {==}
    case False{}:
      {==}

# PROOF: case analysis on public_url: with it, Elsewhere is False; without it,
# both sides are allow_unauthenticated.
def Laws.foreign_host_refused(p, a):
  match p:
    case True{}:
      {==}
    case False{}:
      match a:
        case True{}:
          {==}
        case False{}:
          {==}

# PROOF: kept is False, so granted starts with Bool.and(False, _).
def Laws.console_id_single_use(p, f, s, v, f2, s2, v2):
  {==}

# PROOF: case analysis on present and fresh until the session operand, False.
def Laws.console_needs_session(p, f, v):
  match p:
    case False{}:
      {==}
    case True{}:
      match f:
        case False{}:
          {==}
        case True{}:
          {==}

# PROOF: case analysis on present, fresh and session down to the last
# operand, False.
def Laws.console_needs_same_pve(p, f, s):
  match p:
    case False{}:
      {==}
    case True{}:
      match f:
        case False{}:
          {==}
        case True{}:
          match s:
            case False{}:
              {==}
            case True{}:
              {==}

# PROOF: case analysis on present, then the fresh operand is False.
def Laws.console_expires(p, s, v):
  match p:
    case False{}:
      {==}
    case True{}:
      {==}

# PROOF: both sides compute to False.
def Laws.console_needs_same_origin():
  {==}

# PROOF: Stop answers `typed`, False here.
def Laws.stop_needs_name(p):
  {==}

# PROOF: case analysis on the action: Bool.or(False, Bool.not(True)) for
# the others, `typed` (False) for Stop.
def Laws.protected_needs_name(a):
  match a:
    case P.Start{}:
      {==}
    case P.Shutdown{}:
      {==}
    case P.Reboot{}:
      {==}
    case P.Stop{}:
      {==}

# PROOF: open the role: all three map to NoPage, which has() answers False.
def Laws.queries_never_for_roles(r):
  match r:
    case Q.Pages{i, w, g, a, s, t, wf, sv, o}:
      {==}

# PROOF: case analysis on the query: every page field is False.
def Laws.no_pages_no_queries(k):
  match k:
    case Q.Pve{}:
      {==}
    case Q.SparkPve{}:
      {==}
    case Q.K8s{}:
      {==}
    case Q.SparkK8s{}:
      {==}
    case Q.Flux{}:
      {==}
    case Q.Alerts{}:
      {==}
    case Q.Security{}:
      {==}
    case Q.Tailscale{}:
      {==}
    case Q.Github{}:
      {==}
    case Q.Openbao{}:
      {==}
    case Q.Cnpg{}:
      {==}
    case Q.Healthchecks{}:
      {==}
    case Q.Helm{}:
      {==}
    case Q.Argocd{}:
      {==}
    case Q.Other{}:
      {==}

# PROOF: no session is Refused before the role is looked at.
def Laws.no_session_refused(r):
  {==}

# PROOF: case analysis on the session: Refused either way.
def Laws.gone_role_refused(l):
  match l:
    case False{}:
      {==}
    case True{}:
      {==}

# PROOF: demo_role() is Known, which access() scopes.
def Laws.demo_is_scoped():
  {==}

# PROOF: Nat.is_le(1 + min(n, m), 1 + m) is Nat.is_le(min(n, m), m): induction
# on n and m.
law min_le:
  for n: Nat
  for m: Nat
  {Nat.is_le(Nat.min(n, m), m) == True{} : Bool}

def min_le(n, m):
  match n m:
    case 0n 0n:
      {==}
    case 0n 1n+q:
      {==}
    case 1n+p 0n:
      {==}
    case 1n+p 1n+q:
      min_le(p, q)

def Laws.demo_cap(n, m):
  min_le(n, m)

# PROOF: granted() is Bool.and(Bool.not(True), _), False at once.
def Laws.lockout_refuses_right_password(f, ok):
  {==}

# PROOF: after() keeps a running lockout before it looks at the attempt.
def Laws.lockout_holds(f, ok, w):
  {==}

# PROOF: case analysis on k: 9 + k + 1 failures compare with 10 as k with 0,
# which is EQ or GT.
def Laws.tenth_failure_locks(k):
  match k:
    case 0n:
      {==}
    case 1n+p:
      {==}

# PROOF: outside a lockout, the right password resets the throttle.
def Laws.success_resets(f, w):
  {==}

# The filters keep a row only when its test said True. Each step lemma takes
# that test's answer `k` with the equation that it is the answer: when kept,
# the equation turns the test into True and the rest is the induction
# hypothesis; when dropped, the hypothesis is the goal.
law row_step:
  for +h: F.Name
  for k: Bool
  for +a: Bool
  for +b: Bool
  for +c: Bool
  for rest: +List<F.Name>
  {F.all_ok(rest, a, b, c) == True{} : Bool} -> {F.ok(h, a, b, c) == k : Bool} -> {F.all_ok(F.keep_row(k, h, rest), a, b, c) == True{} : Bool}

def row_step(h, k, a, b, c, rest):
  match k:
    case True{}:
      ih => e =>
        %Equal.sym(Bool, F.ok(h, a, b, c), True{}, e) : {Bool.and(_, F.all_ok(rest, a, b, c)) == True{} : Bool}
        ih
    case False{}:
      ih => e => ih

# PROOF: induction on the rows, with row_step.
def Laws.role_rows_in_namespaces(xs, a, b, c):
  match xs:
    case Nil{}:
      {==}
    case Con{+h, +t}:
      row_step(h, F.ok(h, a, b, c), a, b, c, F.rows(t, a, b, c))(Laws.role_rows_in_namespaces(t, a, b, c))({==})

law flux_step:
  for +h: +List<F.Name>
  for k: Bool
  for +a: Bool
  for +b: Bool
  for +c: Bool
  for rest: +List<+List<F.Name>>
  {F.all_flux_ok(rest, a, b, c) == True{} : Bool} -> {F.flux_ok(h, a, b, c) == k : Bool} -> {F.all_flux_ok(F.keep_list(k, h, rest), a, b, c) == True{} : Bool}

def flux_step(h, k, a, b, c, rest):
  match k:
    case True{}:
      ih => e =>
        %Equal.sym(Bool, F.flux_ok(h, a, b, c), True{}, e) : {Bool.and(_, F.all_flux_ok(rest, a, b, c)) == True{} : Bool}
        ih
    case False{}:
      ih => e => ih

# PROOF: induction on the objects, with flux_step.
def Laws.role_flux_in_namespaces(xs, a, b, c):
  match xs:
    case Nil{}:
      {==}
    case Con{+h, +t}:
      flux_step(h, F.flux_ok(h, a, b, c), a, b, c, F.flux(t, a, b, c))(Laws.role_flux_in_namespaces(t, a, b, c))({==})

law guest_step:
  for +h: +List<F.Name>
  for k: Bool
  for +a: Bool
  for +b: Bool
  for +c: Bool
  for rest: +List<+List<F.Name>>
  {F.all_tagged(rest, a, b, c) == True{} : Bool} -> {F.any_ok(h, a, b, c) == k : Bool} -> {F.all_tagged(F.keep_list(k, h, rest), a, b, c) == True{} : Bool}

def guest_step(h, k, a, b, c, rest):
  match k:
    case True{}:
      ih => e =>
        %Equal.sym(Bool, F.any_ok(h, a, b, c), True{}, e) : {Bool.and(_, F.all_tagged(rest, a, b, c)) == True{} : Bool}
        ih
    case False{}:
      ih => e => ih

# PROOF: induction on the guests, with guest_step.
def Laws.role_guests_tagged(xs, a, b, c):
  match xs:
    case Nil{}:
      {==}
    case Con{+h, +t}:
      guest_step(h, F.any_ok(h, a, b, c), a, b, c, F.guests(t, a, b, c))(Laws.role_guests_tagged(t, a, b, c))({==})

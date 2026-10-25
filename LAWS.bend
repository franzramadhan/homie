# Laws of Homie: claims the checker must prove (`bend PROOF.bend`). The
# maintainer owns this file; PROOF.bend holds the proofs. See CLAUDE.md.
import Base
import ./spec/roles.bend as R
import ./spec/guard.bend as G
import ./spec/console.bend as C
import ./spec/power.bend as P
import ./spec/queries.bend as Q
import ./spec/sessions.bend as S
import ./spec/lockout.bend as L
import ./spec/filter.bend as F
import ./spec/drift.bend as D

# LAW: a role never reaches an action route (power, reconcile, refresh), with
# any method, any role and either GitHub store.
law no_actions:
  for role: R.Role
  for m: R.Method
  for ghs: Bool
  {R.role_route(role, m, R.Act{}, ghs) == False{} : Bool}

# LAW: a role never reaches a route the model does not know.
law no_unknown_routes:
  for role: R.Role
  for m: R.Method
  for ghs: Bool
  {R.role_route(role, m, R.Unknown{}, ghs) == False{} : Bool}

# LAW: without per-session GitHub watches, a role reaches no GitHub picker
# route (sign-in, callback, repositories, workflows, save).
law picker_needs_session:
  for r: R.Route
  for m: R.Method
  for role: R.Role
  {Bool.and(R.is_picker(r), R.role_route(role, m, r, False{})) == False{} : Bool}

# LAW: a role without the workflows page reaches no GitHub picker route, in
# any GitHub store.
law picker_needs_workflows_page:
  for r: R.Route
  for m: R.Method
  for ghs: Bool
  for logs: Bool
  {Bool.and(R.is_picker(r), R.role_route(R.Role{logs, False{}}, m, r, ghs)) == False{} : Bool}

# LAW: a role reaches nothing with a method other than GET, HEAD or POST.
law other_methods_refused:
  for r: R.Route
  for role: R.Role
  for ghs: Bool
  {R.role_route(role, R.Other{}, r, ghs) == False{} : Bool}

# LAW: a write without the X-Homie header is refused, whatever its Origin.
law write_needs_header:
  for o: G.Origin
  {G.write_ok(False{}, o) == False{} : Bool}

# LAW: a write with an Origin from another site is refused.
law write_refuses_foreign_origin:
  for x: Bool
  {G.write_ok(x, G.Foreign{}) == False{} : Bool}

# LAW: without login, a Host other than a loopback name or the public_url host
# passes only when no public_url is set and allow_unauthenticated is on.
law foreign_host_refused:
  for p: Bool
  for a: Bool
  {G.host_allowed(G.Elsewhere{}, p, a) == Bool.and(Bool.not(p), a) : Bool}

# LAW: a console id opens at most one WebSocket: after any attempt, a second
# one is refused.
law console_id_single_use:
  for p: Bool
  for f: Bool
  for s: Bool
  for v: Bool
  for f2: Bool
  for s2: Bool
  for v2: Bool
  {C.granted(C.kept(p, f, s, v), f2, s2, v2) == False{} : Bool}

# LAW: a console id is refused from another session.
law console_needs_session:
  for p: Bool
  for f: Bool
  for v: Bool
  {C.granted(p, f, False{}, v) == False{} : Bool}

# LAW: a console id is refused on another Proxmox.
law console_needs_same_pve:
  for p: Bool
  for f: Bool
  for s: Bool
  {C.granted(p, f, s, False{}) == False{} : Bool}

# LAW: a console id is refused once its TTL has passed.
law console_expires:
  for p: Bool
  for s: Bool
  for v: Bool
  {C.granted(p, False{}, s, v) == False{} : Bool}

# LAW: a console WebSocket without an Origin, or with one from another site,
# is refused.
law console_needs_same_origin:
  {Bool.or(C.origin_ok(C.Absent{}), C.origin_ok(C.Foreign{})) == False{} : Bool}

# LAW: stop never runs without the guest's name typed.
law stop_needs_name:
  for p: Bool
  {P.proceeds(P.Stop{}, p, False{}) == False{} : Bool}

# LAW: no action runs on a protected guest without its name typed.
law protected_needs_name:
  for a: P.Action
  {P.proceeds(a, True{}, False{}) == False{} : Bool}

# LAW: Helm, Argo CD and unknown queries are never served to a role, whatever
# its pages.
law queries_never_for_roles:
  for r: Q.Pages
  {Bool.or(Q.allows(r, Q.Helm{}), Bool.or(Q.allows(r, Q.Argocd{}), Q.allows(r, Q.Other{}))) == False{} : Bool}

# LAW: a role without pages runs no query.
law no_pages_no_queries:
  for k: Q.Query
  {Q.allows(Q.Pages{False{}, False{}, False{}, False{}, False{}, False{}, False{}, False{}, False{}}, k) == False{} : Bool}

# LAW: with a login method, a request without a session is refused.
law no_session_refused:
  for r: S.RoleRef
  {S.access(False{}, r) == S.Refused{} : S.Access}

# LAW: a session whose role left the config is refused, never given full
# access.
law gone_role_refused:
  for l: Bool
  {S.access(l, S.Gone{}) == S.Refused{} : S.Access}

# LAW: a demo session is scoped to the demo role, never full access.
law demo_is_scoped:
  {S.access(True{}, S.demo_role()) == S.Scoped{} : S.Access}

# LAW: after a demo login there are at most max_sessions demo sessions
# (a cap of 1 + m, whatever n existed before).
law demo_cap:
  for n: Nat
  for m: Nat
  {Nat.is_le(S.demo_after(n, m), 1n+m) == True{} : Bool}

# LAW: during a lockout even the right password is refused.
law lockout_refuses_right_password:
  for f: Nat
  for ok: Bool
  {L.granted(L.Throttle{f, True{}}, ok) == False{} : Bool}

# LAW: an attempt during a lockout does not end it.
law lockout_holds:
  for f: Nat
  for ok: Bool
  for w: Bool
  {L.locked(L.after(L.Throttle{f, True{}}, ok, w)) == True{} : Bool}

# LAW: a failure in the window of nine or more earlier failures starts a
# lockout.
law tenth_failure_locks:
  for k: Nat
  {L.locked(L.after(L.Throttle{Nat.add(9n, k), False{}}, False{}, True{})) == True{} : Bool}

# LAW: a successful login outside a lockout clears the failures.
law success_resets:
  for f: Nat
  for w: Bool
  {L.after(L.Throttle{f, False{}}, True{}, w) == L.Throttle{0n, False{}} : L.Throttle}

# LAW: every Kubernetes row a role sees (workload, unhealthy pod, event) is in
# one of its namespaces.
law role_rows_in_namespaces:
  for xs: +List<F.Name>
  for +a: Bool
  for +b: Bool
  for +c: Bool
  {F.all_ok(F.rows(xs, a, b, c), a, b, c) == True{} : Bool}

# LAW: every Flux object a role sees deploys somewhere, and only to its
# namespaces.
law role_flux_in_namespaces:
  for xs: +List<+List<F.Name>>
  for +a: Bool
  for +b: Bool
  for +c: Bool
  {F.all_flux_ok(F.flux(xs, a, b, c), a, b, c) == True{} : Bool}

# LAW: every Proxmox guest a role sees carries one of its tags.
law role_guests_tagged:
  for xs: +List<+List<F.Name>>
  for +a: Bool
  for +b: Bool
  for +c: Bool
  {F.all_tagged(F.guests(xs, a, b, c), a, b, c) == True{} : Bool}

# LAW: the config drift check passes exactly when CONFIG.md names every key
# of src/config.rs: a new or renamed key cannot merge undocumented.
law config_documented:
  for xs: +List<D.Key>
  for +a: Bool
  for +b: Bool
  for +c: Bool
  {D.passes(xs, a, b, c) == D.all_documented(xs, a, b, c) : Bool}

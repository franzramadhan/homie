# Laws of Homie: claims the checker must prove (`bend PROOF.bend`). The
# maintainer owns this file; PROOF.bend holds the proofs. See CLAUDE.md.
import Base
import ./spec/roles.bend as R

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

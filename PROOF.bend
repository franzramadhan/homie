# Proofs of LAWS.bend, checked by `bend PROOF.bend` before every commit.
import Base
import ./LAWS.bend as Laws
import ./spec/roles.bend as R

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

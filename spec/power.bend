# A model of `confirmation_needed` in src/integrations/proxmox.rs: when a
# power action runs without the guest's name typed. The Rust test
# `proxmox::tests::confirmations_match_spec` keeps the two in step.
import Base

type Action is Data:
  Start{}
  Shutdown{}
  Reboot{}
  Stop{}

# `typed`: the confirmation equals the guest's name
def proceeds(a: Action, protected: Bool, typed: Bool) -> Bool:
  match a:
    case Start{}:
      Bool.or(typed, Bool.not(protected))
    case Shutdown{}:
      Bool.or(typed, Bool.not(protected))
    case Reboot{}:
      Bool.or(typed, Bool.not(protected))
    case Stop{}:
      typed

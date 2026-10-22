# 0010: Guest migration

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-22 | - | v0.2.0 |

> Migrate Proxmox VMs and containers between nodes from the Infrastructure
> page.

## Problem

Draining a node for maintenance means the Proxmox UI or `qm migrate`.

## Proposal

- Action `pve.guest.migrate` with `target` node: VMs online when running
  (`online=1`), containers with `restart=1`; offline otherwise.
- Validation: target online and not the current node; storage check from
  `GET /nodes/{node}/qemu/{vmid}/migrate` (preconditions: local disks,
  allowed nodes); local disks need `with-local-disks=1`, shown as a warning.
- Same rules as other actions: one task per guest, typed name for protected
  guests, task polling, action log.
- The token role needs `VM.Migrate`; without it the button is hidden.

## Alternatives

| Option | Why not |
|---|---|
| Node "evacuate" (all guests) | Bigger blast radius; single-guest first |

## Testing and rollout

- **Tests:** precondition mapping; online/offline/restart choice.
- **Rollout:** needs the role change; e2e migrates a sandbox VM between nodes.

## Open questions

1. Offer a node-level "migrate all" later?

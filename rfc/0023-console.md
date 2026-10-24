# 0023: Proxmox consoles

| Status | Date | PR | Release |
|---|---|---|---|
| draft | 2026-10-24 | #N | - |

> Open a guest's console from Homie: a link to the Proxmox console for every
> guest and node, and, when turned on, the guest console embedded in Homie
> (noVNC for VMs, xterm.js for containers), proxied by Homie.

## Problem

To reach a guest's screen the user opens the Proxmox UI, logs in, finds the
guest and opens its console: four steps from a page that already lists the
guest. RFC 0001 named consoles a later milestone; no RFC was written.

## Proposal

### 1. Console links (always on)

- Every guest row on `/infra` gets a **console** link, every node a
  **shell** link, to the Proxmox UI's own console page:
  - VM: `{ui_url}/?console=kvm&novnc=1&vmid={vmid}&vmname={name}&node={node}&resize=scale`
  - Container: `{ui_url}/?console=lxc&xtermjs=1&vmid={vmid}&vmname={name}&node={node}`
  - Node: `{ui_url}/?console=shell&xtermjs=1&node={node}`
- The browser talks to Proxmox directly with the user's own Proxmox login.
  Homie grants nothing and needs no new privilege.
- Values are percent-encoded as in RFC 0007. Roles drop the links, like every
  link to another UI (RFC 0016).

### 2. Embedded guest console (opt-in)

- `console = true` in `[proxmox.<name>]` adds an **open console** button on
  guest rows. It opens `/console/{pve}/{node}/{type}/{vmid}`: a full-height
  page with the guest name, status, a **Ctrl+Alt+Del** button for VMs, and a
  link back.
- Containers use xterm.js (`termproxy`). VMs with `serial0` use xterm.js on
  the serial port (`termproxy`), the others noVNC (`vncproxy`). A toggle on
  the console page switches a serial VM to noVNC for boot screens,
  installers and desktops. Serial is the default because it carries text,
  not screen pixels: far less traffic, no frame lag on a slow link, sharp
  text and normal copy and paste.
- Flow, with the Proxmox ticket never sent to the browser:
  1. `POST /api/act/pve/{name}/console` with `{node, type, vmid, mode}`
     (write checks: X-Homie header, same-site Origin). Homie calls
     `vncproxy` or `termproxy` with `websocket=1` and keeps the returned
     port and ticket in memory under a random id, bound to the session,
     valid 30 s, single use. The browser gets the id only.
  2. `GET /api/act/pve/{name}/console/{id}` upgrades to a WebSocket (Origin
     checked against the Host). Homie opens `vncwebsocket` on the same node
     with its API token and the ticket, then copies frames both ways.
- Guards:
  - The button shows only when the token holds `VM.Console`
    (`can_console`, read from the token's permissions like `can_power`).
  - No typed confirm, protected guests included: opening a console changes
    nothing on the guest, and the guest's own login still stands in the way.
  - Roles with `actions = false` and demo sessions get 403 and no button.
  - A console ends after `console_max_minutes` (default 60), when the
    session ends, or on logout.
- Host shells stay links only (section 1). A Proxmox API token gets a
  `login:` prompt on a node shell, not a shell, and `Sys.Console` on a node
  is too close to root to hand to a dashboard token.
- The noVNC and xterm.js code loads only on the console page, as its own
  chunk: the initial JS budget (RFC 0001: 250 kB gzip) does not change.
- Proxmox logs the console under the token (`homie@pve!<token>`), so Homie's
  log is the record of who opened it.

### Configuration

| Key | Default | |
|---|---|---|
| `[proxmox.<name>] console` | `false` | embedded consoles for this cluster; the token also needs `VM.Console` |
| `[proxmox.<name>] console_max_minutes` | `60` | an embedded console closes after this |

Token privilege: add `VM.Console` to the role from RFC 0001
(`PVEAuditor` + `VM.PowerMgmt`). Without it Homie shows links only.
`homie check` reports `console: on`, `off (no VM.Console)` or `off`.

### Logs and metrics

| | |
|---|---|
| `msg="console" user=... pve=... guest=... result=open` | a console was opened |
| `msg="console" ... result=closed seconds=... bytes=...` | it closed, with how long and how much traffic |
| `msg="console" ... result=denied reason=...` | role, demo, no `VM.Console`, expired id |

## Alternatives

| Option | Why not |
|---|---|
| Links only | Covered by section 1; the embed saves the second login and keeps the user in Homie |
| Proxmox console in an `<iframe>` | The Proxmox cookie in a frame on another origin is a third-party cookie, which browsers block; the user would still log in to Proxmox |
| Hand the ticket to the browser and let it reach Proxmox directly | Needs a route from the browser to every node and leaks a console ticket to the client |
| Per-user Proxmox tickets through an OpenID realm | Proxmox would log the real user and enforce its own ACLs, but it needs a realm, user mapping and a ticket per user; its own RFC, still on the roadmap |
| Embedded host shell | See section 2: no shell for a token, and `Sys.Console` is too much privilege |
| SPICE | Needs a desktop client; not a browser feature |

## Testing and rollout

- **Tests:** link templates and encoding; the console id is single use,
  expires after 30 s and is bound to its session; Origin check on the
  upgrade; role and demo denials; serial or noVNC from the guest config; `can_console` from the
  permissions; the proxy copies frames both ways and closes both sides
  (against a local WebSocket server).
- **Build check first:** confirm that `vncwebsocket` and `termproxy` accept an
  API token plus the ticket on Proxmox 8 and 9. Forum reports disagree. If a
  token cannot open them, section 2 goes back to this RFC before any more
  code.
- **End to end:** a throwaway test guest, a container and a VM with
  `serial0`.
- **Rollout:** links in the next minor release; the embed behind
  `console = true`, off by default.
- **Off switch:** `console = false`, or remove `VM.Console` from the token.

## Decisions

1. No typed confirm for consoles, protected guests included.
2. Serial VMs open in xterm.js by default, with a toggle to noVNC.

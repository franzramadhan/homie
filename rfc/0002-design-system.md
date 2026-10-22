# 0002: Homie design system

| Status | Date | PR | Release |
|---|---|---|---|
| accepted | 2026-10-22 | #14 | v0.2.0 |

> Give Homie one deliberate visual language: a dense, dark-first dashboard
> whose only loud colors are the status colors. The structure is inspired by
> Linear's design system (surface ladder, hairlines, a single accent), with
> Homie's own accent, system fonts, and light and dark themes that follow the
> operating system live.

## Problem

The UI uses Tailwind's default zinc palette ad hoc: every component picks its
own shades, borders and spacing. Three concrete problems:

- **Status and decoration compete.** The favicon and the "ok" state are the
  same emerald, links and badges have no shared rules, and nothing reserves
  color for meaning.
- **No tokens.** Colors, radii and type sizes are repeated literals in each
  component, so a change means editing every file, and light and dark mode
  can drift apart.
- **No written reference.** Agents and contributors have nothing to read
  before building a page, so each page looks slightly different.

## Proposal

### Principles

1. **Status colors are the only loud colors.** Green, amber, red and grey
   carry meaning; nothing decorative uses them.
2. **One accent, used sparingly:** focus rings, the active navigation item,
   primary buttons, and links on hover. Never for status, never as
   background decoration.
3. **Dense and calm.** Small type, compact rows, hairline borders, elevation
   by surface lightness, no shadows on dark.
4. **Both themes are first class.** Light and dark follow
   `prefers-color-scheme` and switch live when the OS appearance changes (no
   reload). There is no manual toggle in v0.2.0.
5. **Text never relies on color alone.** Every status keeps a word ("online",
   "failed", "scheduled off").

### Tokens

Defined once as CSS variables in `web/src/index.css`, mapped into Tailwind
v4 `@theme`, with a dark set under `@media (prefers-color-scheme: dark)`.
Components use the token names, never raw shades.

| Token | Dark | Light | Use |
|---|---|---|---|
| `canvas` | `#08090a` | `#f7f8f8` | page background |
| `surface-1` | `#0f1011` | `#ffffff` | cards, panels |
| `surface-2` | `#141516` | `#f3f4f5` | table row hover, inputs |
| `surface-3` | `#191a1b` | `#ebedef` | pressed, selected |
| `hairline` | `#23252a` | `#e2e4e7` | borders, dividers |
| `hairline-strong` | `#34343a` | `#cfd2d6` | input borders, focus-adjacent |
| `ink` | `#f7f8f8` | `#0b0c0d` | primary text |
| `ink-muted` | `#d0d6e0` | `#3c4048` | secondary text |
| `ink-subtle` | `#8a8f98` | `#6b7079` | labels, column headers, timestamps |
| `ink-faint` | `#62666d` | `#9aa0a8` | placeholders, disabled |
| `accent` | `#7c83f0` | `#4f55d6` | focus ring, active nav, primary button |
| `accent-hover` | `#9aa0ff` | `#3f45c2` | hover of the above |

Status colors are Homie's own and tuned for WCAG AA contrast as badge text on
their tinted background in both themes:

| Status | Text dark / light | Tint dark / light | Meaning |
|---|---|---|---|
| `ok` | `#4cc38a` / `#157244` | 12% of the text color over `surface-1` | healthy, online, Ready |
| `warn` | `#f0b44c` / `#8a5700` | same rule | degraded, stale, expiring |
| `bad` | `#f2666a` / `#c4262b` | same rule | down, failed, firing |
| `quiet` | `#8a8f98` / `#5f646d` | same rule | scheduled off (quiet hours) |

The accent is an indigo of Homie's own, deliberately not Linear's exact
lavender: Homie borrows the structure, not the brand. The favicon moves from
emerald to the accent, so no brand mark looks like a status.

### Typography

- **Font:** the system stack, no web fonts (nothing extra to download, and
  the content security policy stays `'self'`):
  `ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif`;
  `ui-monospace, SFMono-Regular, Menlo, monospace` for addresses, digests,
  revisions and task ids.
- **Scale:** 12 (captions, badges), 13 (tables, secondary), 14 (body), 16
  (section titles), 20 (page title). Weights 400, 500, 600.
- **Numbers:** `tabular-nums` everywhere numbers line up (tables, meters).
- **Headings:** slight negative tracking (-0.01em) at 16 px and above.

### Shape, spacing, motion

- 4 px spacing grid. Radius 6 px (controls, badges), 8 px (cards).
- Table rows about 32 px; headers sticky inside their card.
- Color transitions 120 ms; none under `prefers-reduced-motion: reduce`.

### Components

Restyled from the tokens, behavior unchanged:

| Component | Rule |
|---|---|
| Navigation | text links in `ink-subtle`; the active page in `ink` with an accent underline |
| Card / Section | `surface-1`, `hairline` border, 8 px radius, title 16 px / 600 |
| Overview card | status shown by a 3 px left border in the status color plus the status word |
| Table | header 12 px `ink-subtle`, rows 13 px, hover `surface-2`, hairline separators |
| Badge | status text on its tint; `muted` badges use `surface-3` and `ink-subtle` |
| Button | primary: accent fill; secondary: `hairline-strong` border; danger: `bad` text and border |
| Input, select | `surface-2`, `hairline-strong` border, accent focus ring |
| Dialog | `surface-1`, backdrop `#000` at 50%, focus moves into the dialog |
| Meter (bar) | track `surface-3`; fill `ok`, `warn` above 80%, `bad` above 90% |
| Focus ring | 2 px accent, 2 px offset, on every interactive element |

### DESIGN.md

A `DESIGN.md` at the repository root, in the DESIGN.md format, holds the
tokens and rules above. Agents read it before touching the UI (`CLAUDE.md`
points to it). This RFC explains why; `DESIGN.md` is the working reference.

## Alternatives

| Option | Why not |
|---|---|
| Copy Linear's DESIGN.md as is | Its own brand (lavender accent, proprietary font, marketing-scale type): Homie is public and should not look like another product |
| Vercel (monochrome, Geist) | Status colors would stand out even more, but fewer surface steps for hierarchy, and Geist means shipping a web font |
| Sentry | Its DESIGN.md describes the marketing site (lime and pink accents, mascots); lime reads as "ok" |
| Supabase, VoltAgent | Emerald accent collides with the "ok" status |
| shadcn/ui or another component library | New dependencies and a larger bundle for components Homie already has |
| Keep Tailwind defaults | The problems above stay |

## Testing and rollout

- **Tests:** screenshots of every page in light and dark (headless Chrome
  with `--blink-settings=preferredColorScheme=0` and `=1`), reviewed before
  merge; a contrast check of every text and badge token pair in both themes
  (AA: 4.5:1 for text, 3:1 for the meter fills and focus ring).
- **Rollout:** v0.2.0. UI only: no API, config or behavior change, so the
  e2e script must pass unchanged.
- **Off switch:** revert the release; there is no runtime setting.

## Changes during implementation

- Light `ok`, `warn` and `quiet` were darkened (first values gave 4.28 to
  4.34:1 as badge text); `web/scripts/contrast.mjs` checks every pair in CI.
- Tints are one rule in both themes: 12% of the status color over `surface-1`.

## Decisions

Accepted 2026-10-22 with the proposed defaults:

1. Accent: the indigo above.
2. Rows about 32 px.
3. Theme follows the OS only; a manual toggle needs its own RFC.

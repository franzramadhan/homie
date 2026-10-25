---
version: 0.2.0
name: Homie
description: "Dense, calm homelab dashboard. Status colors are the only loud colors; one indigo accent marks focus, the active page and primary actions. Structure inspired by Linear's design system (surface ladder, hairlines, a single accent), with Homie's own accent, system fonts, and light and dark themes that follow the OS live. Rationale: rfc/0002-design-system.md."

colors:
  light:
    canvas: "#f7f8f8"
    surface-1: "#ffffff"
    surface-2: "#f3f4f5"
    surface-3: "#ebedef"
    hairline: "#e2e4e7"
    hairline-strong: "#cfd2d6"
    ink: "#0b0c0d"
    ink-muted: "#3c4048"
    ink-subtle: "#6b7079"
    ink-faint: "#9aa0a8"
    accent: "#4f55d6"
    accent-hover: "#3f45c2"
    on-accent: "#ffffff"
    ok: "#157244"
    warn: "#8a5700"
    bad: "#c4262b"
    quiet: "#5f646d"
  dark:
    canvas: "#08090a"
    surface-1: "#0f1011"
    surface-2: "#141516"
    surface-3: "#191a1b"
    hairline: "#23252a"
    hairline-strong: "#34343a"
    ink: "#f7f8f8"
    ink-muted: "#d0d6e0"
    ink-subtle: "#8a8f98"
    ink-faint: "#62666d"
    accent: "#7c83f0"
    accent-hover: "#9aa0ff"
    on-accent: "#08090a"
    ok: "#4cc38a"
    warn: "#f0b44c"
    bad: "#f2666a"
    quiet: "#8a8f98"

typography:
  sans: 'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif'
  mono: "ui-monospace, SFMono-Regular, Menlo, monospace"
  scale:
    caption: { size: 12px, use: "badges, column headers, timestamps" }
    table: { size: 13px, use: "table cells, secondary text" }
    body: { size: 14px, weight: 400 }
    title: { size: 16px, weight: 600, tracking: -0.01em, use: "card and section titles" }
    page: { size: 20px, weight: 600, tracking: -0.01em, use: "brand and page titles" }
  numbers: tabular-nums

shape:
  radius-control: 6px
  radius-card: 8px
  spacing-grid: 4px
  row-height: 32px
---

# Homie DESIGN.md

The working reference for every UI change. Tokens live in
`web/src/index.css` (CSS variables mapped into Tailwind v4 `@theme`); use
the token names (`bg-surface-1`, `text-ink-subtle`, `border-hairline`,
`bg-ok/12 text-ok`), never raw Tailwind shades, and never `dark:` variants:
the tokens switch with `prefers-color-scheme`.

## Rules

1. **Status colors are the only loud colors.** `ok`, `warn`, `bad`, `quiet`
   carry meaning only: badges, meter fills, the 3 px left border of an
   overview card. Nothing decorative uses them.
2. **One accent, sparingly:** focus rings, the active navigation underline,
   primary buttons, link hover. Never status, never background decoration.
3. **Elevation by lightness.** `canvas` < `surface-1` (cards) < `surface-2`
   (hover, inputs) < `surface-3` (pressed, meter track). Hairline borders,
   no shadows.
4. **Words, not only color.** Every status shows a word ("online", "failed",
   "scheduled off").
5. **Contrast is tested.** `bun run check:contrast` (CI) keeps text at 4.5:1
   and the accent ring and meter fills at 3:1 in both themes.
6. **One exception: the Map canvas** (RFC 0028). Inside its `<canvas>`
   only, a fixed pixel palette is decorative. Status there still uses the
   status colors (their dark values, which read on grass), always with a
   word in a label next to it. Everything around the canvas follows the
   rules above.

## Components

| Component | Recipe |
|---|---|
| Page header | brand 20 px / 600; nav links `text-ink-subtle`, active `text-ink` with a 2 px `accent` underline; header bottom `hairline` |
| Card / Section | `bg-surface-1 border border-hairline rounded-card p-4`; title 16 px / 600 |
| Overview card | as Card, plus `border-l-[3px]` in the status color and a status badge |
| Table | header 12 px `text-ink-subtle`, sticky on `surface-1`; rows 13 px, `tabular-nums`, `hover:bg-surface-2`, `hairline` separators |
| Badge | `bg-<status>/12 text-<status>`; neutral: `bg-surface-3 text-ink-subtle` |
| Button | primary `bg-accent text-on-accent`; secondary `border-hairline-strong text-ink-muted`; danger `border-bad/40 text-bad` |
| Input, select | `bg-surface-2 border-hairline-strong rounded-control`, placeholder `text-ink-faint` |
| Dialog | `bg-surface-1 border-hairline rounded-card`, backdrop black at 50% |
| Meter | track `bg-surface-3` rounded-full; fill `ok`, `warn` above 80%, `bad` above 90% |
| Focus | 2 px `accent` outline, 2 px offset (global `:focus-visible`) |
| Motion | color transitions 120 ms; none under `prefers-reduced-motion` |

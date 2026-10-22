// WCAG contrast check of the design tokens in src/index.css, both themes
// (RFC 0002). Text needs 4.5:1, the accent ring and meter fills 3:1.
// Run: bun run check:contrast
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../src/index.css", import.meta.url), "utf8");
const block = (from) => {
  const start = css.indexOf("{", css.indexOf(from)) + 1;
  const vars = {};
  for (const m of css.slice(start, css.indexOf("}", start)).matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})/gi)) vars[m[1]] = m[2];
  return vars;
};
const themes = { light: block(":root {"), dark: { ...block(":root {"), ...block("@media (prefers-color-scheme: dark)") } };

const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
const lum = (c) => {
  const [r, g, b] = c.map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const ratio = (a, b) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};
// a status tint: the status color at 12% over the card surface
const mix = (fg, bg, alpha) => fg.map((v, i) => v * alpha + bg[i] * (1 - alpha));

let failed = 0;
for (const [name, t] of Object.entries(themes)) {
  const c = Object.fromEntries(Object.entries(t).map(([k, v]) => [k, rgb(v)]));
  const checks = [];
  for (const fg of ["ink", "ink-muted", "ink-subtle"])
    for (const bg of ["canvas", "surface-1", "surface-2"]) checks.push([`${fg} on ${bg}`, c[fg], c[bg], 4.5]);
  for (const s of ["ok", "warn", "bad", "quiet"]) {
    checks.push([`${s} badge`, c[s], mix(c[s], c["surface-1"], 0.12), 4.5]);
    checks.push([`${s} meter on surface-3`, c[s], c["surface-3"], 3]);
  }
  checks.push(["on-accent on accent", c["on-accent"], c.accent, 4.5]);
  checks.push(["accent ring on surface-1", c.accent, c["surface-1"], 3]);
  for (const [label, fg, bg, min] of checks) {
    const r = ratio(fg, bg);
    if (r < min) failed++;
    console.log(`${r < min ? "FAIL" : "ok  "}  ${name.padEnd(5)}  ${label.padEnd(26)} ${r.toFixed(2)} (min ${min})`);
  }
}
if (failed) {
  console.error(`${failed} pairs below WCAG AA`);
  process.exit(1);
}

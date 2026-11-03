// Table export (RFC 0029): CSV text and a browser download. Pure apart from
// `download`, so scripts/csv.test.ts can run it without a browser.

/** One CSV cell. A leading `=`, `+`, `-`, `@`, tab or carriage return gets a
 * `'` so a spreadsheet does not run it as a formula. */
export function cell(v: unknown): string {
  let s = v == null ? "" : typeof v === "object" ? JSON.stringify(v) : String(v);
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
  return /[",\n\r]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
}

export function csv(header: string[], rows: unknown[][]): string {
  return [header, ...rows].map((r) => r.map(cell).join(",") + "\n").join("");
}

/** `homie-<name>-<UTC time>.<ext>`, safe as a file name. */
export function fileName(name: string, ext: string, now = new Date()): string {
  const t = now.toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");
  return `homie-${name.replace(/[^\w.-]+/g, "-")}-${t}.${ext}`;
}

export function download(name: string, type: string, body: BlobPart) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([body], { type }));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

// The CSV writer of the table export (RFC 0029). Run: bun run check:csv
import assert from "node:assert/strict";
import { cell, csv, fileName } from "../src/csv.ts";

assert.equal(cell("plain"), "plain");
assert.equal(cell(42), "42");
assert.equal(cell(null), "");
assert.equal(cell("a,b"), '"a,b"');
assert.equal(cell('say "hi"\nbye'), '"say ""hi""\nbye"');
assert.equal(cell("=HYPERLINK(1)"), "'=HYPERLINK(1)");
assert.equal(cell("-1,2"), "\"'-1,2\"");
assert.equal(cell(["a", "b"]), '"[""a"",""b""]"');
assert.equal(csv(["Name", "Ready"], [["web", true], ["db", false]]), "Name,Ready\nweb,true\ndb,false\n");
assert.equal(fileName("helm releases/main", "csv", new Date("2026-11-03T06:15:00.123Z")), "homie-helm-releases-main-20261103T061500Z.csv");
console.log("csv ok");

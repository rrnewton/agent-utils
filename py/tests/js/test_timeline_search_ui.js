"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const appPath = path.resolve(
  __dirname,
  "../../agent_team_timeline/static/app.js"
);
const source = fs.readFileSync(appPath, "utf8");

function functionSource(name) {
  const marker = "  function " + name + "(";
  const start = source.indexOf(marker);
  assert.notStrictEqual(start, -1, "missing function " + name);
  const next = source.indexOf("\n  function ", start + marker.length);
  assert.notStrictEqual(next, -1, "missing function boundary after " + name);
  return source.slice(start, next);
}

const names = [
  "text",
  "compactSearchText",
  "searchQueryParts",
  "allSearchRanges",
  "searchWordCharacter",
  "wholeSearchRanges",
  "smartSearchMatch",
  "searchExcerpt"
];
const context = {};
vm.createContext(context);
vm.runInContext(names.map(functionSource).join("\n"), context);

assert.ok(context.smartSearchMatch("DBI is a solid B3 at 130/152.", "B3"));
assert.strictEqual(
  context.smartSearchMatch("artifact hash 7ab3cdef was recorded", "B3"),
  null,
  "whole-term matching must not confuse a hash fragment with maturity level B3"
);
assert.strictEqual(
  context.smartSearchMatch("backend level is B30", "B3"),
  null,
  "whole-term matching must reject longer alphanumeric tokens"
);

assert.ok(
  context.smartSearchMatch("KVM backend maturity is now B3.", "KVM B3"),
  "unquoted query terms use AND semantics"
);
assert.strictEqual(
  context.smartSearchMatch("KVM backend maturity remains B2.", "KVM B3"),
  null
);
assert.ok(
  context.smartSearchMatch(
    "The DBI backend passed more than fifty percent of ptrace tests.",
    "\"fifty percent\" DBI"
  ),
  "quoted phrases stay contiguous while combining with other terms"
);

const longText = "prefix ".repeat(40) + "KVM reached maturity B3" + " suffix".repeat(50);
const match = context.smartSearchMatch(longText, "KVM B3");
assert.ok(match);
const excerpt = context.searchExcerpt(match);
assert.strictEqual(excerpt.truncated, true);
assert.ok(excerpt.leadingOmitted > 0);
assert.ok(excerpt.trailingOmitted > 0);
assert.ok(excerpt.ranges.length >= 2);
assert.ok(excerpt.text.includes("KVM reached maturity B3"));

console.log("timeline transcript search UI tests passed");

"use strict";

// `zoomToActivityRange` chooses between two ways of framing a zoom: the bounds recorded on the
// record, and a scan of whatever timeline data is in memory. It used to make that choice by
// asking which *generation* the page had loaded -- `app.schemaMode === "schema2"` -- which was
// true of the only generation that published recorded bounds and became wrong the moment a
// second one did. Schema 3 publishes the same two numbers, derived by the same function, as a
// spine record keyed by stable reference.
//
// So the test is not "does it read the bounds", it is "does it read the bounds *whatever the
// schema says*", which is the bug. Each case below pins one branch and names the failure it
// would be: framing an agent's seven idle hours instead of the one hour it worked, or fetching
// a day of detail shards to recompute a number the record was already carrying.

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const source = fs.readFileSync(
  path.resolve(__dirname, "../../agent_team_timeline/static/app.js"),
  "utf8"
);

function functionSource(name) {
  const marker = "  function " + name + "(";
  const start = source.indexOf(marker);
  assert.notStrictEqual(start, -1, "missing function " + name);
  const next = source.indexOf("\n  function ", start + marker.length);
  assert.notStrictEqual(next, -1, "missing function boundary after " + name);
  return source.slice(start, next);
}

function harness(schemaMode, scanResult) {
  const calls = { zoomed: [], scanned: 0, fetched: 0 };
  const context = {
    MIN_VIEW_MS: 1000,
    app: { schemaMode: schemaMode, viewStart: 0, viewEnd: 1000, data: {} },
    timelineCore: {
      activityRangeWithin: function () {
        calls.scanned += 1;
        return scanResult;
      }
    },
    zoomToRange: function (start, end) {
      calls.zoomed.push([start, end]);
    },
    detailShardsForRange: function () {
      return ["a-shard"];
    },
    requestDetailShards: function () {
      calls.fetched += 1;
      return { promise: Promise.resolve() };
    },
    showDetailLoadError: function (error) {
      throw error;
    }
  };
  vm.createContext(context);
  vm.runInContext(
    [functionSource("number"), functionSource("zoomToActivityRange")].join("\n"),
    context
  );
  return { context: context, calls: calls };
}

const RECORDED = { start_ms: 0, end_ms: 700000, activity_start_ms: 600000, activity_end_ms: 660000 };
const BARE = { start_ms: 0, end_ms: 700000 };

// Recorded bounds are honoured under every generation, including the one that did not exist
// when the guard was written. Under schema 3 the old code fell through to `finishZoom`, which
// scans `app.data` -- and under schema 3 `app.data` holds no events at all, so the zoom framed
// the record's whole interval and the feature silently did nothing.
["schema1", "schema2", "schema3"].forEach(function (mode) {
  const run = harness(mode, null);
  run.context.zoomToActivityRange(RECORDED, null);
  assert.deepStrictEqual(run.calls.zoomed, [[600000, 660000]], mode);
  assert.strictEqual(run.calls.scanned, 0, mode + " must not scan when the record says");
  assert.strictEqual(run.calls.fetched, 0, mode + " must not fetch when the record says");
});

// Bounds that are absent, unparseable or empty are not bounds. An inverted or zero-width pair
// would zoom to nothing, so it falls through to the derivation rather than being obeyed.
[
  BARE,
  { start_ms: 0, end_ms: 700000, activity_start_ms: 5, activity_end_ms: 5 },
  { start_ms: 0, end_ms: 700000, activity_start_ms: 9, activity_end_ms: 4 },
  { start_ms: 0, end_ms: 700000, activity_start_ms: "soon", activity_end_ms: 4 }
].forEach(function (bounds, index) {
  const run = harness("schema1", { start_ms: 10, end_ms: 20 });
  run.context.zoomToActivityRange(bounds, null);
  assert.deepStrictEqual(run.calls.zoomed, [[10, 20]], "case " + index);
  assert.strictEqual(run.calls.scanned, 1, "case " + index);
});

// Schema 1 has the whole timeline in memory, so the derivation is immediate and must not go
// looking for shards that a monolithic export does not have.
const monolith = harness("schema1", { start_ms: 10, end_ms: 20 });
monolith.context.zoomToActivityRange(BARE, null);
assert.strictEqual(monolith.calls.fetched, 0);

// A sharded generation without recorded bounds has to fetch the interval's detail before the
// derivation can see anything, and only then zoom.
const sharded = harness("schema2", { start_ms: 30, end_ms: 40 });
sharded.context.zoomToActivityRange(BARE, null);
assert.strictEqual(sharded.calls.fetched, 1);
assert.deepStrictEqual(sharded.calls.zoomed, []);
Promise.resolve().then(function () {
  assert.deepStrictEqual(sharded.calls.zoomed, [[30, 40]]);
  console.log("timeline zoom UI tests passed");
});

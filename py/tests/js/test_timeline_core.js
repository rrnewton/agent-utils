"use strict";

const assert = require("assert");
const path = require("path");

const core = require(path.resolve(
  __dirname,
  "..",
  "..",
  "agent_team_timeline",
  "static",
  "timeline-core.js"
));

const packed = core.packLifetimes([
  { id: "root", start_ms: 0, end_ms: 100, dedicated: true, input_index: 0 },
  { id: "b", start_ms: 5, end_ms: 8, official_name: "/root/b", input_index: 2 },
  { id: "a", start_ms: 0, end_ms: 10, official_name: "/root/a", input_index: 1 },
  { id: "c", start_ms: 10, end_ms: 12, official_name: "/root/c", input_index: 3 }
]);
assert.strictEqual(packed.lane_count, 3);
assert.strictEqual(packed.lane_by_id.root, 0);
assert.strictEqual(packed.lane_by_id.a, 1);
assert.strictEqual(packed.lane_by_id.b, 2);
assert.strictEqual(packed.lane_by_id.c, 1, "half-open adjacent lifetimes reuse a lane");

let selection = core.nextPhaseSelection(null, "agent-a", "phase-1", 10, 20);
assert.deepStrictEqual(selection, { kind: "agent", agent_id: "agent-a" });
selection = core.nextPhaseSelection(selection, "agent-a", "phase-1", 10, 20);
assert.deepStrictEqual(selection, {
  kind: "phase",
  agent_id: "agent-a",
  phase_id: "phase-1",
  start_ms: 10,
  end_ms: 20
});
selection = core.nextPhaseSelection(selection, "agent-a", "phase-1", 10, 20);
assert.deepStrictEqual(selection, { kind: "agent", agent_id: "agent-a" });

const spawn = {
  id: "spawn",
  kind: "spawn",
  source_id: "parent",
  target_id: "agent-a",
  source_ms: 9,
  target_ms: 10
};
const inPhase = {
  id: "message-in",
  kind: "message",
  source_id: "agent-a",
  target_id: "parent",
  source_ms: 15,
  target_ms: 15
};
const outOfPhase = {
  id: "message-out",
  kind: "message",
  source_id: "agent-a",
  target_id: "parent",
  source_ms: 25,
  target_ms: 25
};
const phaseSelection = {
  kind: "phase",
  agent_id: "agent-a",
  phase_id: "phase-1",
  start_ms: 10,
  end_ms: 20
};
assert.strictEqual(core.edgeDisplayState(spawn, null, false, true), "normal");
assert.strictEqual(core.edgeDisplayState(inPhase, null, false, true), "hidden");
assert.strictEqual(core.edgeDisplayState(inPhase, phaseSelection, false, true), "highlighted");
assert.strictEqual(core.edgeDisplayState(outOfPhase, phaseSelection, false, true), "hidden");
assert.strictEqual(core.edgeDisplayState(outOfPhase, phaseSelection, true, true), "dimmed");

console.log("timeline core tests passed");

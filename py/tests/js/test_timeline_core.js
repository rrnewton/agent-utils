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

const tuesdayStart = Date.UTC(2026, 7, 4, 4);
const wednesdayStart = Date.UTC(2026, 7, 5, 4);
const thursdayStart = Date.UTC(2026, 7, 6, 4);
const calendarRollups = [
  { kind: "daily", start_ms: tuesdayStart, end_ms: wednesdayStart },
  { kind: "daily", start_ms: wednesdayStart, end_ms: thursdayStart },
  {
    kind: "weekly",
    start_ms: Date.UTC(2026, 7, 3, 4),
    end_ms: Date.UTC(2026, 7, 10, 4)
  },
  {
    kind: "monthly",
    start_ms: Date.UTC(2026, 7, 1, 4),
    end_ms: Date.UTC(2026, 8, 1, 4)
  },
  {
    kind: "quarterly",
    start_ms: Date.UTC(2026, 6, 1, 4),
    end_ms: Date.UTC(2026, 9, 1, 4)
  },
  { kind: "unknown", start_ms: 0, end_ms: Number.MAX_SAFE_INTEGER },
  { kind: "daily", start_ms: 20, end_ms: 10 }
];
const eventRange = {
  start_ms: Date.UTC(2026, 7, 5, 3, 10, 54),
  end_ms: Date.UTC(2026, 7, 5, 20, 28, 31)
};
const navigationRange = core.navigableRange(eventRange, calendarRollups);
assert.deepStrictEqual(navigationRange, {
  start_ms: calendarRollups[4].start_ms,
  end_ms: calendarRollups[4].end_ms
});
calendarRollups.slice(0, 5).forEach(function (rollup) {
  assert.deepStrictEqual(
    core.boundedViewRange(
      rollup.start_ms,
      rollup.end_ms,
      navigationRange,
      1000
    ),
    { start_ms: rollup.start_ms, end_ms: rollup.end_ms },
    rollup.kind + " bounds remain navigable outside the event range"
  );
});

const springForwardDay = {
  kind: "daily",
  start_ms: Date.UTC(2026, 2, 8, 5),
  end_ms: Date.UTC(2026, 2, 9, 4)
};
assert.strictEqual(
  springForwardDay.end_ms - springForwardDay.start_ms,
  23 * 60 * 60 * 1000
);
const springNavigation = core.navigableRange(
  {
    start_ms: Date.UTC(2026, 2, 8, 7),
    end_ms: Date.UTC(2026, 2, 8, 12)
  },
  [springForwardDay]
);
assert.deepStrictEqual(
  core.boundedViewRange(
    springForwardDay.start_ms,
    springForwardDay.end_ms,
    springNavigation,
    1000
  ),
  { start_ms: springForwardDay.start_ms, end_ms: springForwardDay.end_ms },
  "rollup zoom uses the supplied DST-aware bounds instead of assuming 24 hours"
);

const activityStart = springForwardDay.end_ms - 42 * 60 * 1000;
assert.deepStrictEqual(
  core.rollupActivityRange(
    springForwardDay,
    {
      range: {
        start_ms: springForwardDay.end_ms - 50 * 60 * 1000,
        end_ms: springForwardDay.end_ms + 12 * 60 * 60 * 1000
      },
      agents: [{
        start_ms: springForwardDay.start_ms,
        end_ms: springForwardDay.end_ms
      }],
      phases: [
        {
          start_ms: springForwardDay.start_ms,
          end_ms: springForwardDay.end_ms,
          states: [{
            kind: "idle",
            start_ms: springForwardDay.start_ms,
            end_ms: springForwardDay.end_ms
          }]
        },
        {
          start_ms: springForwardDay.end_ms - 50 * 60 * 1000,
          end_ms: springForwardDay.end_ms,
          states: [
            {
              kind: "idle",
              start_ms: springForwardDay.end_ms - 50 * 60 * 1000,
              end_ms: activityStart
            },
            {
              kind: "active",
              start_ms: activityStart,
              end_ms: springForwardDay.end_ms
            }
          ]
        }
      ],
      events: [],
      edges: []
    },
    1000
  ),
  { start_ms: activityStart, end_ms: springForwardDay.end_ms },
  "idle states and agent lifetimes do not fill the empty portion of a rollup"
);

const sourceRollup = { start_ms: 100_000, end_ms: 200_000 };
assert.deepStrictEqual(
  core.rollupActivityRange(
    sourceRollup,
    {
      range: { start_ms: 100_000, end_ms: 200_000 },
      phases: [
        {
          start_ms: 120_000,
          end_ms: 130_000,
          states: [{ kind: "tool", start_ms: 120_000, end_ms: 130_000 }]
        },
        { start_ms: 140_000, end_ms: 150_000 }
      ],
      events: [{ at_ms: 110_000 }],
      edges: [{ source_ms: 160_000, target_ms: 190_000 }]
    },
    1000
  ),
  { start_ms: 110_000, end_ms: 190_001 },
  "events, edge endpoints, non-idle states, and unsegmented phases define activity"
);
assert.deepStrictEqual(
  core.rollupActivityRange(
    sourceRollup,
    {
      range: { start_ms: 90_000, end_ms: 180_000 },
      phases: [],
      events: [{ at_ms: 199_900 }],
      edges: []
    },
    1000
  ),
  { start_ms: 199_000, end_ms: 200_000 },
  "minimum point padding stays inside the selected rollup"
);
assert.deepStrictEqual(
  core.rollupActivityRange(
    sourceRollup,
    {
      range: { start_ms: 120_000, end_ms: 180_000 },
      phases: [],
      events: [],
      edges: []
    },
    1000
  ),
  { start_ms: 120_000, end_ms: 180_000 },
  "a rollup without activity falls back to its intersection with the data range"
);

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
selection = core.nextPhaseSelection(selection, "agent-a", "phase-2", 21, 30);
assert.deepStrictEqual(selection, {
  kind: "phase",
  agent_id: "agent-a",
  phase_id: "phase-2",
  start_ms: 21,
  end_ms: 30
}, "a different phase on the selected agent becomes the phase selection");
selection = core.nextPhaseSelection(selection, "agent-a", "phase-2", 21, 30);
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
const result = {
  id: "result",
  kind: "result",
  source_id: "agent-a",
  target_id: "parent",
  source_ms: 20,
  target_ms: 20
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
assert.strictEqual(
  core.edgeDisplayState(result, null, false, false),
  "normal",
  "the terminal child-to-parent join remains visible when detailed edges are off"
);
assert.strictEqual(
  core.edgeDisplayState(result, phaseSelection, false, false),
  "highlighted",
  "the structural join highlights without the highlighted-detailed toggle"
);
assert.strictEqual(
  core.edgeDisplayState(result, { kind: "agent", agent_id: "unrelated" }, false, false),
  "dimmed",
  "an unrelated structural join dims instead of disappearing"
);
assert.strictEqual(core.edgeDisplayState(inPhase, null, false, true), "hidden");
assert.strictEqual(core.edgeDisplayState(inPhase, phaseSelection, false, true), "highlighted");
assert.strictEqual(core.edgeDisplayState(outOfPhase, phaseSelection, false, true), "hidden");
assert.strictEqual(core.edgeDisplayState(outOfPhase, phaseSelection, true, true), "dimmed");

console.log("timeline core tests passed");

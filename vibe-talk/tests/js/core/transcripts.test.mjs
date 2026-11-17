// Tests for `#1608 agent-transcripts-timeline`: the Transcripts core
// (agent · day labels, scoped filtering) and the server search keeping
// transcript hits apart from the raw feed's hits, hidden by default.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CORE = await moduleForTests("src/core/transcripts/index.ts");
const SEARCH = await moduleForTests("src/core/search/index.ts");
const VIEW = await moduleForTests("src/view/transcript/index.ts");

function testClock() {
  const timers = new Map();
  let next = 1;
  return {
    setTimeout: (run, ms) => { timers.set(next, { run, ms }); return next++; },
    clearTimeout: (handle) => { timers.delete(handle); },
    fire: () => { const due = [...timers.values()]; timers.clear(); for (const { run } of due) run(); },
  };
}

const rows = [
  { channel: { id: "C1" }, thread_id: "t2", agent: "lane-b", team: null, day: "2026-11-14", source: "title" },
  { channel: { id: "C1" }, thread_id: "t1", agent: "coordx", team: "team-a", day: "2026-11-15", source: "metadata" },
  { channel: { id: "C1" }, thread_id: "t3", agent: "lane-a", team: null, day: "2026-11-15", source: "metadata" },
];

test("transcripts list as agent · day, newest day first, never a thread name", () => {
  const sorted = CORE.sortTranscripts(rows);
  assert.deepEqual(sorted.map((row) => CORE.transcriptLabel(row)), [
    "coordx · 2026-11-15",
    "lane-a · 2026-11-15",
    "lane-b · 2026-11-14",
  ]);
  for (const row of sorted) assert.ok(!CORE.transcriptLabel(row).includes(":"), "no slug: description");
});

test("the scoped search filters by agent, day and team only", () => {
  assert.equal(CORE.filterTranscripts(rows, "coordx").length, 1);
  assert.equal(CORE.filterTranscripts(rows, "2026-11-15").length, 2);
  assert.equal(CORE.filterTranscripts(rows, "team-a")[0].agent, "coordx");
  assert.equal(CORE.filterTranscripts(rows, "").length, 3);
  assert.equal(CORE.chunkMatches("the cache key ignored the region", "cache region"), true);
  assert.equal(CORE.chunkMatches("the cache key", "region"), false);
});

test("server search keeps transcript hits apart and clears them on a new search", async () => {
  const clock = testClock();
  const hit = (id) => ({ id, channel_id: "C1", timestamp: "2026-11-15T10:00:00Z" });
  const answers = [
    () => Promise.resolve({ hits: [hit("m1")], transcript_hits: [hit("x1")], scopes: [], resume_below_ms: null }),
    () => Promise.resolve({ hits: [], transcript_hits: [], scopes: [], resume_below_ms: null }),
  ];
  const owner = SEARCH.createServerSearch({
    clock,
    pathFor: () => "/search",
    fetchAnswer: () => answers.shift()(),
    render: () => {},
    guard: (fn) => { fn(); },
  });
  owner.schedule("key-1", true);
  clock.fire();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(owner.state().hits.map((one) => one.id), ["m1"]);
  assert.deepEqual(owner.state().transcriptHits.map((one) => one.id), ["x1"]);
  owner.schedule("key-2", true);
  assert.deepEqual(owner.state().transcriptHits, [], "a new search forgets the old transcript hits");
});

test("a chunk's time is said the channel rows' way in Eastern, never as a raw ISO stamp", () => {
  const was = process.env.TZ;
  process.env.TZ = "America/New_York";
  try {
    // 16:30 UTC is 12:30 PM in America/New_York (the zone this test pins).
    const said = VIEW.chunkWhen({ timestamp: "2026-10-12T16:30:00.000Z" });
    assert.equal(said, "10/12 12:30 PM");
    assert.ok(!said.includes("T16:30"), "a raw ISO stamp reached the screen");
    // A time that cannot be parsed shows the raw value, by the owner's rule.
    assert.equal(VIEW.chunkWhen({ timestamp: "not-a-time" }), "not-a-time");
    assert.equal(VIEW.chunkWhen({ timestamp: "" }), "");
  } finally {
    if (was === undefined) delete process.env.TZ;
    else process.env.TZ = was;
  }
});

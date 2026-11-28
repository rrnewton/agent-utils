// Tests for web/src/core/channels/islands.ts on its own (`#2426 vibe-talk-sparse-loading-program`,
// slice R2): where a far-back reader's island breaks off from the newest window, the gaps a break-off
// leaves, where they stand among the rows, and how a fill narrows and closes them. Pure: no DOM.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNELS = await moduleForTests("src/core/channels/index.ts");
const { gapForRun, gapLine, gapPlaces, gapsAboveFloor, narrowGap, planBreakOff, planTopTrim, withGap } = CHANNELS;

const T0 = Date.parse("2026-11-01T00:00:00Z");
const held = (n) => Array.from({ length: n }, (_, i) => ({ id: String(1000 + i), ms: T0 + i * 60000 }));
const message = (i) => ({ id: String(1000 + i), timestamp: new Date(T0 + i * 60000).toISOString(), content: `m${i}` });
const BUDGET = { newestRows: 150, islandRows: 100 };

test("`#2426` R2: no break-off while the reader is in the newest window, or while what is held fits", () => {
  const rows = held(600);
  assert.equal(planBreakOff(rows, null, new Set(), BUDGET), null, "no reader, no break-off");
  assert.equal(planBreakOff(rows, 599, new Set(), BUDGET), null, "at the foot");
  assert.equal(planBreakOff(rows, 450, new Set(), BUDGET), null, "inside the newest 150");
  assert.equal(planBreakOff(held(250), 10, new Set(), BUDGET), null, "250 held fit 150 + 100");
  assert.equal(planBreakOff(held(260), 10, new Set(), BUDGET, 20), null, "inside the slack");
});

test("`#2426` R2: a far-back reader keeps an island around them and the newest window; the middle goes", () => {
  const rows = held(600);
  const plan = planBreakOff(rows, 100, new Set(), BUDGET);
  // Island: 100 rows around index 100 (49 above, 50 below): 51..150. Newest window: 450..599.
  assert.deepStrictEqual(plan, [{ first: 151, last: 449 }]);
  const kept = rows.length - (449 - 151 + 1);
  assert.equal(kept, BUDGET.newestRows + BUDGET.islandRows + 51, "kept = newest + island + the rows older than the island");
  // An island at the very top keeps its full size inside the held stretch.
  assert.deepStrictEqual(planBreakOff(rows, 0, new Set(), BUDGET), [{ first: 100, last: 449 }]);
  // An island that would touch the newest window is no break-off.
  assert.equal(planBreakOff(rows, 420, new Set(), BUDGET), null);
});

test("`#2426` R2: a pinned row between stays, splitting what goes into runs; the plan's properties hold at random", () => {
  const rows = held(600);
  const pins = new Set(["1300"]);
  assert.deepStrictEqual(planBreakOff(rows, 100, pins, BUDGET), [{ first: 151, last: 299 }, { first: 301, last: 449 }]);
  let seed = 7;
  const rand = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648; };
  for (let trial = 0; trial < 300; trial += 1) {
    const n = 1 + Math.floor(rand() * 900);
    const r = held(n);
    const anchor = Math.floor(rand() * n);
    const pinSet = new Set(Array.from({ length: Math.floor(rand() * 5) }, () => String(1000 + Math.floor(rand() * n))));
    const budget = { newestRows: 1 + Math.floor(rand() * 200), islandRows: 1 + Math.floor(rand() * 200) };
    const plan = planBreakOff(r, anchor, pinSet, budget) || [];
    const gone = new Set();
    for (const run of plan) for (let at = run.first; at <= run.last; at += 1) gone.add(at);
    assert.ok(!gone.has(anchor), "the row being read went");
    for (let at = Math.max(0, n - budget.newestRows); at < n; at += 1) assert.ok(!gone.has(at), "a newest-window row went");
    for (const at of gone) assert.ok(!pinSet.has(r[at].id), "a pinned row went");
    for (const at of gone) assert.ok(at > anchor, "a row older than the reader went: that is the floor's business");
    for (const run of plan) assert.ok(r[run.first - 1] && r[run.last + 1], "a run is not between two held rows");
    if (plan.length) assert.ok(n - gone.size >= budget.newestRows + Math.min(budget.islandRows, anchor + 1), "kept less than the budgets");
  }
});

test("`#2426` R2: a run leaves a gap with its edges, count and height; gaps stand above the first row past their older edge", () => {
  const rows = held(600);
  const gap = gapForRun(rows, { first: 151, last: 449 }, 29910.4);
  assert.deepStrictEqual(gap, { olderId: "1150", olderMs: rows[150].ms, newerId: "1450", newerMs: rows[450].ms, count: 299, heightPx: 29910 });
  assert.equal(gapForRun(rows, { first: 0, last: 9 }, null), null, "a run at the top is the floor, not a gap");
  assert.equal(gapLine(gap), "299 messages not loaded");
  assert.equal(gapLine({ ...gap, count: 1 }), "1 message not loaded");
  assert.equal(gapLine({ ...gap, count: null }), "Messages not loaded");
  const shown = [...Array.from({ length: 151 }, (_, i) => message(i)), ...Array.from({ length: 150 }, (_, i) => message(450 + i))];
  const places = gapPlaces(shown, [gap]);
  assert.deepStrictEqual([...places.keys()], ["1150"]);
  // The older edge let go of in turn: the gap still stands, above the first newer row.
  const lessShown = shown.filter((row) => row.id !== "1150");
  assert.deepStrictEqual([...gapPlaces(lessShown, [gap]).keys()], ["1149"]);
  // Nothing newer than it: not between two islands, left out.
  assert.equal(gapPlaces(shown.slice(0, 151), [gap]).size, 0);
  const later = { ...gap, olderId: "1500", olderMs: rows[500].ms, newerId: "1550", newerMs: rows[550].ms };
  const two = withGap(withGap([], later), gap);
  assert.deepStrictEqual(two.map((g) => g.olderId), ["1150", "1500"], "gaps kept oldest first");
  // A break-off further back runs into the gap that stood: one gap, counts and heights summed.
  const further = gapForRun(rows, { first: 101, last: 150 }, 500);
  const joined = withGap([gap], { ...further, newerId: gap.newerId, newerMs: gap.newerMs });
  assert.deepStrictEqual(joined, [{ olderId: "1100", olderMs: rows[100].ms, newerId: "1450", newerMs: rows[450].ms, count: 349, heightPx: 30410 }]);
  assert.equal(withGap([{ ...gap, count: null }], { ...further, newerId: gap.newerId, newerMs: gap.newerMs })[0].count, null,
    "an uncounted gap joined stays uncounted");
});

test("`#2426` R2: a fill narrows a gap from the side it came from and closes it on contact or when the read is exhausted", () => {
  const rows = held(600);
  const gap = gapForRun(rows, { first: 151, last: 449 }, 2990);
  // From the older side (the reader came down from the island): 50 rows below its edge.
  const down = narrowGap(gap, "older", Array.from({ length: 50 }, (_, i) => message(151 + i)), false);
  assert.deepStrictEqual([down.olderId, down.newerId, down.count, down.heightPx], ["1200", "1450", 249, 2490]);
  // From the newer side (the reader came up from the newest window): 50 rows above its edge.
  const up = narrowGap(down, "newer", Array.from({ length: 50 }, (_, i) => message(400 + i)), false);
  assert.deepStrictEqual([up.olderId, up.newerId, up.count], ["1200", "1400", 199]);
  // A fill that reaches the other edge closes it.
  assert.equal(narrowGap(up, "older", Array.from({ length: 201 }, (_, i) => message(201 + i)), false), null);
  // An exhausted read closes it too; an empty one that is not leaves it as it was.
  assert.equal(narrowGap(up, "older", [], true), null);
  assert.equal(narrowGap(up, "older", [], false), up);
  // An uncounted gap stays uncounted.
  const loose = narrowGap({ ...gap, count: null, heightPx: null }, "older", [message(160)], false);
  assert.deepStrictEqual([loose.olderId, loose.count, loose.heightPx], ["1160", null, null]);
});

test("`#2426` R2: a far-back reader's island lets the rows older than it go; a pin older than it stops the trim", () => {
  const rows = held(600);
  assert.equal(planTopTrim(rows, 599, new Set(), BUDGET), null, "at the foot: the residency layer's discard, not this");
  assert.equal(planTopTrim(held(240), 100, new Set(), BUDGET), null, "what is held fits");
  assert.equal(planTopTrim(rows, 30, new Set(), BUDGET), null, "the island already reaches the top");
  // Island: 100 rows around index 300 (49 above): keep from 251.
  assert.equal(planTopTrim(rows, 300, new Set(), BUDGET), 251);
  assert.equal(planTopTrim(rows, 300, new Set(["1100"]), BUDGET), 100, "the oldest pin older than the island stops it");
  assert.equal(planTopTrim(rows, 300, new Set(["1000"]), BUDGET), null, "a pin at the very top keeps everything");
  for (let trial = 0; trial < 200; trial += 1) {
    const n = 1 + ((trial * 37) % 800);
    const anchor = (trial * 53) % n;
    const at = planTopTrim(held(n), anchor, new Set(), BUDGET);
    if (at !== null) assert.ok(at > 0 && at <= anchor, `trim at ${at} would take the row being read (${anchor})`);
  }
});

test("`#2426` R2: a late reply riding with its root does not close a gap; only top-level messages move its edge", () => {
  const rows = held(600);
  const gap = gapForRun(rows, { first: 151, last: 449 }, null);
  const root = { ...message(160), thread: { id: "t160", is_root: true } };
  const lateReply = { id: "9999", timestamp: new Date(T0 + 590 * 60000).toISOString(), content: "late", thread: { id: "t160", is_root: false } };
  const next = narrowGap(gap, "older", [root, lateReply], false);
  assert.ok(next !== null, "a late reply past the newer edge closed the gap");
  assert.equal(next.olderId, "1160");
});

test("`#2426` R2: residency giving up the far-back island raises the floor past its seam, and the seam goes", () => {
  const rows = held(600);
  const far = gapForRun(rows, { first: 101, last: 299 }, null);
  const near = gapForRun(rows, { first: 401, last: 449 }, null);
  assert.deepStrictEqual(gapsAboveFloor([far, near], null), [far, near], "no floor: every gap stands");
  assert.deepStrictEqual(gapsAboveFloor([far, near], rows[350].ms), [near], "the gap under the floor went");
  assert.deepStrictEqual(gapsAboveFloor([far, near], rows[300].ms), [near], "a floor on the newer edge: the gap is under it");
  assert.deepStrictEqual(gapsAboveFloor([far], rows[299].ms), [far], "a floor inside the gap leaves it standing");
  assert.deepStrictEqual(gapsAboveFloor(undefined, rows[10].ms), []);
});

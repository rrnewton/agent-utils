// Tests for web/src/core/channels/residency.ts and web/src/core/settings/residency.ts on their own
// (`#2427 vibe-talk-windowed-default-on`, the residency layer of `#2426`): the plan is pure and
// total, and giving up a run raises every view that lost a row.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNELS = await moduleForTests("src/core/channels/index.ts");
const SETTINGS = await moduleForTests("src/core/settings/index.ts");

const BASE = Date.UTC(2026, 10, 27, 12, 0, 0);
const iso = (ms) => new Date(ms).toISOString();

/** A seeded generator, so a failing case can be named and replayed. */
function rng(seed) {
  let state = seed >>> 0 || 1;
  return () => {
    state ^= state << 13; state >>>= 0;
    state ^= state >>> 17;
    state ^= state << 5; state >>>= 0;
    return state / 0x100000000;
  };
}

function heldRows(count) {
  return Array.from({ length: count }, (_, i) => ({ id: `m${String(i).padStart(5, "0")}`, ms: BASE + i * 60000 }));
}

test("`#2427` residency: nothing goes unless the reader is at the foot of the newest with no search open", () => {
  const { planResidency } = CHANNELS;
  const held = heldRows(400);
  const boundaries = [{ oldestId: held[250].id, oldestMs: held[250].ms, nextBefore: "c250" }];
  const budget = { newestRows: 100, islandRows: 0, maxBytes: 1 };
  const base = { held, pins: new Set(), anchor: null, atNewest: true, searchOpen: false, boundaries, bytesOf: () => 0 };
  assert.equal(planResidency(base, budget).discard.length, 1);
  for (const blocked of [{ atNewest: false }, { searchOpen: true }, { anchor: held[390].id }]) {
    const plan = planResidency({ ...base, ...blocked }, budget);
    assert.deepStrictEqual(plan.discard, [], `gave rows up with ${JSON.stringify(blocked)}`);
    assert.deepStrictEqual(plan.keep, held.map((row) => row.id));
  }
  // At or under the newest window, nothing goes either.
  assert.deepStrictEqual(planResidency({ ...base, held: held.slice(0, 100) }, budget).discard, []);
  // With no boundary at or below the limit, nothing goes: a cut without a cursor would leave a hole.
  assert.deepStrictEqual(planResidency({ ...base, boundaries: [] }, budget).discard, []);
  assert.deepStrictEqual(planResidency({ ...base, boundaries: [{ ...boundaries[0], oldestId: held[350].id }] }, budget).discard, []);
});

test("`#2427` residency: the cut is the deepest boundary that keeps the newest window and every pin", () => {
  const { planResidency } = CHANNELS;
  const held = heldRows(400);
  const at = (i) => ({ oldestId: held[i].id, oldestMs: held[i].ms, nextBefore: `c${i}` });
  const budget = { newestRows: 150, islandRows: 0, maxBytes: 1 };
  const plan = planResidency({ held, pins: new Set(), anchor: null, atNewest: true, searchOpen: false,
    boundaries: [at(50), at(200), at(260), at(300)], bytesOf: () => 0 }, budget);
  assert.equal(plan.discard.length, 1);
  const span = plan.discard[0];
  // 400 - 150 = 250: the boundary at 260 would cut into the newest window, so 200 is the cut.
  assert.deepStrictEqual([span.firstId, span.lastId, span.cutId, span.count, span.nextBefore, span.edge, span.heightPx],
    [held[0].id, held[199].id, held[200].id, 200, "c200", "oldest", null]);
  assert.equal(span.cutMs, held[200].ms);
  assert.deepStrictEqual(plan.keep, held.slice(200).map((row) => row.id));
  // A pin at 120 stops the run above it: the deepest boundary at or above the pin is 50.
  const pinned = planResidency({ held, pins: new Set([held[120].id]), anchor: null, atNewest: true, searchOpen: false,
    boundaries: [at(50), at(200)], bytesOf: () => 0 }, budget);
  assert.equal(pinned.discard[0].cutId, held[50].id);
  // A pin on the first row keeps everything.
  assert.deepStrictEqual(planResidency({ held, pins: new Set([held[0].id]), anchor: null, atNewest: true, searchOpen: false,
    boundaries: [at(50), at(200)], bytesOf: () => 0 }, budget).discard, []);
});

test("`#2427` residency: properties over random inputs (seeded)", () => {
  const { planResidency } = CHANNELS;
  for (let seed = 1; seed <= 400; seed += 1) {
    const random = rng(seed);
    const count = Math.floor(random() * 600);
    const held = heldRows(count);
    const newestRows = 1 + Math.floor(random() * 300);
    const pins = new Set();
    for (let k = Math.floor(random() * 4); k > 0 && count; k -= 1) pins.add(held[Math.floor(random() * count)].id);
    const boundaries = [];
    for (let k = Math.floor(random() * 8); k > 0 && count; k -= 1) {
      const i = Math.floor(random() * count);
      boundaries.push({ oldestId: held[i].id, oldestMs: held[i].ms, nextBefore: `c${i}` });
    }
    const input = { held, pins, anchor: random() < 0.2 ? "reader" : null, atNewest: random() < 0.9,
      searchOpen: random() < 0.1, boundaries, bytesOf: () => 0 };
    const plan = planResidency(input, { newestRows, islandRows: 0, maxBytes: 1 });
    const where = `seed ${seed}`;
    const gone = plan.discard.flatMap((span) => held.slice(0, span.count).map((row) => row.id));
    // keep and discard partition held, in order.
    assert.deepStrictEqual([...gone, ...plan.keep], held.map((row) => row.id), where);
    assert.ok(plan.discard.length <= 1, where);
    // no pin in any discard; the last newestRows always kept.
    assert.ok(gone.every((id) => !pins.has(id)), `${where}: a pin was given up`);
    assert.ok(plan.keep.length >= Math.min(count, newestRows), `${where}: the newest window was cut`);
    if (plan.discard.length) {
      const span = plan.discard[0];
      // one contiguous oldest run, ending at a page boundary, only when allowed.
      assert.ok(input.atNewest && !input.searchOpen && input.anchor === null, `${where}: gave up while not allowed`);
      assert.equal(span.firstId, held[0].id, where);
      assert.equal(span.lastId, held[span.count - 1].id, where);
      assert.ok(boundaries.some((b) => b.oldestId === span.cutId && b.nextBefore === span.nextBefore), `${where}: the cut is no boundary`);
      assert.equal(plan.keep[0], span.cutId, where);
    }
  }
});

test("`#2427` residency: giving up a run cuts the store by time in every view and raises each view that lost a row", () => {
  const { applyDiscard, emptyCanon } = CHANNELS;
  const canon = emptyCanon("c1");
  const message = (id, minute, thread = null, root = false) => ({ id, channel_id: "c1", timestamp: iso(BASE + minute * 60000),
    content: id, thread: thread ? { id: thread, is_root: root, root_message_id: thread, reply_count: null, reply_count_exact: true } : null });
  canon.messages = [
    message("r1", 0, "r1", true), message("a", 1), message("r1-x", 2, "r1"), message("b", 3), message("pinned", 4),
    message("c", 5), message("r1-y", 6, "r1"), message("d", 7),
  ];
  canon.threads = [{ id: "r1", root: canon.messages[0], title: "Thread", reply_count: 2, reply_count_exact: true }];
  const cover = (floor) => ({ floor, more: true, notice: "", top: null, dense: null, at: 1, live: true, cursor: "old" });
  canon.views.set("main", cover(BASE));
  canon.views.set("flat", cover(BASE));
  canon.views.set("thread:r1", cover(BASE));
  canon.views.set("threads", { ...cover(BASE), cursor: "cards" });
  canon.replyGaps.set("r1", 0);
  const span = { firstId: "r1", lastId: "b", cutMs: BASE + 5 * 60000, cutId: "c", count: 4, heightPx: null, edge: "oldest", nextBefore: "c-cursor" };
  const result = applyDiscard(canon, span, { viewKey: "main", pins: new Set(["pinned"]), stillToDo: () => false });
  assert.deepStrictEqual(canon.messages.map((m) => m.id), ["pinned", "c", "r1-y", "d"], "the store kept what the cut left, and the pin");
  assert.deepStrictEqual(result.removed.map((m) => m.id), ["r1", "a", "r1-x", "b"]);
  assert.deepStrictEqual([...result.raised].sort(), ["flat", "main", "thread:r1"]);
  assert.equal(canon.views.get("main").cursor, "c-cursor", "the planned view walks back from the span's cursor");
  assert.equal(canon.views.get("flat").cursor, null, "another view re-reads for its cursor");
  assert.equal(canon.views.get("thread:r1").cursor, null);
  for (const key of ["main", "flat", "thread:r1"]) {
    assert.equal(canon.views.get(key).more, true, key);
    assert.equal(canon.views.get(key).floor, span.cutMs, key);
  }
  assert.equal(canon.views.get("threads").cursor, "cards", "the cards' cover is not touched");
  assert.equal(canon.threads.length, 1, "thread summaries are kept");
  // The read reply given up counts as one the store lacks for being old.
  assert.equal(canon.replyGaps.get("r1"), 1);
  // Nothing ordered before the cut: nothing changes.
  const again = applyDiscard(canon, span, { viewKey: "main", pins: new Set(["pinned"]), stillToDo: () => false });
  assert.deepStrictEqual([again.removed, again.raised], [[], []]);
});

test("`#2427` residency budget: small defaults, its own key, never the polluted data-reach-back", () => {
  const { residencyBudget, RESIDENT_ROWS_PHONE, RESIDENT_ROWS_DESKTOP } = SETTINGS;
  const store = new Map();
  const settings = { get: (setting) => setting.read(store.has(setting.key) ? store.get(setting.key) : null) };
  assert.equal(residencyBudget(settings, true).newestRows, RESIDENT_ROWS_PHONE);
  assert.equal(residencyBudget(settings, false).newestRows, RESIDENT_ROWS_DESKTOP);
  // `#2426` islands read the far-back island's rows beside the newest window's: the same per device.
  assert.equal(residencyBudget(settings, true).islandRows, RESIDENT_ROWS_PHONE);
  assert.equal(residencyBudget(settings, false).islandRows, RESIDENT_ROWS_DESKTOP);
  assert.ok(RESIDENT_ROWS_PHONE <= 300 && RESIDENT_ROWS_DESKTOP <= 300);
  store.set("vibe-talk.voice.data-reach-back", "5000");
  assert.equal(residencyBudget(settings, false).newestRows, RESIDENT_ROWS_DESKTOP, "the windowing switch's saved 5000 took over");
  store.set("vibe-talk.voice.resident-rows", "400");
  assert.equal(residencyBudget(settings, false).newestRows, 400);
  store.set("vibe-talk.voice.resident-rows", "0");
  assert.equal(residencyBudget(settings, true).newestRows, RESIDENT_ROWS_PHONE);
});

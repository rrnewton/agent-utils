// Tests for web/src/view/channel/channelloading.ts and helddone.ts on their own (`#624
// voice-page-modularity`): the loading line and background reads, and Done on held
// messages through its settling and batches. The page over the same factories is in
// ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");
const CORE = await moduleForTests("src/core/channels/index.ts");
const ANNOTATIONS = await moduleForTests("src/core/annotations/index.ts");

function makeLoading() {
  const calls = [];
  const indicator = { hidden: true, textContent: "" };
  const state = { refreshing: false, lineTimer: null, view: "main" };
  const loading = CHANNEL.createChannelLoading({
    el: () => indicator,
    loadState: () => state,
    renderChannelFreshness: () => calls.push("freshness"),
    selection: () => ({ view: state.view }),
  });
  return { loading, indicator, state, calls };
}

test("the loading line shows for an asked read, hides after, and stays quiet over rows", () => {
  const { loading, indicator, state, calls } = makeLoading();
  loading.renderChannelLoading(true);
  assert.equal(indicator.hidden, false);
  assert.equal(indicator.textContent, "Loading messages…");
  assert.equal(state.refreshing, true);
  state.view = "threads";
  loading.renderChannelLoading(true);
  assert.equal(indicator.textContent, "Loading threads…");
  loading.renderChannelLoading(true, true);
  assert.equal(indicator.hidden, true, "a quiet refresh over rows shows no line");
  loading.renderChannelLoading(false);
  assert.equal(indicator.hidden, true);
  assert.equal(state.refreshing, false);
  assert.deepEqual(calls, ["freshness", "freshness", "freshness", "freshness"]);
  assert.equal(CHANNEL.LOADING_LINE_DELAY_MS, 1000);
});

test("only a read nothing the reader did asked for is background", () => {
  const { loading } = makeLoading();
  assert.equal(loading.backgroundRead({ reason: "poll" }), true);
  assert.equal(loading.backgroundRead({ reason: ["live", "mutation"] }), true);
  assert.equal(loading.backgroundRead({ reason: ["live", "asked"] }), false);
  assert.equal(loading.backgroundRead({ reason: "enter" }), false);
  assert.equal(loading.backgroundRead(undefined), false);
});

function makeHeld() {
  const calls = [];
  const canon = CORE.emptyCanon ? null : null;
  const store = {
    channel: "c1",
    messages: [
      { id: "r1", thread: { id: "t1", is_root: true } },
      { id: "a1", thread: { id: "t1" } },
      { id: "b1" },
    ],
    dismissed: new Set(),
    replyGaps: new Map(), settledByDone: new Set(), views: new Map(), threads: [],
    hasThreads: true, scope: null, unplaced: new Set(), unplacedArrivals: new Map(), complete: false,
  };
  const state = { threading: true, channel: "c1", view: "main", pinsOnly: false, batches: [] };
  const held = CHANNEL.createHeldDone({
    readLedger: ANNOTATIONS.createReadLedger(),
    providerFacts: { threading: () => state.threading },
    channelStore: { current: () => store, open: () => store },
    selectedChannelId: () => state.channel,
    selection: () => ({ view: state.view }),
    pinsView: () => ({ only: state.pinsOnly }),
    stillToDo: (message, done) => !done.has(String(message.id)),
    inboxPost: async (route, body) => { state.batches.push([route, body.messages]); return { messages: body.messages }; },
  });
  return { held, store, state, calls };
}

test("Done here marks the store at once and a failed write takes it back", () => {
  const { held, store, state } = makeHeld();
  const mark = held.markHeldDone(["a1"], true);
  assert.deepEqual([...store.dismissed], ["a1"]);
  assert.deepEqual(mark.flipped, ["a1"]);
  held.settleHeldDone(mark, false);
  assert.deepEqual([...store.dismissed], [], "a failed write took back what it changed");
  const again = held.markHeldDone(["a1", "b1"], true);
  held.settleHeldDone(again, true);
  assert.deepEqual([...store.dismissed].sort(), ["a1", "b1"]);
  state.threading = false;
  const none = held.markHeldDone(["b1"], false);
  assert.equal(none.canon, null, "without threads the page holds no store for it");
  assert.deepEqual([...store.dismissed].sort(), ["a1", "b1"], "the store was not touched");
});

test("a mark's part names only its ids, with settlings by their root", () => {
  const { held } = makeHeld();
  const mark = {
    ids: ["a1", "b1"], done: true, canon: null, flipped: ["a1", "b1"], settled: [], unsettled: [],
  };
  const part = held.heldDonePart(mark, ["a1"]);
  assert.deepEqual(part.ids, ["a1"]);
  assert.deepEqual(part.flipped, ["a1"]);
});

test("Done writes go in batches and settle from Main only", async () => {
  const { held, state } = makeHeld();
  assert.equal(held.doneSettlesHere(), true);
  state.view = "thread";
  assert.equal(held.doneSettlesHere(), false);
  state.view = "main"; state.pinsOnly = true;
  assert.equal(held.doneSettlesHere(), false);
  const landed = [];
  const stored = await held.writeInBatches("/dismiss", ["1", "2", "3"], landed);
  assert.deepEqual(stored, ["1", "2", "3"]);
  assert.deepEqual(landed, ["1", "2", "3"]);
  assert.deepEqual(state.batches, [["/dismiss", ["1", "2", "3"]]]);
});

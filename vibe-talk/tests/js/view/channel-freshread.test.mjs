// Tests for web/src/view/channel/freshread.ts on its own (`#624 voice-page-modularity`):
// a landed read's freshness and quick retry, and this page's own posts standing. The page
// over the same factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");

function makeFresh() {
  const calls = [];
  const state = {
    decision: { freshAt: 42, failed: false, freshness: "fresh" },
    retryWanted: false, armed: false, dueAt: 0, threading: true, channel: "c1",
    provider: null, freshAt: 0, sent: new Set(["m1"]), standing: new Set(["m1"]),
  };
  const fresh = CHANNEL.createFreshRead({
    timelineReader: { freshDecision: () => state.decision },
    standInRetries: { noteRead: (failed) => { calls.push(["noteRead", failed]); return state.retryWanted; } },
    ownPosts: {
      sentFromHere: (id) => state.sent.has(id),
      load: () => calls.push("load"),
      stands: (id) => state.standing.has(id),
    },
    loadState: () => state,
    channelPoll: () => ({ armed: () => state.armed, dueAt: () => state.dueAt }),
    scheduleDiscordPoll: (ms) => calls.push(["schedulePoll", ms]),
    setChannelFreshness: (s) => calls.push(["freshness", s]),
    notePinsRevision: (rev) => calls.push(["pins", rev]),
    noteBookmark: () => calls.push("bookmark"),
    providerOfChannel: () => state.provider,
    providerFacts: { threading: () => state.threading },
    selectedChannelId: () => state.channel,
  });
  return { fresh, state, calls };
}

test("a landed read stamps its freshness, pins and bookmark", () => {
  const { fresh, state, calls } = makeFresh();
  fresh.noteFreshRead({ pins_revision: 7 });
  assert.equal(state.freshAt, 42);
  assert.deepEqual(calls, [["noteRead", false], ["freshness", "fresh"], ["pins", 7], "bookmark"]);
});

test("a stand-in read brings an armed poll forward, and only then", () => {
  const { fresh, state, calls } = makeFresh();
  state.decision = { freshAt: 1, failed: true, freshness: "failed" };
  state.retryWanted = true;
  fresh.noteFreshRead({});
  assert.deepEqual(calls.filter((c) => Array.isArray(c) && c[0] === "schedulePoll"), [], "no poll armed, none brought forward");
  state.armed = true;
  state.dueAt = Date.now() + 5000;
  fresh.noteFreshRead({});
  const scheduled = calls.filter((c) => Array.isArray(c) && c[0] === "schedulePoll");
  assert.equal(scheduled.length, 1);
  assert.ok(scheduled[0][1] > 4000 && scheduled[0][1] <= 5000, `the armed poll's remaining wait, got ${scheduled[0][1]}`);
});

test("own posts stand, and the store keeps them only where threads are", () => {
  const { fresh, state, calls } = makeFresh();
  assert.equal(fresh.sentFromThisPage({ id: "m1" }), true);
  assert.equal(fresh.sentFromThisPage({ id: "m9" }), false);
  assert.equal(fresh.ownPostStands("m1"), true);
  fresh.loadOwnPosts();
  assert.deepEqual(calls, ["load"]);
  // No provider described: the selected channel's threading decides.
  assert.equal(fresh.storeKeepsOwnPosts("c1"), true);
  assert.equal(fresh.storeKeepsOwnPosts("other"), false);
  state.threading = false;
  assert.equal(fresh.storeKeepsOwnPosts("c1"), false);
  // A described provider answers for its own channel either way.
  state.provider = { threading_supported: true };
  assert.equal(fresh.storeKeepsOwnPosts("c1"), true);
  state.provider = { threading_supported: false };
  assert.equal(fresh.storeKeepsOwnPosts("c1"), false);
});

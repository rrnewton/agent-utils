// Tests for web/src/view/channel/channelscope.ts on its own (`#624 voice-page-modularity`):
// the saved scope's dismissed set, recording a page-mode scope, clearing what a credential
// read, and what a failed read does to the rows standing. The page over the same factory is
// in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");

function makeScope() {
  const calls = [];
  const state = {
    threading: true, hiding: false, channel: "c1", side: false, sideMode: "thread",
    archived: new Set(["m2"]), freshAt: 0, token: "tok",
    canon: { channel: "c1", dismissed: new Set(["m1"]) },
    cache: null, belongs: true,
  };
  const load = { generation: 0, freshAt: 0 };
  const done = { archived: state.archived };
  const listing = { newestId: "m9" };
  const older = { moreAbove: true, cursor: "cur" };
  const rows = { messages: [{ id: "m1" }, { id: "m2" }], threads: [{ id: "t1" }] };
  const scope = CHANNEL.createChannelScope({
    providerFacts: { threading: () => state.threading, liveDelivery: () => "off", setThreading: () => {}, setBroadcast: () => {}, broadcastSupported: () => false },
    channelStore: { current: () => state.canon, open: () => { calls.push("openStore"); state.canon = { channel: "", dismissed: new Set() }; } },
    channelDirectory: { setCapability: () => {}, setChannels: () => {} },
    messageCache: {
      readMessageCache: () => state.cache,
      trimCacheEntry: () => calls.push("trim"),
      writeMessageCache: () => calls.push("writeCache"),
      forgetChannelScopes: (gone) => calls.push(["forgetScopes", gone("c1")]),
      dropMessageCache: () => calls.push("dropCache"),
    },
    selectedChannelId: () => state.channel,
    selection: () => ({ view: "main", threadId: null, thread: null }),
    columnWork: { side: () => state.side },
    columns: () => ({ main: "main-col", side: { mode: state.sideMode } }),
    timelineRows: () => rows,
    doneState: () => done,
    loadState: () => load,
    listState: () => listing,
    olderPages: () => older,
    readModes: () => ({ hiding: () => state.hiding }),
    bookmarkToSave: () => "bm",
    listedMessages: () => rows.messages,
    plainReach: () => ({ gapFrom: () => null, restore: () => {} }),
    loadCanon: () => calls.push("loadCanon"),
    inheritAllCover: () => calls.push("inheritAllCover"),
    drawProjection: () => true,
    knownChannel: () => ({ id: "c1" }),
    channelName: () => "chan",
    restoreSavedBookmark: () => {},
    el: () => ({ replaceChildren: () => {}, hidden: false }),
    applyNewestPage: () => 2,
    olderPager: () => ({ loadedIsWhole: () => false }),
    channelSummary: () => "sum",
    renderChannelSeam: () => {},
    setChannelFreshness: (s) => calls.push(["freshness", s]),
    renderOutgoingMessages: () => {},
    learnProviders: () => {},
    providerOfChannel: () => null,
    fillChannelSelect: () => {},
    whoIsWho: () => ({ setDeploymentCodingAgents: () => {}, setOwnerAuthorId: () => {}, learnOwnerIds: () => {} }),
    noteSelfAuthor: () => {},
    channelSettings: () => ({ fillServiceSelect: () => {}, renderChannelBox: () => {} }),
    openChannelView: () => calls.push("openChannelView"),
    restoreChannelComposer: () => calls.push("restoreComposer"),
    inColumn: (_c, act) => act(),
    readAloudOwner: () => ({ stopReading: () => calls.push("stopReading"), readingMode: true }),
    channelContexts: { clear: () => calls.push("clearContexts") },
    channelReads: () => ({ payCursor: () => calls.push("payCursor") }),
    forgetPins: () => calls.push("forgetPins"),
    readingBookmark: () => ({ forgetDrawn: () => calls.push("forgetDrawn") }),
    renderOlderControl: () => {},
    screenIdentity: { claim: () => calls.push("claim"), belongsTo: () => state.belongs },
    token: () => state.token,
    sideThreads: { clear: () => calls.push("clearSideThreads") },
    resetSideColumn: () => calls.push("resetSide"),
    forgetBookmarks: () => calls.push("forgetBookmarks"),
    putDockedReplyAway: () => calls.push("putReplyAway"),
    inboxLedger: () => ({ clear: () => calls.push("clearLedger") }),
    uiState: () => ({ forget: () => calls.push("forgetUi") }),
    readableChannelFailure: () => "readable",
    isSlowFailure: () => false,
    channelContextKey: () => "ctx",
    threadPresence: { forget: () => {} },
    voiceCall: () => ({ renderControls: () => {} }),
    renderColumns: () => {},
    saveUiState: () => {},
  });
  return { scope, state, calls, rows, load, done };
}

test("the store's dismissed set follows the rows on screen", () => {
  const { scope, state } = makeScope();
  scope.syncCanonDismissed();
  assert.deepEqual([...state.canon.dismissed], ["m2"]);
  state.side = true; state.sideMode = "cards"; state.archived.add("m1");
  scope.syncCanonDismissed();
  assert.deepEqual([...state.canon.dismissed], ["m2"], "over the cards the right column speaks for no rows");
});

test("a page-mode scope is recorded with its dismissed rows and gap", () => {
  const { scope, state, calls } = makeScope();
  state.threading = false;
  state.cache = { v: 1, identity: "x", shell: null, scopes: {} };
  scope.saveChannelScope();
  const entry = state.cache.scopes["c1"];
  assert.equal(entry.mode, "page");
  assert.deepEqual(entry.dismissed, ["m2"]);
  assert.equal(entry.bookmark, "bm");
  assert.deepEqual(calls, ["trim", "writeCache"]);
});

test("forgetting takes every row a credential read off the screen", () => {
  const { scope, state, calls, rows, load, done } = makeScope();
  scope.forgetMessages();
  assert.deepEqual(rows.messages, []);
  assert.equal(done.archived.size, 0);
  assert.equal(load.generation, 1);
  assert.ok(calls.includes("dropCache") && calls.includes("forgetUi") && calls.includes("putReplyAway"));
  assert.ok(calls.indexOf("putReplyAway") < calls.indexOf("dropCache"));
});

test("a refused read forgets, an unknown channel clears, a plain failure only stale-marks", () => {
  const { scope, state, calls, rows } = makeScope();
  const refused = { refused: true };
  scope.noteChannelReadFailure(refused);
  assert.equal(refused.message, "readable");
  assert.deepEqual(rows.messages, [], "a refusal took the rows");

  const again = makeScope();
  again.scope.noteChannelReadFailure({ status: 404, code: "unknown_channel" });
  assert.deepEqual(again.calls.filter((c) => Array.isArray(c)), [["forgetScopes", true], ["freshness", "fresh"]]);
  assert.deepEqual(again.rows.messages, []);

  const plain = makeScope();
  plain.load.freshAt = 5;
  const err = { network: true };
  plain.scope.noteChannelReadFailure(err);
  assert.equal(err.shownInPill, true);
  assert.deepEqual(plain.calls, [["freshness", "offline"]]);
  assert.equal(plain.rows.messages.length, 2, "the rows stand under a plain failure");
});

test("a screen that belongs to another token is cleared before it can be saved", () => {
  const { scope, state, calls, rows } = makeScope();
  assert.equal(scope.screenBelongsToToken(), true);
  state.belongs = false;
  assert.equal(scope.screenBelongsToToken(), false);
  assert.deepEqual(rows.messages, []);
  assert.ok(calls.includes("forgetBookmarks"));
});

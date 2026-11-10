// Tests for web/src/view/channel/readstate.ts and pendingsends.ts on their own (`#624
// voice-page-modularity`): folding a page's read state into the store, id order, and the
// pending sends' questions. The page over the same factories is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");

function makeReadState() {
  const calls = [];
  const canon = { channel: "c1", messages: [{ id: "10" }, { id: "20" }, { id: "30" }], dismissed: new Set(["10"]) };
  const state = { speaks: new Map(), queued: 0 };
  const rs = CHANNEL.createReadState({
    channelStore: { current: () => canon, open: () => canon },
    readLedger: {
      speaksFor: (payload, id) => state.speaks.get(String(id)) !== false,
      sentEpochOf: () => 3,
    },
    copyLedger: {
      orderCopy: (copy, at) => calls.push(["orderCopy", at]),
      buildCopyOn: () => calls.push("buildCopyOn"),
      orderCopiesRead: (_c, messages, number) => calls.push(["orderCopiesRead", messages.length, number]),
      copyOlderThan: () => true,
    },
    readOrder: { sentNumberOf: () => 9 },
    queueReadAllForCounts: () => { state.queued += 1; },
  });
  return { rs, canon, state, calls };
}

test("id order is numeric whatever the length, and nothing else", () => {
  const { rs } = makeReadState();
  assert.equal(rs.atOrAfter("100", "99"), true);
  assert.equal(rs.atOrAfter("99", "100"), false);
  assert.equal(rs.atOrAfter("010", "10"), true);
  assert.equal(rs.atOrAfter("x10", "10"), false);
  assert.equal(rs.atOrAfter("10", ""), false);
});

test("a page's read state folds into the store, only where the read speaks", () => {
  const { rs, canon, state } = makeReadState();
  state.speaks.set("20", false);
  const changed = rs.foldReadState({ messages: [{ id: "10" }, { id: "20" }, { id: "30" }], dismissed: ["30"] }, "flat");
  assert.equal(changed, true);
  assert.deepEqual([...canon.dismissed].sort(), ["30"], "10 put back, 20 untouched (a local write is newer), 30 marked");
  assert.equal(rs.foldReadState({ messages: [], dismissed: [] }, "flat"), false);
});

test("Main's reply dismissals mark every held message from their position", () => {
  const { rs, canon } = makeReadState();
  canon.dismissed = new Set(["20"]);
  rs.foldReadState({ messages: [], reply_dismissals: { messages: ["30"], from: "15" } }, "main");
  assert.deepEqual([...canon.dismissed], ["30"], "20 put back by the complete marks, 10 before the position untouched");
  // Another view's marks say nothing complete.
  canon.dismissed = new Set(["20"]);
  rs.foldReadState({ messages: [], reply_dismissals: { messages: ["30"], from: "15" } }, "flat");
  assert.deepEqual([...canon.dismissed], ["20"]);
});

test("the ledgers are asked under their own names", () => {
  const { rs, calls } = makeReadState();
  assert.equal(rs.readSpeaksFor({}, "10"), true);
  assert.equal(rs.sentEpochOf({}), 3);
  assert.equal(rs.sentNumberOf({}), 9);
  rs.orderCopy({}, 5);
  rs.buildCopyOn({}, {});
  rs.orderCopiesRead([{ id: "1" }], 2);
  assert.equal(rs.copyOlderThan({}, "1", 2, new Map()), true);
  assert.deepEqual(calls, [["orderCopy", 5], "buildCopyOn", ["orderCopiesRead", 1, 2]]);
});

test("pending sends are asked of the reckoning handed in", () => {
  const asked = [];
  const pending = CHANNEL.createPendingSends({
    pendingThreads: {
      threadOf: (entry) => { asked.push(["threadOf", entry]); return "t1"; },
      into: (thread) => { asked.push(["into", thread]); return 2; },
      times: () => new Map([["t1", 42]]),
    },
  });
  assert.equal(pending.outgoingThreadOf({ id: "e1" }), "t1");
  assert.equal(pending.pendingInto("t1"), 2);
  assert.deepEqual([...pending.pendingThreadTimes()], [["t1", 42]]);
  assert.deepEqual(asked, [["threadOf", { id: "e1" }], ["into", "t1"]]);
});

// Tests for web/src/core/sync/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** How the page keeps its channels current: reads, streams and their order (web/src/core/sync). */
const SYNC = await moduleForTests("src/core/sync/index.ts");

test("`#624 voice-page-modularity`: a Server-Sent Events stream read into frames, across chunk boundaries, from the module alone", async () => {
  const { parseEventFrame, readEventStream } = SYNC;
  assert.deepStrictEqual(parseEventFrame("id: 7\nevent: message_create\ndata: {\"a\":\ndata: 1}\r"),
    { id: "7", event: "message_create", data: '{"a":\n1}' });
  assert.deepStrictEqual(parseEventFrame(": keep-alive"), { id: null, event: "message", data: "" }, "a comment was a frame with data");
  assert.deepStrictEqual(parseEventFrame("data"), { id: null, event: "message", data: "" });
  const reading = (chunks, { stopAfter = Infinity } = {}) => {
    let read = 0;
    let cancelled = 0;
    const reader = {
      read: async () => (read < chunks.length ? { done: false, value: chunks[read++] } : { done: true }),
      cancel: () => { cancelled += 1; return Promise.reject(new Error("refused")); },
    };
    const frames = [];
    const decoder = { decode: (bytes, options) => { assert.deepStrictEqual(options, { stream: true }); return bytes; } };
    const done = readEventStream(reader, () => read <= stopAfter, (frame) => {
      frames.push(frame);
      return frame.event === "reset" ? "reset" : undefined;
    }, decoder);
    return done.then((how) => ({ how, frames, cancelled: () => cancelled }));
  };
  // A frame split across chunks waits for its blank line; a partial last frame is never handed on.
  const split = await reading(["data: one\n", "\nid: 2\nda", "ta: two\n\n: ping\n\ndata: partial"]);
  assert.deepStrictEqual(split.frames.map((frame) => [frame.id, frame.data]), [[null, "one"], ["2", "two"], [null, ""]]);
  assert.equal(split.how, "ended");
  assert.equal((await reading(["event: reset\n\n", "data: after\n\n"])).how, "reset", "a reset frame did not end the read as a reset");
  // Stopped: the reader is cancelled, its refusal swallowed, and no later frame is handed on.
  const stopped = await reading(["data: a\n\n", "data: b\n\n"], { stopAfter: 1 });
  assert.deepStrictEqual(stopped.frames.map((frame) => frame.data), ["a"]);
  assert.deepStrictEqual([stopped.how, stopped.cancelled()], ["ended", 1]);
});

test("`#624 voice-page-modularity`: a read asked for while another is out joins the one queued, and streams' generations, from the module alone", () => {
  const { joinQueuedReads, createStreamGeneration } = SYNC;
  assert.deepStrictEqual(joinQueuedReads({ reason: "reset" }, { reason: "live", keepPosition: true }),
    { reason: ["reset", "live"], keepPosition: true }, "a reason was dropped, or the newest options lost");
  assert.deepStrictEqual(joinQueuedReads({ reason: ["poll", "live"] }, { reason: "live" }), { reason: ["poll", "live"] });
  assert.deepStrictEqual(joinQueuedReads({ reason: "poll" }, { reason: "poll" }), { reason: "poll" }, "one reason was not left as one");
  assert.deepStrictEqual(joinQueuedReads({ reason: "view", quiet: true, readAs: "old" }, { reason: "poll", readAs: "new" }),
    { reason: ["view", "poll"], readAs: "new", quiet: true }, "quiet was not kept from the read already queued");
  assert.deepStrictEqual(joinQueuedReads(undefined, undefined), { reason: undefined });
  const generation = createStreamGeneration();
  const started = generation.current();
  generation.bump();
  assert.deepStrictEqual([started, generation.current()], [0, 1]);
});

test("`#624 voice-page-modularity`: the live events counted per channel, from the module alone", () => {
  const { createLiveEvents } = SYNC;
  const live = createLiveEvents();
  assert.deepStrictEqual([live.seen("c"), live.news("c"), live.removals("c")], [0, 0, 0]);
  live.noteEvent("c");
  live.noteEvent("c");
  live.noteNews("c");
  live.noteRemoval("d");
  assert.deepStrictEqual([live.seen("c"), live.news("c"), live.removals("c"), live.removals("d"), live.seen("d")], [2, 1, 0, 1, 0],
    "one channel's or one kind's events were counted as another's");
});

test("`#624 voice-page-modularity`: views held unpolled after a read given up on, from the module alone", () => {
  const { createReadHolds, SERVER_READ_BOUND_MS } = SYNC;
  let now = 1000;
  const holds = createReadHolds({ now: () => now });
  const hold = holds.hold("c main", "c", 1000);
  holds.hold("c threads", "c", 1000);
  holds.hold("d main", "d", 1000);
  // Only a read whose every reason is the poll waits.
  assert.deepStrictEqual([holds.held("c main", "poll"), holds.held("c main", ["poll", "poll"]), holds.held("c main", ["poll", "reset"]),
    holds.held("c main", undefined), holds.held("e main", "poll")], [true, true, false, false, false]);
  assert.equal(holds.channelWaits("c"), true);
  // An answer takes away its own hold, not one set again since.
  holds.release("c main", { channel: "c", until: 0 });
  assert.equal(holds.held("c main", "poll"), true, "an answer took away a hold that was not its own");
  holds.release("c main", hold);
  assert.equal(holds.held("c main", "poll"), false);
  // Live news ends a channel's holds; the server's bound ends the rest.
  holds.liftChannel("c");
  assert.deepStrictEqual([holds.channelWaits("c"), holds.channelWaits("d")], [false, true]);
  now = 1000 + SERVER_READ_BOUND_MS;
  assert.equal(holds.held("d main", "poll"), false, "a hold past the server's bound still held the poll");
  holds.hold("d main", "d", 1000);
  assert.equal(holds.channelWaits("d"), false, "a hold past its bound kept the placing read waiting");
});

test("`#624 voice-page-modularity`: which replayed events still wait for a read, from the module alone", () => {
  const { createReplayLedger } = SYNC;
  const ledger = createReplayLedger();
  assert.deepStrictEqual([ledger.started(), ledger.start(), ledger.start(), ledger.started()], [0, 1, 2, 2]);
  ledger.noteAttached();
  assert.equal(ledger.attachedAfter(), 2);
  // Nothing landed yet: both wait.
  assert.equal(ledger.awaitRead("m1", "c", 2), false);
  assert.equal(ledger.awaitRead("message_update:m2", "c", 2), false);
  assert.deepStrictEqual([ledger.waiting(), ledger.mutationAwaits()], [2, true]);
  // A delta of additions begun after them answers the new message, not the edit.
  const read = ledger.start();
  ledger.settle(read, { messages: [], threads: [], delta: { complete: false } }, "c");
  assert.deepStrictEqual([ledger.waiting(), ledger.mutationAwaits()], [1, true], "an incomplete delta answered an edit");
  // A failed read answers for nothing; a complete one answers the edit.
  ledger.settle(ledger.start(), null, "c");
  assert.equal(ledger.waiting(), 1);
  ledger.settle(ledger.start(), { messages: [], delta: null }, "c");
  assert.deepStrictEqual([ledger.waiting(), ledger.mutationAwaits()], [0, false]);
  // Once a read past an arrival has landed, a later replay of it needs no read; another channel's is forgotten.
  assert.equal(ledger.awaitRead("m3", "c", 3), true, "a landed read did not answer an event it began after");
  assert.equal(ledger.awaitRead("m4", "other", 99), false);
  ledger.settle(ledger.start(), null, "c");
  assert.equal(ledger.waiting(), 0, "an event of a channel no longer on screen still waited");
  // A page that holds it answers it, though the read began before it arrived.
  assert.equal(ledger.awaitRead("m5", "c", 99), false);
  ledger.settle(4, { messages: [{ id: "m5" }] }, "c");
  assert.equal(ledger.waiting(), 0);
});

test("`#624 voice-page-modularity`: the order timeline reads were sent in, from the module alone", () => {
  const { createReadOrder } = SYNC;
  const order = createReadOrder();
  const first = {};
  const second = {};
  const unasked = {};
  assert.deepStrictEqual([order.asked(), order.now()], [0, 0.5]);
  const firstNumber = order.asked();
  order.noteAsked();
  const secondNumber = order.asked();
  order.noteAsked();
  // The second read's answer lands first: each still stands where its read was sent.
  order.noteSent(second, secondNumber);
  order.noteSent(first, firstNumber);
  assert.deepStrictEqual([order.sentNumberOf(first), order.sentNumberOf(second)], [0, 1]);
  assert.equal(order.sentNumberOf(unasked), 2, "a payload no read sent did not stand after every read so far");
  assert.equal(order.now(), 2.5, "something put in now did not stand between reads");
});

test("`#624 voice-page-modularity`: the quick retries after a stand-in read, from the module alone", () => {
  const { createStandInRetries, STAND_IN_RETRY_MS, DISCORD_POLL_MS } = SYNC;
  const retries = createStandInRetries();
  assert.equal(retries.take(DISCORD_POLL_MS), DISCORD_POLL_MS, "a poll came early with no stand-in read before it");
  // Each stand-in read wants the next quick retry, until they are spent.
  const waits = [];
  for (let read = 0; read <= STAND_IN_RETRY_MS.length; read += 1) {
    const wanted = retries.noteRead(true);
    waits.push([wanted, retries.take(DISCORD_POLL_MS)]);
  }
  assert.deepStrictEqual(waits, [...STAND_IN_RETRY_MS.map((ms) => [true, ms]), [false, DISCORD_POLL_MS]]);
  // A shorter wait already armed is kept; a current read starts the retries again.
  retries.noteRead(false);
  assert.equal(retries.noteRead(true), true, "a current read did not start the quick retries again");
  assert.equal(retries.take(1000), 1000);
  assert.equal(retries.noteRead(false), false);
  assert.equal(retries.take(DISCORD_POLL_MS), DISCORD_POLL_MS, "a retry wanted before a current read was still taken");
});

test("`#624 voice-page-modularity`: which read a refresh makes, and how fresh it is, from the module alone", () => {
  const { deltaCursor, isCursorRefusal, isMutationKey, staleAsOf, STALE_AS_OF_MS, TIMELINE_RECONCILE_MS } = SYNC;
  let now = 10 * TIMELINE_RECONCILE_MS;
  const facts = (over = {}) => ({
    storeChannel: () => "c", selectedChannel: () => "c", complete: () => true, mutationAwaits: () => false, now: () => now,
    cover: () => ({ live: true, newest: "cur", fullAt: now - 1000 }), ...over,
  });
  assert.equal(deltaCursor("poll", facts()), "cur");
  assert.equal(deltaCursor("enter", facts()), null, "a reason no delta serves read a delta");
  assert.equal(deltaCursor(undefined, facts()), null);
  assert.equal(deltaCursor("poll", facts({ storeChannel: () => "other" })), null, "another channel's cursor was used");
  assert.equal(deltaCursor("poll", facts({ cover: () => ({ live: false, newest: "cur", fullAt: now }) })), null);
  assert.equal(deltaCursor("poll", facts({ cover: () => ({ live: true, newest: "cur", fullAt: now - TIMELINE_RECONCILE_MS }) })), null,
    "a view past its reconcile read a delta");
  // Edits and removals need complete deltas; a replay only while one is waiting for a read.
  assert.deepStrictEqual(["mutation", "replay", "poll", "live"].map((reason) => deltaCursor(reason, facts({ complete: () => false }))),
    [null, "cur", null, "cur"]);
  assert.equal(deltaCursor("replay", facts({ complete: () => false, mutationAwaits: () => true })), null);
  // A queued read standing for several reasons is a delta only if every one could be.
  assert.equal(deltaCursor(["poll", "live"], facts()), "cur");
  assert.equal(deltaCursor(["poll", "enter"], facts()), null);
  assert.equal(deltaCursor([], facts()), null);
  assert.deepStrictEqual([isCursorRefusal({ status: 410, code: "cursor_expired" }), isCursorRefusal({ status: 400, code: "cursor_mismatch" }),
    isCursorRefusal({ status: 410, code: "other" }), isCursorRefusal(null)], [true, true, false, false]);
  assert.deepStrictEqual([isMutationKey("message_update:1"), isMutationKey("message_delete:1"), isMutationKey("message:1")], [true, true, false]);
  // Freshness: now, unless the server says its copy is old enough to say so, or it answered from its own copy.
  const at = (ms) => new Date(ms).toISOString();
  now = Date.parse("2026-10-29T12:00:00Z");
  const clock = () => now;
  assert.equal(staleAsOf({ as_of: at(now - STALE_AS_OF_MS) }, false, clock), null);
  assert.equal(staleAsOf({ as_of: at(now - STALE_AS_OF_MS - 1000) }, false, clock), now - STALE_AS_OF_MS - 1000);
  assert.equal(staleAsOf({}, false, clock), null);
  assert.equal(staleAsOf({ as_of: at(now - 5000) }, true, clock), now - 5000, "a stand-in copy was taken for a current one");
  assert.equal(staleAsOf({ as_of: at(now + 5000) }, true, clock), now, "a stand-in copy dated ahead claimed to be newer than now");
  assert.equal(staleAsOf(null, true, clock), now);
});

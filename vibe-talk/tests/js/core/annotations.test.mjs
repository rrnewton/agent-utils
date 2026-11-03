// Tests for web/src/core/annotations/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** What the reader has done to messages, as the page holds it (web/src/core/annotations). */
const ANNOTATIONS = await moduleForTests("src/core/annotations/index.ts");

test("`#624 voice-page-modularity`: the reading bookmark's owner, from the module alone: a read's word, the owner's move ahead of its write, settling it", () => {
  const { createBookmarks, sameAnchor, isBookmarkState, anchorOfMessage } = ANNOTATIONS;
  let epoch = 0;
  const store = createBookmarks({ bumpEpoch: () => ++epoch });
  const a = anchorOfMessage({ id: "1", timestamp: "t1", thread: { id: "th" } }, "above");
  const b = anchorOfMessage({ id: "2", timestamp: "t2" }, "below");
  assert.deepStrictEqual(a, { message_id: "1", place: "above", timestamp: "t1", thread_id: "th" });
  assert.deepStrictEqual([sameAnchor(a, a), sameAnchor(a, b), sameAnchor(null, undefined), isBookmarkState({ revision: 1, anchor: null })], [true, false, true, true]);
  // A read's word is taken while nothing of this page's is on its way.
  assert.equal(store.note("c", { revision: 3, anchor: a }, 0), true);
  assert.deepStrictEqual(store.held("c"), { revision: 3, anchor: a, token: null, known: true });
  assert.equal(store.toSave("c").revision, 3);
  // The owner's move: at once, before the write; a read meanwhile, or sent before the write settled, is not taken.
  const move = store.stage("c", b);
  assert.deepStrictEqual([store.held("c").anchor, store.writing("c"), epoch], [b, true, 1]);
  assert.equal(store.note("c", { revision: 4, anchor: a }, 5), false, "a read taken while a write was on its way");
  store.writeEnded("c");
  assert.deepStrictEqual([store.writing("c"), epoch], [false, 2]);
  assert.equal(store.note("c", { revision: 4, anchor: a }, 1), false, "a read sent before the write settled was taken");
  // Settled by the server's answer, still this move's: its revision and its anchor.
  assert.deepStrictEqual(store.settle(move, { bookmark: { revision: 5, anchor: b } }), { mine: true, before: b, after: b });
  assert.equal(store.held("c").revision, 5);
  // A failed write is taken back.
  const failed = store.stage("c", null);
  store.writeEnded("c");
  assert.deepStrictEqual(store.settle(failed, null), { mine: true, before: null, after: b });
  // A saved copy shows only where nothing newer is held; forgetting the credential forgets them all.
  store.restoreSaved("c", { revision: 1, anchor: a });
  assert.equal(store.held("c").revision, 5);
  store.forget();
  assert.equal(store.held("c"), null);
  assert.equal(store.settle(move, null), null, "a write settled after its credential was forgotten");
});

test("`#624 voice-page-modularity`: which reads still speak for a message's read state, from the module alone", () => {
  const { createReadLedger } = ANNOTATIONS;
  const ledger = createReadLedger();
  const before = {};
  ledger.noteSent(before, ledger.epoch());
  assert.equal(ledger.speaksFor(before, "1"), true, "a read spoke for nothing it wrote");
  ledger.writeStarted(["1"]);
  const during = {};
  ledger.noteSent(during, ledger.epoch());
  assert.deepStrictEqual([ledger.speaksFor(before, "1"), ledger.speaksFor(during, "1"), ledger.written("1")], [false, false, true]);
  ledger.writeSettled(["1"]);
  const after = {};
  ledger.noteSent(after, ledger.epoch());
  assert.deepStrictEqual([ledger.speaksFor(during, "1"), ledger.speaksFor(after, 1)], [false, true],
    "a read sent while the write was out spoke after it settled, or one sent after did not");
  // Two writes on their way: settling one is not enough.
  ledger.writeStarted(["2"]);
  ledger.writeStarted(["2"]);
  ledger.writeSettled(["2"]);
  assert.equal(ledger.speaksFor({}, "2"), false);
  // A payload no read sent is now, and holds the local state already; a seeded message speaks from its epoch.
  ledger.seed("3", ledger.bump());
  assert.equal(ledger.speaksFor({}, "3"), true);
  assert.deepStrictEqual([...ledger.writtenIds()], ["1", "2", "3"]);
});

test("`#624 voice-page-modularity`: the pinned messages' owner, from the module alone: a list, changes laid over it, answers and refusals", () => {
  const { createPins } = ANNOTATIONS;
  const pins = createPins();
  const pin = (id) => ({ message_id: id, content: `m${id}` });
  pins.forget("c");
  assert.deepStrictEqual([pins.channel(), pins.revision(), pins.size()], ["c", null, 0]);
  // A change asked for and not answered survives a list read meanwhile.
  pins.stage("2", pin("2"));
  pins.apply({ pins: [pin("1")], revision: 4, limit: 50 });
  assert.deepStrictEqual(pins.entries().map(([id]) => id), ["1", "2"]);
  assert.deepStrictEqual([pins.revision(), pins.limit(), pins.isPinned("c", 2), pins.isPinned("d", "2")], [4, 50, true, false]);
  // Answered: the server's pin, and the oldest it unpinned to make room.
  pins.answered("2", { ...pin("2"), content: "server's" }, ["1"]);
  assert.deepStrictEqual(pins.entries().map(([id, held]) => [id, held.content]), [["2", "server's"]]);
  // Refused: what was held before, back.
  const before = pins.held("2");
  pins.stage("2", null);
  assert.equal(pins.isPinned("c", "2"), false);
  pins.rollback("2", before);
  assert.equal(pins.held("2"), before);
  // One read in flight; a newer one supersedes it; a failure says why.
  const first = pins.startRead("c");
  const second = pins.startRead("c");
  assert.deepStrictEqual([pins.isCurrent(first), pins.isCurrent(second)], [false, true]);
  pins.endRead(first);
  assert.equal(pins.inFlight(), second, "an old read's end ended the current one");
  pins.failed("no answer");
  assert.equal(pins.problem(), "no answer");
  pins.forget("d");
  assert.deepStrictEqual([pins.channel(), pins.problem(), pins.inFlight(), pins.size()], ["d", "", null, 0]);
});

test("`#624 voice-page-modularity`: a removed bookmark's Undo is offered for BOOKMARK_UNDO_MS, one at a time", () => {
  const { createBookmarkUndo, BOOKMARK_UNDO_MS } = ANNOTATIONS;
  assert.equal(BOOKMARK_UNDO_MS, 10000);
  const timers = new Map();
  let next = 0;
  let drawn = 0;
  const undo = createBookmarkUndo({
    clock: { setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; }, clearTimeout: (h) => timers.delete(h) },
    ended: () => { drawn += 1; },
  });
  assert.equal(undo.current(), null);
  const token = {};
  undo.offer({ channel: "c", anchor: { message_id: "1" }, token, at: 3 });
  assert.equal(undo.current().token, token);
  assert.equal([...timers.values()][0].ms, BOOKMARK_UNDO_MS);
  undo.offer({ channel: "c", anchor: { message_id: "2" }, token: {}, at: 4 });
  assert.equal(timers.size, 1, "a second offer left the first one's timer armed");
  [...timers.values()][0].run();
  assert.deepStrictEqual([undo.current(), drawn], [null, 1]);
  undo.offer({ channel: "d", anchor: { message_id: "3" }, token: {}, at: 5 });
  undo.end();
  assert.deepStrictEqual([undo.current(), drawn, timers.size], [null, 2, 0]);
});

test("`#624 voice-page-modularity`: the old place marker is read once, looked for a bounded number of reads, and dropped from storage", () => {
  const { createLegacyMarker, MARKER_KEY, LEGACY_MARKER_READS } = ANNOTATIONS;
  const store = new Map([[MARKER_KEY, "M7"]]);
  const storage = { getItem: (key) => store.get(key) ?? null, setItem: (key, value) => store.set(key, value), removeItem: (key) => store.delete(key) };
  const marker = createLegacyMarker({ storage });
  assert.equal(marker.id(), "M7");
  for (let read = 1; read < LEGACY_MARKER_READS; read += 1) marker.noteRead();
  assert.equal(marker.exhausted(), false);
  marker.noteRead();
  assert.equal(marker.exhausted(), true);
  marker.drop();
  assert.equal(marker.id(), null);
  assert.equal(store.has(MARKER_KEY), false);
  const refused = { getItem: () => { throw new Error("SecurityError"); }, setItem: () => {}, removeItem: () => { throw new Error("SecurityError"); } };
  const none = createLegacyMarker({ storage: refused });
  assert.equal(none.id(), null, "a browser refusing storage has no old marker");
  none.noteRead();
  none.drop();
  assert.equal(none.id(), null);
});

test("`#624 voice-page-modularity`: a bookmark write goes in order, conditional where asked, not sent after a failed Done, and says what came of it", async () => {
  const { createBookmarks, createBookmarkWrites, sameAnchor } = ANNOTATIONS;
  const bookmarks = createBookmarks({ bumpEpoch: () => {} });
  const calls = [];
  const said = [];
  let refuse = null;
  const writes = createBookmarkWrites({
    bookmarks,
    queue: (write) => write(),
    put: async (channel, body) => {
      calls.push(["PUT", channel, body]);
      if (refuse) throw refuse;
      return { bookmark: { revision: 2, anchor: { message_id: body.message_id, timestamp: body.timestamp, place: body.place, thread_id: null } }, bookmark_notice: "" };
    },
    remove: async (channel, revision) => { calls.push(["DELETE", channel, revision]); return { bookmark: { revision: 3, anchor: null }, bookmark_notice: "" }; },
    isAnswer: (body) => Boolean(body && body.bookmark && typeof body.bookmark.revision === "number"),
    channel: () => "C1",
    anchor: (channel) => { const held = bookmarks.held(channel); return held ? held.anchor : null; },
    same: (one, other) => sameAnchor(one, other),
    redraw: () => said.push("redraw"),
    save: () => said.push("save"),
    status: (text) => said.push(text),
    error: (text) => said.push(`error: ${text}`),
  });
  const anchor = { message_id: "M1", timestamp: "2026-11-02T00:00:00Z", place: "above", thread_id: null };
  const move = bookmarks.stage("C1", anchor);
  assert.equal(await writes.send(move), "saved");
  assert.deepStrictEqual(calls[0], ["PUT", "C1", anchor], "an unconditional write sends no revision");
  assert.ok(said.includes("save"));
  const cancelled = bookmarks.stage("C1", null);
  assert.equal(await writes.send(cancelled, { after: Promise.resolve(false) }), "cancelled");
  assert.equal(calls.length, 1, "nothing is sent after the Done write it follows failed");
  refuse = { status: 409, body: { bookmark: { revision: 9, anchor: null }, bookmark_notice: "" } };
  const moved = bookmarks.stage("C1", { ...anchor, message_id: "M2" });
  assert.equal(await writes.send(moved, { conditional: true }), "moved");
  assert.equal(calls.at(-1)[2].if_revision, 2, "a conditional write sends the revision held");
  assert.ok(said.includes("Bookmark moved on another device."));
  refuse = { status: 500, detail: "the store is down" };
  const failed = bookmarks.stage("C1", { ...anchor, message_id: "M3" });
  assert.equal(await writes.send(failed), "failed");
  assert.deepStrictEqual(said.slice(-2), ["The bookmark was not saved.", "error: The bookmark could not be saved: the store is down"]);
  said.length = 0;
  refuse = { status: 500, detail: "again" };
  assert.equal(await writes.send(bookmarks.stage("C1", { ...anchor, message_id: "M4" }), { quiet: true }), "failed");
  assert.ok(!said.some((line) => line.startsWith("The bookmark")), "a quiet write says nothing");
});

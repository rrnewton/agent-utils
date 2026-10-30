// Tests for web/src/core/channels/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** One store per channel and its views' rules (web/src/core/channels). */
const CHANNELS = await moduleForTests("src/core/channels/index.ts");

test("`#624 voice-page-modularity`: the channel store's rules hold from the module alone: views, time order, threads", () => {
  const { emptyCanon, inTimeOrder, inViewOf, sameMessage, threadOf, timeOf, viewOfKey } = CHANNELS;
  const a = emptyCanon(42);
  const b = emptyCanon();
  assert.equal(a.channel, "42");
  assert.equal(b.channel, "");
  assert.notEqual(a.views, b.views, "two stores share a map");
  assert.deepStrictEqual([a.scope, a.complete, a.hasThreads, a.messages, a.threads], [null, null, false, [], []]);
  assert.deepStrictEqual(viewOfKey("thread:t1"), { view: "thread", thread: "t1" });
  assert.deepStrictEqual(viewOfKey("main"), { view: "main", thread: null });
  assert.ok(Number.isNaN(timeOf(null, "timestamp")) && Number.isNaN(timeOf({ timestamp: "never" }, "timestamp")));
  // Stable by time where every row has one; the server's order where one has none.
  const rows = [{ id: "b", timestamp: "2026-10-01T00:00:02Z" }, { id: "a", timestamp: "2026-10-01T00:00:01Z" },
    { id: "c", timestamp: "2026-10-01T00:00:01Z" }];
  assert.deepStrictEqual(inTimeOrder(rows, "timestamp").map((row) => row.id), ["a", "c", "b"]);
  const unknown = [...rows, { id: "d" }];
  assert.equal(inTimeOrder(unknown, "timestamp"), unknown, "a row with no time did not leave the order as given");
  // The server's timeline rule: All everything, a thread its own, Main roots and unthreaded, and in a
  // scoped channel its one conversation whole.
  const root = { id: "1", thread: { id: "t", is_root: true } };
  const reply = { id: "2", thread: { id: "t", is_root: false } };
  const plain = { id: "3" };
  assert.deepStrictEqual([root, reply, plain].map((m) => inViewOf(m, "main", null, null)), [true, false, true]);
  assert.deepStrictEqual([root, reply, plain].map((m) => inViewOf(m, "thread", "t", null)), [true, true, false]);
  assert.deepStrictEqual([root, reply, plain].map((m) => inViewOf(m, "flat", null, null)), [true, true, true]);
  assert.equal(inViewOf(reply, "main", null, "t"), true, "a scoped channel's reply is not on its Main");
  assert.deepStrictEqual([threadOf(reply), threadOf(plain), threadOf(null), threadOf({ thread: { id: 7 } })], ["t", null, null, null]);
  assert.equal(sameMessage({ id: "1", content: "x" }, { id: "1", content: "x" }), true);
  assert.equal(sameMessage({ content: "x", id: "1" }, { id: "1", content: "x" }), false, "fields in another order are another copy");
  assert.equal(sameMessage(undefined, undefined), false, "nothing held is never the arriving copy");
});

test("`#624 voice-page-modularity`: folding a page into a store, from the module alone: a newest page's window, older pages, removals below it", () => {
  const { emptyCanon, mergeIntoCanon, upsertThreadSummary, heldCopies } = CHANNELS;
  const at = (s) => `2026-10-01T00:00:${String(s).padStart(2, "0")}Z`;
  const m = (id, s) => ({ id, timestamp: at(s), content: `m${id}` });
  const canon = emptyCanon("c");
  canon.messages = [m("1", 1), m("2", 2), m("3", 3), m("4", 4)];
  const kept = canon.messages[2];
  // A newest page reaching back to 3 that says there is more: 4 is gone upstream; 1 and 2 are below it.
  mergeIntoCanon(canon, "messages", [m("3", 3), m("5", 5)], false, true, () => true);
  assert.deepStrictEqual(canon.messages.map((x) => x.id), ["1", "2", "3", "5"]);
  assert.equal(canon.messages[2], kept, "a copy held word for word did not keep the store's object");
  // ...but a message removed upstream below the window goes with the next newest page.
  canon.removed.set("1", 9);
  mergeIntoCanon(canon, "messages", [m("3", 3), m("5", 5)], false, true, () => true);
  assert.deepStrictEqual(canon.messages.map((x) => x.id), ["2", "3", "5"]);
  // An older page only adds; a newest page with nothing more answers for everything of its view.
  mergeIntoCanon(canon, "messages", [m("0", 0)], true, true, () => true);
  assert.deepStrictEqual(canon.messages.map((x) => x.id), ["0", "2", "3", "5"]);
  mergeIntoCanon(canon, "messages", [m("5", 5)], false, false, (x) => x.id !== "2");
  assert.deepStrictEqual(canon.messages.map((x) => x.id), ["2", "5"], "rows of another view were taken, or rows of this one kept");
  // A copy that lost its upstream address keeps the held one's.
  canon.messages = [{ ...m("7", 7), upstream_url: "https://example.com/7" }];
  mergeIntoCanon(canon, "messages", [{ ...m("7", 7), content: "edited" }], false, false, () => true);
  assert.deepStrictEqual([canon.messages[0].content, canon.messages[0].upstream_url], ["edited", "https://example.com/7"]);
  assert.deepStrictEqual([...heldCopies(canon, [{ id: "7" }, { id: "8" }]).keys()], ["7"]);
  // A thread summary replaces the one held by id, in time order.
  upsertThreadSummary(canon, { id: "t1", updated_at: at(5) });
  upsertThreadSummary(canon, { id: "t2", updated_at: at(3) });
  upsertThreadSummary(canon, { id: "t1", updated_at: at(1) });
  assert.deepStrictEqual(canon.threads.map((t) => t.id), ["t1", "t2"]);
});

test("`#624 voice-page-modularity`: the server's versions decide copies and absences, from the module alone", () => {
  const { serverVersions, pageCopyNewer, goneByAbsence, refreshFailed, exactCount, allPageAnswersFor, emptyCanon } = CHANNELS;
  assert.equal(serverVersions({}), null, "an older server's page has versions");
  assert.deepStrictEqual(serverVersions({ observed_ms: 10, settled_below_ms: 5 }), { observed: 10, settledBelow: 5 });
  assert.deepStrictEqual(serverVersions({ observed_ms: 10 }), { observed: 10, settledBelow: null });
  const versions = { observed: 100, settledBelow: Date.parse("2026-10-01T00:00:10Z") };
  assert.equal(pageCopyNewer(undefined, {}, versions), null);
  assert.equal(pageCopyNewer({ id: "1" }, {}, versions), null, "a copy that does not say how new it is was decided");
  assert.equal(pageCopyNewer({ observed_ms: 50, edited_timestamp: "2026-10-01T00:00:09Z" },
    { edited_timestamp: "2026-10-01T00:00:08Z" }, versions), false, "an earlier edit replaced a later one");
  assert.equal(pageCopyNewer({ observed_ms: 150 }, {}, versions), false);
  assert.equal(pageCopyNewer({ observed_ms: 50 }, {}, versions), true);
  assert.equal(goneByAbsence({ observed_ms: 50, timestamp: "2026-10-01T00:00:05Z" }, versions), true);
  assert.equal(goneByAbsence({ observed_ms: 150, timestamp: "2026-10-01T00:00:05Z" }, versions), false, "a copy newer than the read was taken away");
  assert.equal(goneByAbsence({ observed_ms: 50, timestamp: "2026-10-01T00:00:15Z" }, versions), false, "the unsettled band was taken as settled");
  assert.equal(goneByAbsence({ observed_ms: 50 }, { observed: 100, settledBelow: null }), false);
  assert.deepStrictEqual([refreshFailed({ refresh_failed: "x" }), refreshFailed({}), refreshFailed(null)], [true, false, false]);
  assert.deepStrictEqual([exactCount({ reply_count: 3 }), exactCount({ reply_count: 3, reply_count_exact: false }), exactCount(null)], [3, null, null]);
  // All's page answers for every reply of a thread it carries whole, and for the rest from its oldest carried reply up.
  const canon = emptyCanon("c");
  const t = (id, root, s, count) => ({ id, timestamp: `2026-10-01T00:00:0${s}Z`,
    thread: { id, is_root: root, reply_count: count ?? null, reply_count_exact: count !== undefined } });
  const page = [t("a", true, 1, 1), { ...t("a", false, 2), id: "a1" }, t("b", true, 3, 5), { ...t("b", false, 6), id: "b2" }];
  const answers = allPageAnswersFor(canon, page, true);
  assert.equal(answers({ ...t("a", false, 1), id: "a0" }), true, "a thread carried whole was not answered for");
  assert.equal(answers({ ...t("b", false, 4), id: "b1" }), false, "a reply below the oldest carried was answered for");
  assert.equal(answers({ ...t("c", false, 4), id: "c1" }), false, "a thread the page carries nothing of was answered for");
  canon.scope = "c";
  assert.equal(answers({ ...t("c", false, 4), id: "c1" }), true, "the scope was not read when the question was asked");
  // A removal as of the read numbered 5 is newer than reads up to 5, and older than read 6.
  const { removedSince, heldNow, deltaSideState } = CHANNELS;
  canon.removed.set("9", 5);
  assert.deepStrictEqual([removedSince(canon, "9", 5), removedSince(canon, "9", 6), removedSince(canon, "8", 0)], [true, false, false]);
  canon.messages = [{ id: 1 }, { id: "2" }];
  assert.deepStrictEqual([...heldNow(canon).keys()], ["1", "2"]);
  const before = deltaSideState(canon, { id: "t" });
  canon.replyGaps.set("t", 1);
  assert.notEqual(deltaSideState(canon, { id: "t" }), before, "a changed gap was not part of what a delta can change");
});

test("`#624 voice-page-modularity`: a view shows the store only from where its own pages reached, from the module alone", () => {
  const { emptyCanon, coverViewOf, projectionOf } = CHANNELS;
  const at = (s) => `2026-10-01T00:00:${String(s).padStart(2, "0")}Z`;
  const m = (id, s, thread) => ({ id, timestamp: at(s), ...(thread ? { thread } : {}) });
  const canon = emptyCanon("c");
  canon.messages = [m("1", 1), m("2", 2, { id: "t", is_root: false }), m("3", 3, { id: "t", is_root: true }), m("4", 4)];
  assert.equal(projectionOf(canon, "main", "main", null), null, "a view no page covered projected rows");
  // Main's newest page reached back to 3 and says there is more: 1 is below its floor, 2 is not Main's.
  coverViewOf(canon, "main", "main", [m("3", 3), m("4", 4)], false, true, "");
  const main = canon.views.get("main");
  assert.deepStrictEqual([main.floor, main.more, main.live, main.cursor, main.top], [Date.parse(at(3)), true, true, null, Date.parse(at(4))]);
  assert.deepStrictEqual(projectionOf(canon, "main", "main", null).map((x) => x.id), ["3", "4"]);
  // A step back to 1 lowers the floor; a refresh that stops short keeps the history already walked back to.
  coverViewOf(canon, "main", "main", [m("1", 1)], true, true, "");
  assert.equal(canon.views.get("main").floor, Date.parse(at(1)));
  coverViewOf(canon, "main", "main", [m("4", 4)], false, true, "");
  assert.equal(canon.views.get("main").floor, Date.parse(at(1)), "a refresh threw away history walked back to");
  assert.deepStrictEqual(projectionOf(canon, "main", "main", null).map((x) => x.id), ["1", "3", "4"]);
  // A newest page with nothing more: the whole view, and Main's own coverage proves nothing of All.
  coverViewOf(canon, "main", "main", [m("4", 4)], false, false, "a notice");
  assert.deepStrictEqual([canon.views.get("main").floor, canon.views.get("main").more, canon.views.get("main").notice], [null, false, "a notice"]);
  assert.equal(projectionOf(canon, "flat", "flat", null), null);
  coverViewOf(canon, "thread:t", "thread", [m("2", 2), m("3", 3)], false, false, "");
  assert.deepStrictEqual(projectionOf(canon, "thread:t", "thread", "t").map((x) => x.id), ["2", "3"]);
});

test("`#624 voice-page-modularity`: a channel's store saved and restored, from the module alone: what is kept, what is not, what is not trusted", () => {
  const { emptyCanon, savedTimelineOf, restoreCanon } = CHANNELS;
  const canon = emptyCanon("c");
  canon.messages = [{ id: "1", thread: { id: "t", is_root: true } }, { id: "2" }];
  canon.threads = [{ id: "t" }];
  canon.views.set("main", { floor: 5, more: true, notice: "", top: 9, dense: NaN, at: 7, live: true, cursor: "cur",
    newest: "next", fullAt: 3, fullSent: 2 });
  canon.dismissed = new Set(["1", "gone"]);
  canon.unplaced = new Set(["2", "gone"]);
  canon.removed = new Map([["2", 4], ["gone", 4]]);
  canon.replyGaps = new Map([["t", 3], ["other", 1]]);
  canon.passedOver = new Set(["t", "other"]);
  canon.settledByDone = new Map([["t", 2]]);
  canon.scope = null;
  const saved = savedTimelineOf(canon, 1234, { at: "2" });
  // Only what the store still holds, of messages and of threads; no cursor; a NaN is a null.
  assert.deepStrictEqual(saved.views, { main: { floor: 5, more: true, at: 7, notice: "", top: 9, dense: null } });
  assert.deepStrictEqual([saved.dismissed, saved.unplaced, saved.removed, saved.replyGaps, saved.passedOver, saved.settledByDone],
    [["1"], ["2"], ["2"], { t: 3 }, ["t"], { t: 2 }]);
  assert.deepStrictEqual([saved.mode, saved.usedAt, saved.bookmark], ["timeline", 1234, { at: "2" }]);
  assert.notEqual(saved.messages, canon.messages, "the save shares the store's list");
  const back = emptyCanon("c");
  restoreCanon(back, JSON.parse(JSON.stringify(saved)));
  assert.deepStrictEqual([...back.dismissed], ["1"]);
  assert.deepStrictEqual([...back.removed], [["2", 0]], "a removal came back with its old place in the order of reads");
  assert.deepStrictEqual(back.views.get("main"), { floor: 5, more: true, at: 7, notice: "", top: 9, dense: null, live: false, cursor: null });
  assert.deepStrictEqual([...back.replyGaps], [["t", 3]]);
  // A save from before passed-over threads and Done's settling were kept: its gaps, and All's cover, are not trusted.
  const old = { ...JSON.parse(JSON.stringify(saved)), views: { ...saved.views, flat: saved.views.main } };
  delete old.passedOver;
  delete old.settledByDone;
  const older = emptyCanon("c");
  restoreCanon(older, old);
  assert.deepStrictEqual([older.replyGaps.size, older.settledByDone.size, older.views.has("flat"), older.views.has("main")], [0, 0, false, true]);
});

test("`#624 voice-page-modularity`: the replies a store lacks for being old, by thread, from the module alone", () => {
  const { emptyCanon, noteReplyGapsIn, allReachesThread } = CHANNELS;
  const at = (s) => `2026-10-01T00:00:${String(s).padStart(2, "0")}Z`;
  const root = (id, s, count) => ({ id: `r${id}`, timestamp: at(s), thread: { id, is_root: true, reply_count: count, reply_count_exact: true } });
  const reply = (id, n, s) => ({ id: `${id}-${n}`, timestamp: at(s), thread: { id, is_root: false, reply_count: null, reply_count_exact: false } });
  const canon = emptyCanon("c");
  let asked = 0;
  const readAll = () => { asked += 1; };
  // A thread's own newest page with its count sets the gap: its count less the replies held.
  canon.messages = [root("t", 1, 5), reply("t", 1, 2), reply("t", 2, 3)];
  noteReplyGapsIn(canon, { messages: [], thread: { id: "t", reply_count: 5, reply_count_exact: true } }, "thread", false, false, new Map(), readAll);
  assert.equal(canon.replyGaps.get("t"), 3);
  // A step back that brings one more reply lowers it by one.
  canon.messages.push(reply("t", 3, 1));
  noteReplyGapsIn(canon, { messages: [reply("t", 3, 1)] }, "thread", true, false, new Map(), readAll);
  assert.equal(canon.replyGaps.get("t"), 2);
  // Main's newest page, with no read of All: no gap is set for a thread it is first to name, and All is read behind it.
  canon.messages.push(root("u", 6, 4));
  noteReplyGapsIn(canon, { messages: [root("u", 6, 4)] }, "main", false, false, new Map(), readAll);
  assert.deepStrictEqual([canon.replyGaps.has("u"), asked], [false, 1]);
  // A scoped channel, or the thread list, says nothing of gaps.
  canon.scope = "u";
  noteReplyGapsIn(canon, { messages: [root("u", 6, 4)] }, "main", false, false, new Map(), readAll);
  assert.equal(asked, 1, "a scoped channel asked for All");
  canon.scope = null;
  // All's cover reaches a thread whose root is no older than its floor, or All is the whole channel.
  assert.equal(allReachesThread(canon, "u", null), false, "All reached a thread with no read of All");
  canon.views.set("flat", { floor: Date.parse(at(4)), more: true, notice: "", top: null, dense: null, at: 0, live: true, cursor: null });
  assert.deepStrictEqual([allReachesThread(canon, "u", null), allReachesThread(canon, "t", null)], [true, false]);
  canon.views.get("flat").more = false;
  assert.equal(allReachesThread(canon, "t", null), true);
});

test("`#624 voice-page-modularity`: the copy-order ledger, from the module alone: where a copy stands, what it was built on, what says it as posted", () => {
  const { createCopyOrder, emptyCanon } = CHANNELS;
  const ledger = createCopyOrder();
  const read = { id: "1" };
  ledger.place(read, 5);
  assert.deepStrictEqual([ledger.copyOlderThan(read, "1", 6, new Map()), ledger.copyOlderThan(read, "1", 5, new Map())], [true, false]);
  // Later only: an older read does not move a copy back.
  ledger.orderCopy(read, 3);
  assert.equal(ledger.copyOlderThan(read, "1", 4, new Map()), false, "an older read moved the copy back");
  // A copy built on another stands where that one stood, and inherits "as posted".
  const live = { id: "2" };
  ledger.place(live, 2);
  ledger.markPosted(live);
  const counted = { id: "2", thread: {} };
  ledger.buildCopyOn(counted, live);
  assert.deepStrictEqual([ledger.isPosted(counted), ledger.copyOlderThan(counted, "2", 3, new Map())], [true, true]);
  assert.equal(ledger.copyOlderThan(counted, "2", 1, new Map(), true), true, "a posted copy was not older in what it says");
  // A read that brings the copy speaks for it from then on: no longer built on, as new as the read.
  const canon = emptyCanon("c");
  canon.messages = [counted];
  ledger.orderCopiesRead(canon, [{ id: "2" }], 9);
  assert.equal(ledger.copyOlderThan(counted, "2", 9, new Map()), false);
  // A copy with no place (saved on the device) is older exactly when the store held it as the read was sent.
  const saved = { id: "3" };
  assert.deepStrictEqual([ledger.copyOlderThan(saved, "3", 1, new Map([["3", saved]])), ledger.copyOlderThan(saved, "3", 1, new Map())], [true, false]);
  // Two ledgers share nothing.
  assert.equal(createCopyOrder().isPosted(live), false);
});

test("`#624 voice-page-modularity`: what a page may fold, from the module alone, with the page's questions injected", () => {
  const { emptyCanon, pageFoldIn, createCopyOrder } = CHANNELS;
  const canon = emptyCanon("c");
  const asked = [];
  const deps = (over = {}) => ({
    number: 5, ledger: createCopyOrder(),
    removedAfterRead: () => false,
    newerPageSpeaksFor: (_canon, message) => { asked.push(message.id); return message.id === "spoken"; },
    ownPostStands: (id) => id === "mine",
    ...over,
  });
  // No store as it was sent: every copy taken; every message answered for, but a fresh own post.
  const plain = pageFoldIn(canon, { messages: [] }, null, deps());
  assert.deepStrictEqual([plain.takes({ id: "1" }), plain.answers({ id: "1" }), plain.answers({ id: "mine" })], [true, true, false]);
  // A removal newer than the read: never taken, always answered for, whatever the page carries.
  canon.removed.set("gone", 7);
  assert.deepStrictEqual([plain.takes({ id: "gone" }), plain.answers({ id: "gone" })], [false, true]);
  // With the store as it was sent: a message never held is taken unless a newer page speaks for it.
  const late = pageFoldIn(canon, { messages: [] }, new Map(), deps());
  assert.deepStrictEqual([late.takes({ id: "new" }), late.takes({ id: "spoken" })], [true, false]);
  assert.deepStrictEqual(asked, ["new", "spoken"]);
  // The server's record of removals is asked too.
  const removedLate = pageFoldIn(canon, { messages: [] }, null, deps({ removedAfterRead: (_c, id) => id === "x" }));
  assert.deepStrictEqual([removedLate.takes({ id: "x" }), removedLate.answers({ id: "x" })], [false, true]);
});

test("`#624 voice-page-modularity`: the record of removals, from the module alone: by the order of reads and by the server's clock", () => {
  const { emptyCanon, createRemovals, forgetUnplaced } = CHANNELS;
  const removals = createRemovals();
  const canon = emptyCanon("c");
  canon.unplaced.add("1");
  canon.unplacedSince.set("1", 3);
  removals.noteRemoved(canon, "1", 4, 1000);
  assert.deepStrictEqual([canon.unplaced.has("1"), canon.unplacedSince.has("1"), canon.removed.get("1")], [false, false, 4]);
  // The server observed the removal at 1000: a page whose read began at or before then is older than it.
  assert.deepStrictEqual([removals.removedAfterRead(canon, "1", { observed: 900, settledBelow: null }),
    removals.removedAfterRead(canon, "1", { observed: 1100, settledBelow: null }), removals.removedAfterRead(canon, "1", null)], [true, false, false]);
  // A read sent after the removal that still carries the message believes it; one sent before does not.
  let asked = 0;
  removals.believeCarried(canon, { messages: [{ id: "1" }] }, () => { asked += 1; return 3; });
  assert.equal(canon.removed.has("1"), true, "a read sent before the removal brought the message back");
  removals.believeCarried(canon, { messages: [{ id: "1" }], observed_ms: 900 }, () => 9);
  assert.equal(canon.removed.has("1"), true, "a read begun before the server observed the removal brought it back");
  removals.believeCarried(canon, { messages: [{ id: "1" }], observed_ms: 1100 }, () => 9);
  assert.equal(canon.removed.has("1"), false, "a read sent after the removal that carries the message was not believed");
  assert.equal(removals.removedAfterRead(canon, "1", { observed: 900, settledBelow: null }), false, "the server's record of a believed message stayed");
  // Nothing removed: the read's place in the order is not even asked.
  removals.believeCarried(emptyCanon("d"), { messages: [{ id: "1" }] }, () => { asked += 1; return 0; });
  assert.equal(asked, 1);
  forgetUnplaced(canon, "nothing");
});

test("`#624 voice-page-modularity`: a delta folded into a store, from the module alone, the page's part injected", () => {
  const { emptyCanon, foldDeltaIn, createCopyOrder, createRemovals } = CHANNELS;
  const at = (s) => `2026-10-01T00:00:${String(s).padStart(2, "0")}Z`;
  const canon = emptyCanon("c");
  const held = { id: "1", timestamp: at(1), content: "a" };
  canon.messages = [held, { id: "2", timestamp: at(2), content: "b" }];
  canon.views.set("main", { floor: null, more: false, notice: "", top: null, dense: null, at: 0, live: false, cursor: null, newest: "cur" });
  const seen = [];
  const deps = {
    ledger: createCopyOrder(), removals: createRemovals(),
    numberOf: () => 7, readsSent: () => 6,
    forgetArchived: (id) => seen.push(["archived", id]),
    foldReadState: () => { seen.push(["read"]); return false; },
    noteReplyGaps: (before) => seen.push(["gaps", [...before.keys()]]),
    markUnchanged: () => seen.push(["unchanged"]),
    staleAsOf: () => null,
  };
  const delta = (over) => ({ messages: [], delta: { deleted: [], removed_threads: [], complete: true }, next_after: "next", ...over });
  // A cursor the cover no longer holds: nothing touched.
  assert.equal(foldDeltaIn(canon, delta({}), "old", "main", "main", deps), false);
  assert.deepStrictEqual(seen, []);
  // A deletion, an edit and a copy held word for word.
  const changed = delta({ messages: [{ ...held }, { id: "2", timestamp: at(2), content: "edited" }],
    delta: { deleted: ["9"], removed_threads: [], complete: true } });
  canon.messages.push({ id: "9", timestamp: at(9), content: "gone" });
  assert.equal(foldDeltaIn(canon, changed, "cur", "main", "main", deps), true);
  assert.deepStrictEqual(canon.messages.map((m) => m.content), ["a", "edited"]);
  assert.equal(canon.messages[0], held, "a copy held word for word lost the store's object");
  assert.deepStrictEqual([canon.removed.get("9"), canon.complete, canon.views.get("main").newest, canon.views.get("main").live], [6, true, "next", true]);
  assert.deepStrictEqual(seen, [["archived", "9"], ["read"], ["gaps", ["1", "2"]]], "the page's part came in another order");
  // A delta that changes nothing is marked so.
  seen.length = 0;
  assert.equal(foldDeltaIn(canon, delta({ messages: [] }), "next", "main", "main", deps), true);
  assert.deepStrictEqual(seen.map(([what]) => what), ["read", "gaps", "unchanged"]);
});

test("`#624 voice-page-modularity`: a live arrival folded into a store, from the module alone", () => {
  const { emptyCanon, foldLiveIn, createCopyOrder, namedRecord } = CHANNELS;
  const at = (s) => `2026-10-01T00:00:${String(s).padStart(2, "0")}Z`;
  const canon = emptyCanon("c");
  let arrivals = 0;
  const ledger = createCopyOrder();
  const deps = { ledger, orderNow: () => 4.5, nextArrival: () => ++arrivals };
  const root = { id: "r", timestamp: at(1), thread: { id: "t", is_root: true, reply_count: 2, reply_count_exact: true, display_name: "Plans", summary: "s" },
    reactions: [{ emoji: "x", count: 1 }], edited_timestamp: at(2), upstream_url: "https://example.com/r" };
  canon.messages = [root];
  canon.threads = [{ id: "t", updated_at: at(1), reply_count: 2, reply_count_exact: true }];
  // The stream's copy of the root, bare: it keeps the store's thread record, reactions, edit mark, address and name.
  const bare = { id: "r", timestamp: at(1), content: "root" };
  const kept = foldLiveIn(canon, bare, true, false, null, deps);
  assert.deepStrictEqual([kept.thread.id, kept.reactions.length, kept.edited_timestamp, kept.upstream_url, kept.thread.display_name],
    ["t", 1, at(2), "https://example.com/r", "Plans"]);
  assert.equal(ledger.isPosted(kept), true, "the stream's copy does not say the message as it was posted");
  // A new reply from the stream counts on its thread's summary and on the root's copy.
  const reply = { id: "1", timestamp: at(5), thread: { id: "t", is_root: false, reply_count: null, reply_count_exact: false } };
  foldLiveIn(canon, reply, true, false, null, deps);
  assert.deepStrictEqual([canon.threads[0].reply_count, canon.messages.find((m) => m.id === "r").thread.reply_count], [3, 3]);
  // ...but a replayed copy does not.
  foldLiveIn(canon, { ...reply, id: "2", timestamp: at(6) }, true, true, null, deps);
  assert.equal(canon.threads[0].reply_count, 3);
  // A bare live copy of a message no page placed is unplaced, numbered in the order of arrivals.
  foldLiveIn(canon, { id: "u", timestamp: at(7) }, true, false, null, deps);
  assert.deepStrictEqual([canon.unplaced.has("u"), canon.unplacedSince.get("u")], [true, 1]);
  assert.deepStrictEqual([namedRecord(root), namedRecord(reply), namedRecord(null)], [root.thread, null, null]);
});

test("`#624 voice-page-modularity`: an unplaced message placed by a read, and which newer pages speak for it, from the module alone", () => {
  const { emptyCanon, placeFromPageIn, newerPage, newerPageSpeaksFor, answeredThread, createCopyOrder } = CHANNELS;
  const canon = emptyCanon("c");
  const reply = { id: "1", timestamp: "2026-10-01T00:00:01Z", thread: { id: "t", is_root: false } };
  const bare = { id: "1", timestamp: "2026-10-01T00:00:01Z" };
  assert.deepStrictEqual([answeredThread(reply), answeredThread(bare), answeredThread({ id: "r", thread: { id: "t", is_root: true } })], ["t", null, null]);
  canon.views.set("thread:t", { floor: null, more: false, notice: "", top: null, dense: null, at: 0, live: true, cursor: null, fullSent: 5 });
  assert.deepStrictEqual([newerPage(canon, "thread", 4, "t"), newerPage(canon, "thread", 5, "t"), newerPage(canon, "main", 0)], [true, false, false]);
  assert.deepStrictEqual([newerPageSpeaksFor(canon, reply, 4), newerPageSpeaksFor(canon, bare, 4)], [true, false]);
  // The stream's bare copy, unplaced; a read's answer carrying its thread record places it.
  const ledger = createCopyOrder();
  canon.messages = [bare];
  canon.unplaced.add("1");
  assert.equal(placeFromPageIn({ messages: [reply] }, canon, 6, null, ledger), true);
  assert.deepStrictEqual([canon.unplaced.has("1"), canon.messages[0].thread.id], [false, "t"]);
  // Nothing unplaced: nothing placed.
  assert.equal(placeFromPageIn({ messages: [reply] }, canon, 6, null, ledger), false);
  // One the store lost, put back unless a newer page that speaks for it has landed.
  canon.messages = [];
  canon.unplaced.add("1");
  assert.equal(placeFromPageIn({ messages: [reply] }, canon, 4, null, ledger), false, "a reply a newer thread page speaks for came back");
  canon.unplaced.add("1");
  assert.equal(placeFromPageIn({ messages: [reply] }, canon, 6, null, ledger), true);
  assert.deepStrictEqual(canon.messages.map((m) => m.id), ["1"]);
});

test("`#624 voice-page-modularity`: a page folded into a store, from the module alone, the page's part injected in order", () => {
  const { emptyCanon, foldPageIn, createCopyOrder, createRemovals } = CHANNELS;
  const at = (s) => `2026-10-01T00:00:${String(s).padStart(2, "0")}Z`;
  const canon = emptyCanon("c");
  const seen = [];
  const deps = {
    ledger: createCopyOrder(), removals: createRemovals(), numberOf: () => 3,
    foldOf: (held) => { seen.push(["fold", held]); return { takes: () => true, answers: () => true }; },
    scopeChanged: () => seen.push(["scope"]),
    staleAsOf: () => null,
    foldReadState: () => seen.push(["read"]),
    noteReplyGaps: (before) => seen.push(["gaps", before.size]),
  };
  const page = { messages: [{ id: "1", timestamp: at(1) }, { id: "2", timestamp: at(2) }], has_more: true, has_threads: true,
    next_after: "cur", thread: { id: "t" } };
  foldPageIn(canon, page, false, "main", null, "main", null, deps);
  assert.deepStrictEqual(canon.messages.map((m) => m.id), ["1", "2"]);
  assert.deepStrictEqual([canon.scope, canon.hasThreads], ["t", true], "a page naming its thread did not scope the channel");
  const cover = canon.views.get("main");
  assert.deepStrictEqual([cover.newest, cover.fullSent, cover.more, cover.floor], ["cur", 3, true, Date.parse(at(1))]);
  assert.deepStrictEqual(seen, [["scope"], ["fold", null], ["read"], ["gaps", 0]], "the page's part came in another order");
  // A page the server answered from its own copy moves no cover, cursor or full read.
  seen.length = 0;
  foldPageIn(canon, { ...page, refresh_failed: "slow", next_after: "other" }, false, "main", null, "main", null, deps);
  assert.equal(canon.views.get("main").newest, "cur", "a failed refresh moved the forward cursor");
  assert.deepStrictEqual(seen.map(([what]) => what), ["fold", "read", "gaps"], "a scope that did not change asked for columns");
});

test("`#624 voice-page-modularity`: what a store holds of every thread's replies, and what their counts say, from the module alone", () => {
  const { emptyCanon, replyTallyOf, exactReplyCount, tallyRecord, unseenRepliesIn, emptyTally } = CHANNELS;
  const canon = emptyCanon("c");
  const root = { id: "r", thread: { id: "t", is_root: true, reply_count: 4, reply_count_exact: true } };
  const reply = (id) => ({ id, thread: { id: "t", is_root: false } });
  canon.messages = [root, reply("1"), reply("2")];
  canon.threads = [{ id: "t", reply_count: 3, reply_count_exact: true, root: { id: "r" } }];
  canon.dismissed = new Set(["1"]);
  const tally = replyTallyOf(canon, (message, done) => !done.has(String(message.id)));
  assert.deepStrictEqual(tally.threads.get("t"), { held: 2, unread: 1 });
  assert.equal(tally.byRoot.get("r").id, "t");
  // The highest exact count of summary, the store's record and the one given; a count not said exact is none.
  assert.equal(exactReplyCount("t", root, tally), 4);
  assert.equal(exactReplyCount("t", root, tally, { reply_count: 9, reply_count_exact: true }), 9);
  assert.equal(exactReplyCount("x", null, emptyTally()), null);
  // Unseen: the count less what is held and what is known old.
  assert.equal(unseenRepliesIn(canon, "t", root, tally), 2);
  canon.replyGaps.set("t", 1);
  assert.equal(unseenRepliesIn(canon, "t", root, tally), 1);
  // A root whose copy came without a record has its summary's.
  assert.deepStrictEqual(tallyRecord({ id: "r" }, { ...emptyTally(), byRoot: new Map([["r", { id: "t", reply_count: 3, reply_count_exact: true }]]) }),
    { id: "t", root_message_id: "r", is_root: true, reply_count: 3, reply_count_exact: true });
  // A scoped channel's replies are not a thread's.
  canon.scope = "t";
  assert.equal(replyTallyOf(canon, () => true).threads.has("t"), false);
});

test("`#624 voice-page-modularity`: the channel store's owner opens a channel afresh and answers the current store", () => {
  const { createChannelStore } = CHANNELS;
  const store = createChannelStore();
  const first = store.current();
  assert.equal(first.channel, "");
  first.messages.push({ id: "1" });
  const opened = store.open(42);
  assert.equal(store.current(), opened);
  assert.deepStrictEqual([opened.channel, opened.messages.length], ["42", 0], "a channel opened did not start afresh");
  assert.equal(store.open().channel, "");
  assert.notEqual(createChannelStore().current(), store.current(), "two owners share a store");
});

test("`#624 voice-page-modularity`: Done in the store at once, and taken back exactly when its write fails, from the module alone", () => {
  const { emptyCanon, markDoneIn, settleDoneIn } = CHANNELS;
  const unread = (message, done) => !done.has(String(message.id));
  const canon = emptyCanon("c");
  const root = { id: "r", thread: { id: "t", is_root: true, reply_count: 3, reply_count_exact: true } };
  canon.messages = [root, { id: "1", thread: { id: "t", is_root: false } }];
  canon.dismissed = new Set(["1"]);
  // Done on the root, from Main: the store at once, and its thread's unplaced replies settled.
  const mark = markDoneIn(canon, ["r", "1"], true, true, unread);
  assert.deepStrictEqual([mark.flipped, [...canon.dismissed].sort(), mark.settled], [["r"], ["1", "r"], [["t", 2]]]);
  assert.deepStrictEqual([canon.replyGaps.get("t"), canon.settledByDone.get("t")], [2, 2]);
  // The write failed: exactly what it changed goes back.
  settleDoneIn(mark, false);
  assert.deepStrictEqual([[...canon.dismissed], canon.replyGaps.has("t"), canon.settledByDone.has("t")], [["1"], false, false]);
  // Succeeded: nothing taken back.
  const kept = markDoneIn(canon, ["r"], true, true, unread);
  settleDoneIn(kept, true);
  assert.equal(canon.dismissed.has("r"), true);
  // Put back takes back what Done settled; failing, it is settled again.
  const back = markDoneIn(canon, ["r"], false, false, unread);
  assert.deepStrictEqual([canon.dismissed.has("r"), canon.replyGaps.has("t"), back.unsettled], [false, false, [["t", 2]]]);
  settleDoneIn(back, false);
  assert.deepStrictEqual([canon.dismissed.has("r"), canon.replyGaps.get("t")], [true, 2]);
  // No store for the channel on screen: nothing changed, nothing to take back.
  const none = markDoneIn(null, ["x"], true, true, unread);
  assert.deepStrictEqual([none.flipped, none.settled, none.unsettled, none.canon], [[], [], [], null]);
  settleDoneIn(none, false);
});

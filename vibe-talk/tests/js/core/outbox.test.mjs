// Tests for web/src/core/outbox/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The outbox: what waits to be sent, and what became of each send (web/src/core/outbox). */
const OUTBOX = await moduleForTests("src/core/outbox/index.ts");

test("`#624 voice-page-modularity`: when a lost send goes out again by itself, and the waits, from the module alone", () => {
  const { nextRetryAt, outgoingRetryWait, retriesItself, OUTGOING_RETRY_ATTEMPTS } = OUTBOX;
  const once = () => true;
  const never = () => false;
  assert.deepStrictEqual([0, 1, 2, 3, 4, 5].map(outgoingRetryWait), [3000, 6000, 12000, 20000, 20000, 20000]);
  const keyed = { keyedOnly: true, channel: "c" };
  assert.deepStrictEqual([retriesItself(keyed, once), retriesItself(keyed, never), retriesItself({ channel: "c" }, once)], [true, false, false]);
  assert.equal(retriesItself({ ...keyed, postedCount: 1, replyTo: "9" }, once), false, "a remainder starting with the reply went again");
  assert.equal(retriesItself({ ...keyed, postedCount: 1, replyTo: "9", continueIn: "t" }, once), true);
  // From when the failed attempt started, never sooner than the floor; none after the last attempt.
  assert.equal(nextRetryAt(keyed, 1000, 1500, once), 4000);
  assert.equal(nextRetryAt(keyed, 1000, 60000, once), 61000, "a slow failure waited a full wait again");
  assert.equal(nextRetryAt({ ...keyed, retriesUsed: OUTGOING_RETRY_ATTEMPTS }, 0, 0, once), null);
  assert.equal(nextRetryAt({ channel: "c" }, 0, 0, once), null, "a send that may post twice went again");
  assert.equal(nextRetryAt({ channel: "c" }, 0, 0, never, true), 3000, "an attempt that posted nothing did not go again");
});

test("`#624 voice-page-modularity`: the recently dismissed, kept, pruned and saved, from the module alone", () => {
  const { createDismissedSends, DISMISSED_SENDS_LIMIT, DISMISSED_SENDS_DAYS } = OUTBOX;
  const day = 24 * 60 * 60 * 1000;
  const saved = new Map();
  const record = { refuse: 0, roomMade: 0 };
  const storage = {
    getItem: (key) => saved.get(key) ?? null,
    // A full quota refuses the next `refuse` writes without a word; the read back is what notices.
    setItem: (key, value) => { if (record.refuse > 0) record.refuse -= 1; else saved.set(key, value); },
    removeItem: (key) => saved.delete(key),
  };
  let now = 100 * day;
  const owner = () => createDismissedSends({
    storage, now: () => now, makeRoom: () => { record.roomMade += 1; },
    decode: (entry) => (entry && typeof entry.id === "string" ? { ...entry } : null),
  });
  const sends = owner();
  assert.equal(sends.add({ id: "a", text: "one" }), true);
  now += 1;
  assert.equal(sends.add({ id: "b", text: "two" }), true);
  now += 1;
  assert.equal(sends.add({ id: "a", text: "one again" }), true);
  assert.deepStrictEqual(sends.list().map((held) => held.entry.text), ["one again", "two"], "not newest first, or a copy kept twice");
  assert.equal(sends.find("b").at, 100 * day + 1);
  assert.deepStrictEqual(JSON.parse(saved.get("vibe-talk.dismissed-sends")), sends.list());
  // Another page of the same device reads it back.
  const again = owner();
  again.load();
  assert.deepStrictEqual(again.list(), sends.list());
  // The quota full once: the saved rows go, and the second write stands.
  record.refuse = 1;
  assert.equal(sends.remove(sends.find("b")), true);
  assert.equal(record.roomMade, 1);
  record.refuse = 2;
  assert.equal(sends.add({ id: "c" }), false, "a copy the device refused was reported kept");
  assert.equal(record.roomMade, 2);
  // Only the newest DISMISSED_SENDS_LIMIT, none older than DISMISSED_SENDS_DAYS.
  for (let i = 0; i < DISMISSED_SENDS_LIMIT + 3; i += 1) sends.add({ id: `n${i}` });
  assert.equal(sends.list().length, DISMISSED_SENDS_LIMIT);
  assert.equal(sends.list()[0].entry.id, `n${DISMISSED_SENDS_LIMIT + 2}`);
  now += DISMISSED_SENDS_DAYS * day;
  sends.prune();
  assert.deepStrictEqual(sends.list(), [], "a send past its days was kept");
  assert.notEqual(saved.get("vibe-talk.dismissed-sends"), undefined, "a prune alone rewrote the device's copy");
  // Loading prunes, and saves what it pruned; a damaged entry or copy is nothing.
  const late = owner();
  late.load();
  assert.deepStrictEqual(late.list(), []);
  assert.equal(saved.has("vibe-talk.dismissed-sends"), false);
  saved.set("vibe-talk.dismissed-sends", JSON.stringify([{ entry: { id: "x" }, at: now }, { entry: null, at: now }, { entry: { id: "y" }, at: "soon" }]));
  late.load();
  assert.deepStrictEqual(late.list().map((held) => held.entry.id), ["x"]);
  saved.set("vibe-talk.dismissed-sends", "{not json");
  late.load();
  assert.deepStrictEqual(late.list(), []);
  assert.equal(late.clear(), true);
});

test("`#624 voice-page-modularity`: the page's own posts, kept in their grace and shared by the device's windows, from the module alone", () => {
  const { createOwnPosts, OWN_POST_GRACE_MS } = OUTBOX;
  const saved = new Map();
  let full = false;
  const storage = {
    getItem: (key) => saved.get(key) ?? null,
    setItem: (key, value) => { if (full) throw new Error("quota"); saved.set(key, value); },
    removeItem: (key) => saved.delete(key),
  };
  let now = 1000;
  const owner = () => createOwnPosts({ storage, now: () => now });
  const page = owner();
  page.noteSent(["a", "b"]);
  assert.deepStrictEqual(JSON.parse(saved.get("vibe-talk.own-posts")), { a: 1000, b: 1000 });
  // Another window adds its own to what this one saved; a reload takes back both.
  now = 2000;
  const other = owner();
  other.noteSent(["c"]);
  assert.deepStrictEqual(JSON.parse(saved.get("vibe-talk.own-posts")), { a: 1000, b: 1000, c: 2000 });
  const reloaded = owner();
  reloaded.load();
  assert.equal(reloaded.stands("c"), true);
  assert.equal(reloaded.sentAt("a"), 1000);
  assert.equal(reloaded.sentFromHere("a"), false, "a post of an earlier page counted as sent from this one");
  assert.equal(page.sentFromHere("a"), true);
  // A removal upstream: kept no longer, here or on the device.
  reloaded.removed("b");
  assert.equal(reloaded.stands("b"), false);
  assert.deepStrictEqual(Object.keys(JSON.parse(saved.get("vibe-talk.own-posts"))).sort(), ["a", "c"]);
  // Past the grace: forgotten as it is asked, though sent from this page it still is.
  now = 1000 + OWN_POST_GRACE_MS;
  assert.equal(page.stands("a"), false);
  assert.equal(page.sentAt("a"), undefined);
  assert.equal(page.sentFromHere("a"), true);
  const late = owner();
  late.load();
  assert.deepStrictEqual([late.stands("a"), late.stands("c")], [false, true]);
  // A full quota or a damaged copy costs only the device's copy.
  full = true;
  page.noteSent(["d"]);
  assert.equal(page.stands("d"), true);
  full = false;
  saved.set("vibe-talk.own-posts", "{not json");
  const damaged = owner();
  damaged.load();
  assert.equal(damaged.stands("c"), false);
  now += OWN_POST_GRACE_MS;
  page.removed("zz");
  damaged.noteSent([]);
  assert.equal(saved.has("vibe-talk.own-posts"), false, "nothing in its grace was still saved");
});

test("`#624 voice-page-modularity`: an outbox entry read back from the device, from the module alone", () => {
  const { outgoingFromSaved, savedWire } = OUTBOX;
  let asked = 0;
  const now = () => { asked += 1; return 5000; };
  const base = { id: "e1", channel: "c", text: " hi ", createdAt: 100, state: "failed", detail: "Refused." };
  for (const bad of [null, 7, "x", {}, { ...base, id: 1 }, { ...base, channel: "" }, { ...base, text: null }, { ...base, state: "posting" }]) {
    assert.equal(outgoingFromSaved(bad, now), null, `read back as an entry: ${JSON.stringify(bad)}`);
  }
  const failed = outgoingFromSaved(base, now);
  assert.deepStrictEqual(
    [failed.state, failed.remaining, failed.originView, failed.marksRead, failed.sendStartedAt, failed.detail, failed.mayHavePosted],
    ["failed", "hi", "main", null, 100, "Refused.", false]);
  assert.equal(asked, 0, "the clock was asked where no saved time was missing");
  // The page closed mid-send: only a POST that had started may have been delivered.
  const posting = outgoingFromSaved({ ...base, state: "sending", phase: "posting", createdAt: undefined }, now);
  assert.deepStrictEqual([posting.state, posting.mayHavePosted, posting.createdAt, posting.detail],
    ["unconfirmed", true, 5000, "This page closed before delivery was confirmed."]);
  for (const phase of ["queued", "placing"]) {
    const waiting = outgoingFromSaved({ ...base, state: "sending", phase }, now);
    assert.deepStrictEqual([waiting.state, waiting.mayHavePosted, waiting.detail],
      ["failed", false, "This page closed before this message was sent."]);
  }
  // An entry saved before marksRead marks what it answers; parts and wire keep only what is whole.
  const reply = outgoingFromSaved({ ...base, state: "sent", replyTo: "r", originView: "thread",
    parts: [{ id: "p", content: "a" }, { id: 2, content: "b" }, null], observedParts: ["p", 3],
    wire: { text: "w", replyTo: "", threadId: "t" }, serverMs: "12", upstreamMs: "soon", sentAt: 9 }, now);
  assert.deepStrictEqual(
    [reply.marksRead, reply.originView, reply.parts, reply.observedParts, reply.wire, reply.serverMs, reply.upstreamMs, reply.sentAt],
    ["r", "thread", [{ id: "p", content: "a" }], ["p"], { text: "w", replyTo: null, threadId: "t" }, 12, null, 9]);
  assert.equal(outgoingFromSaved({ ...base, replyTo: "r", marksRead: null }, now).marksRead, null);
  assert.equal(savedWire({ text: 3 }), null);
});

test("`#624 voice-page-modularity`: the message boxes' drafts, saved and taken back, from the module alone", () => {
  const { createChannelDrafts } = OUTBOX;
  const saved = new Map();
  let refuse = 0;
  let roomMade = 0;
  const storage = {
    getItem: (key) => saved.get(key) ?? null,
    setItem: (key, value) => { if (refuse > 0) refuse -= 1; else saved.set(key, value); },
    removeItem: (key) => saved.delete(key),
  };
  const make = () => createChannelDrafts({ storage, makeRoom: () => { roomMade += 1; } });
  const drafts = make();
  drafts.set('["c",null]', "hello");
  drafts.set('["c","t"]', "in a thread");
  assert.equal(saved.has("vibe-talk.channel-drafts"), false, "a set wrote to the device before a save");
  assert.equal(drafts.save(), true);
  assert.deepStrictEqual(JSON.parse(saved.get("vibe-talk.channel-drafts")), { '["c",null]': "hello", '["c","t"]': "in a thread" });
  // A full quota: the saved channel rows go once, and a second refusal is reported.
  drafts.delete('["c","t"]');
  refuse = 1;
  assert.equal(drafts.save(), true);
  assert.equal(roomMade, 1);
  refuse = 2;
  drafts.set('["d",null]', "lost");
  assert.equal(drafts.save(), false);
  assert.equal(roomMade, 2);
  // Another page takes back what the device kept; anything not a string is not a draft, and nor are blank
  // words (`#846 draft-chip-honest`).
  saved.set("vibe-talk.channel-drafts", JSON.stringify({ a: "kept", b: 3, c: "", d: " \n\t " }));
  const again = make();
  again.load();
  assert.deepStrictEqual([again.get("a"), again.get("b"), again.get("c"), again.get("d")], ["kept", undefined, undefined, undefined]);
  saved.set("vibe-talk.channel-drafts", "{not json");
  const damaged = make();
  damaged.load();
  assert.equal(damaged.get("a"), undefined);
});

test("`#624 voice-page-modularity`: what a send can do, and when its receipt goes, from the module alone", () => {
  const { editableSend, waitingItsTurn, unsentText, underDraft, sendSnippet, sentReceiptRetires, SENT_RECEIPT_MS, OWN_POST_GRACE_MS } = OUTBOX;
  const send = { id: "e", channel: "c", text: "all of it", parts: [], observedParts: [] };
  assert.equal(unsentText({ ...send, postedCount: 1, remaining: "the rest" }), "the rest");
  assert.equal(unsentText(send), "all of it");
  const queued = { ...send, state: "sending", phase: "queued" };
  assert.deepStrictEqual([waitingItsTurn(queued, false), waitingItsTurn(queued, true), waitingItsTurn({ ...queued, phase: "posting" }, false)],
    [true, false, false]);
  assert.deepStrictEqual([editableSend({ ...send, state: "failed" }, false), editableSend(queued, false), editableSend(queued, true)],
    [true, true, false]);
  assert.equal(editableSend({ ...send, state: "failed", mayHavePosted: true }, false), false, "a send that may have landed was editable");
  assert.equal(editableSend({ ...send, state: "unconfirmed" }, false), false);
  assert.deepStrictEqual([underDraft("draft  \n", "text"), underDraft("  ", "text"), underDraft(undefined, "text")],
    ["draft\n\ntext", "text", "text"]);
  assert.equal(sendSnippet("short  one"), "short one");
  assert.equal(sendSnippet("the quick brown fox jumps over the lazy dog"), "the quick brown fox jumps…");
  assert.equal(sendSnippet("abcdefghijklmnopqrstuvwxyz0123456789"), "abcdefghijklmnopqrstuvwxyz0…");
  // A receipt: gone once read back whole, or past the grace; where the store keeps own posts, once
  // shown long enough and every part is drawn or stored in the selected channel.
  let now = 1000;
  const view = (over = {}) => ({ now: () => now, keepsOwnPosts: () => true, selectedChannel: () => "c", storedIn: () => new Set(["p2"]), ...over });
  const sent = { ...send, state: "sent", sentAt: 1000, parts: [{ id: "p1" }, { id: "p2" }] };
  const none = new Set();
  assert.equal(sentReceiptRetires({ ...sent, state: "failed" }, none, view()), false);
  assert.equal(sentReceiptRetires({ ...sent, parts: [] }, none, view()), false);
  assert.equal(sentReceiptRetires({ ...sent, observedParts: ["p1", "p2"] }, none, view()), true);
  now = 1000 + SENT_RECEIPT_MS - 1;
  assert.equal(sentReceiptRetires(sent, new Set(["p1"]), view()), false, "a receipt went before it was shown long enough");
  now = 1000 + SENT_RECEIPT_MS;
  assert.equal(sentReceiptRetires(sent, new Set(["p1"]), view()), true);
  assert.equal(sentReceiptRetires(sent, none, view()), false, "a part neither drawn nor stored retired its receipt");
  assert.equal(sentReceiptRetires(sent, new Set(["p1"]), view({ keepsOwnPosts: () => false })), false);
  assert.equal(sentReceiptRetires(sent, new Set(["p1", "p2"]), view({ selectedChannel: () => "other" })), false);
  assert.equal(sentReceiptRetires(sent, new Set(["p1"]), view({ storedIn: () => null })), false);
  now = 1000 + OWN_POST_GRACE_MS;
  assert.equal(sentReceiptRetires(sent, none, view({ keepsOwnPosts: () => false })), true);
});

test("`#624 voice-page-modularity`: the outbox's entries, order, changes and saved copy, from the module alone", () => {
  const { createOutboxStore } = OUTBOX;
  const saved = new Map();
  let refuse = 0;
  let roomMade = 0;
  const storage = {
    getItem: (key) => saved.get(key) ?? null,
    setItem: (key, value) => { if (refuse > 0) refuse -= 1; else saved.set(key, value); },
    removeItem: (key) => saved.delete(key),
  };
  const outbox = createOutboxStore({ storage, makeRoom: () => { roomMade += 1; }, keepsOwnPosts: (channel) => channel === "threaded" });
  const a = { id: "a", channel: "flat", state: "failed", createdAt: 1 };
  const b = { id: "b", channel: "threaded", state: "sent", createdAt: 2 };
  const c = { id: "c", channel: "threaded", state: "sending", createdAt: 3 };
  outbox.add(a);
  outbox.add(c);
  outbox.add(b);
  assert.deepStrictEqual([...outbox.ids()], ["a", "c", "b"], "not in the order written");
  assert.equal(outbox.get("c"), c, "the outbox held a copy, not the page's own entry");
  assert.deepStrictEqual([outbox.has("b"), outbox.size(), outbox.version()], [true, 3, 0]);
  // A save moves the version and leaves out a confirmed send where the store keeps own posts.
  assert.equal(outbox.save(), true);
  assert.deepStrictEqual(JSON.parse(saved.get("vibe-talk.outgoing-messages")).map((entry) => entry.id), ["a", "c"]);
  assert.deepStrictEqual([outbox.version(), outbox.storageOkay()], [1, true]);
  outbox.touch();
  assert.equal(outbox.version(), 2);
  // Removing while walking passes the removed entry over; putting one back puts it where it was written.
  for (const entry of outbox.values()) if (entry.id === "a") outbox.remove("c");
  assert.deepStrictEqual([...outbox.ids()], ["a", "b"]);
  outbox.remove("a");
  outbox.putBackInOrder(a);
  outbox.putBackInOrder(c);
  assert.deepStrictEqual([...outbox.ids()], ["a", "b", "c"]);
  // A full quota gives up the saved rows once; a second refusal is reported, and remembered. (The
  // copy already there is cleared each time: a refused write that reads back the same copy stands.)
  saved.clear();
  refuse = 1;
  assert.equal(outbox.save(), true);
  assert.equal(roomMade, 1);
  saved.clear();
  refuse = 2;
  assert.equal(outbox.save(), false);
  assert.deepStrictEqual([roomMade, outbox.storageOkay()], [2, false]);
  assert.equal(outbox.save(), true);
  assert.deepStrictEqual(outbox.saved().map((entry) => entry.id), ["a", "c"]);
  outbox.refused();
  assert.equal(outbox.storageOkay(), false);
  saved.set("vibe-talk.outgoing-messages", "{not json");
  assert.throws(() => outbox.saved());
});

test("`#624 voice-page-modularity`: the reply drafts are kept by message id, and a new thread's tick only for this session", () => {
  const { createReplyDrafts, DRAFTS_KEY } = OUTBOX;
  assert.equal(DRAFTS_KEY, "vibe-talk.voice.drafts");
  const kept = new Map([[DRAFTS_KEY, JSON.stringify({ 101: "half a thought", 102: "", 103: 7, 105: "  \n " })]]);
  let full = false;
  let roomMade = 0;
  const storage = {
    getItem: (key) => kept.get(key) ?? null,
    setItem: (key, value) => { if (full) throw new Error("quota"); kept.set(key, value); },
    removeItem: (key) => kept.delete(key),
  };
  const drafts = createReplyDrafts({ storage, makeRoom: () => { roomMade += 1; full = false; } });
  drafts.load();
  assert.deepStrictEqual([drafts.has("101"), drafts.get("101"), drafts.has("102"), drafts.has("103"), drafts.has("105")],
    [true, "half a thought", false, false, false], "an empty, blank (#846 draft-chip-honest) or non-text draft was taken back");
  drafts.set("104", "another");
  drafts.delete("101");
  assert.equal(drafts.save(), true);
  assert.deepStrictEqual(JSON.parse(kept.get(DRAFTS_KEY)), { 104: "another" });
  full = true;
  assert.equal(drafts.save(), true, "the saved rows' room was not given up for the drafts");
  assert.equal(roomMade, 1);
  storage.setItem = () => { throw new Error("refused"); };
  assert.equal(drafts.save(), false, "a refused write said the drafts were kept");
  assert.equal(drafts.get("104"), "another", "a refused write lost the draft for this page");
  // The tick goes with the draft, only for this session.
  assert.equal(drafts.startsThread("104"), false);
  drafts.setStartsThread("104", true);
  assert.equal(drafts.startsThread("104"), true);
  drafts.setStartsThread("104", false);
  assert.equal(drafts.startsThread("104"), false);
  // A record that does not parse leaves none.
  const broken = createReplyDrafts({ storage: { getItem: () => "{not json", setItem() {}, removeItem() {} }, makeRoom() {} });
  broken.load();
  assert.equal(broken.has("101"), false);
});

test("`#624 voice-page-modularity`: the threads sends in the outbox go into, counted once for each change", () => {
  const { createPendingThreads } = OUTBOX;
  let version = 1;
  let channel = "c";
  let threadReplies = true;
  let worked = 0;
  const sends = [
    { channel: "c", threadId: "t1", remaining: 1, createdAt: 10 },
    { channel: "c", continueIn: "t2", remaining: 1, createdAt: 30 },
    { channel: "c", replyTo: "m1", remaining: 1, createdAt: 20 },
    { channel: "c", threadId: "t1", remaining: 1, createdAt: 40, state: "sent" },
    { channel: "c", threadId: "t1", remaining: 0, createdAt: 50 },
    { channel: "c", threadId: "t1", remaining: 1, createdAt: 60, echoed: true },
    { channel: "d", threadId: "t1", remaining: 1, createdAt: 70 },
    { channel: "c", replyTo: "m2", remaining: 1, createdAt: 80 },
  ];
  const store = { channel: "c", messages: [{ id: "m1", thread: { id: "t1" } }, { id: "m2" }] };
  const pending = createPendingThreads({
    channel: () => channel,
    outboxVersion: () => version,
    sends: () => { worked += 1; return sends; },
    store: () => store,
    repliesInThread: () => threadReplies,
    threadRecordOf: (message) => message.thread || null,
    idOf: (message) => message.id,
  });
  assert.equal(pending.threadOf(sends[2]), "t1", "a thread reply did not go into its message's thread");
  assert.equal(pending.threadOf(sends[7]), null, "a message with no thread record was given one");
  assert.deepStrictEqual([pending.into("t1"), pending.into("t2"), pending.into("t9")], [2, 1, 0]);
  assert.deepStrictEqual([...pending.times()], [["t1", 20], ["t2", 30]]);
  assert.equal(pending.key(), "c t1=2 t2=1");
  // `#624 voice-page-modularity`: the rows passed over for it once, and not again until the key moves.
  assert.deepStrictEqual([pending.noteDrawn(), pending.noteDrawn()], [true, false]);
  pending.into("t1");
  assert.equal(worked, 1, "the counts were worked out again with nothing changed");
  version = 2;
  threadReplies = false;
  assert.equal(pending.into("t1"), 1, "a reply on a service without thread replies counted in a thread");
  assert.equal(worked, 2);
  channel = "d";
  assert.equal(pending.into("t1"), 1);
  assert.equal(pending.key(), "d t1=1");
});

test("`#624 voice-page-modularity`: a dismissed send's Undo is offered for SEND_UNDO_MS, one at a time", () => {
  const { createSendUndo, SEND_UNDO_MS } = OUTBOX;
  const timers = [];
  let ended = 0;
  const undo = createSendUndo({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    ended: () => { ended += 1; },
  });
  assert.equal(undo.current(), null);
  undo.offer({ id: "o1", hold: null, at: 1 });
  assert.deepStrictEqual({ ...undo.current(), timer: undefined }, { id: "o1", hold: null, at: 1, timer: undefined });
  assert.equal(timers[0].ms, SEND_UNDO_MS);
  undo.offer({ id: "o2", hold: { at: 5 }, at: 2 });
  assert.equal(timers[0], null, "a new offer replaces the one up");
  assert.equal(undo.current().id, "o2");
  timers[1].run();
  assert.equal(undo.current(), null, "the chip's time ran out");
  assert.equal(ended, 1);
  undo.offer({ id: "o3", hold: null, at: 3 });
  undo.end();
  assert.equal(timers[2], null);
  assert.equal(ended, 2);
});

test("`#819 voice-page-quoted-reply-and-draft-chip`: the draft ledger finds a channel's live drafts newest first, across a reload", () => {
  const { createDraftLedger, createChannelDrafts, DRAFT_LEDGER_KEY, QUICK_DRAFTS_KEY } = OUTBOX;
  const saved = new Map();
  const storage = { getItem: (key) => (saved.has(key) ? saved.get(key) : null), setItem: (key, value) => saved.set(key, String(value)), removeItem: (key) => saved.delete(key) };
  let clock = 1000;
  const ledger = createDraftLedger({ storage, makeRoom() {}, now: () => clock });
  ledger.touch("composer", '["c1",null]', "c1");
  clock = 2000;
  ledger.touch("reply", "m1", "c1");
  clock = 3000;
  ledger.touch("quick", "c1\nm2\nwords", "c1");
  ledger.touch("reply", "m9", "c2");
  const live = new Set(['["c1",null]', "m1", "c1\nm2\nwords", "m9"]);
  assert.deepStrictEqual(ledger.inChannel("c1", (entry) => live.has(entry.key)).map((entry) => entry.kind), ["quick", "reply", "composer"]);
  live.delete("m1");
  assert.deepStrictEqual(ledger.inChannel("c1", (entry) => live.has(entry.key)).map((entry) => entry.key), ["c1\nm2\nwords", '["c1",null]'],
    "a draft its store no longer holds is not offered");
  ledger.drop("quick", "c1\nm2\nwords");
  assert.equal(ledger.has("quick", "c1\nm2\nwords"), false);
  ledger.prune((entry) => entry.key !== "m9");
  assert.equal(ledger.has("reply", "m9"), false, "a draft its store no longer holds is let go of");
  assert.equal(ledger.has("reply", "m1"), true);
  assert.equal(ledger.save(), true);
  assert.ok(saved.has(DRAFT_LEDGER_KEY));
  const again = createDraftLedger({ storage, makeRoom() {}, now: () => 0 });
  again.load();
  assert.deepStrictEqual(again.inChannel("c1", () => true).map((entry) => [entry.kind, entry.at]), [["reply", 2000], ["composer", 1000]]);
  saved.set(DRAFT_LEDGER_KEY, "{broken");
  const broken = createDraftLedger({ storage, makeRoom() {}, now: () => 0 });
  broken.load();
  assert.deepStrictEqual(broken.inChannel("c1", () => true), [], "a damaged record leaves an empty ledger");
  // The quick replies' drafts: the boxes' kind of store under their own key.
  const quick = createChannelDrafts({ storage, makeRoom() {}, key: QUICK_DRAFTS_KEY });
  quick.set("c1\nm2\nwords", "half a thought");
  assert.equal(quick.save(), true);
  assert.deepStrictEqual(JSON.parse(saved.get(QUICK_DRAFTS_KEY)), { "c1\nm2\nwords": "half a thought" });
  assert.deepStrictEqual(quick.keys(), ["c1\nm2\nwords"]);
});

test("`#819 voice-page-quoted-reply-and-draft-chip`: the Draft chip's round goes newest first, on from the last, and starts again in another channel", () => {
  const { createDraftRound } = OUTBOX;
  const round = createDraftRound();
  const drafts = [{ kind: "reply", key: "m2" }, { kind: "quick", key: "c1\nm1\nq" }, { kind: "composer", key: '["c1",null]' }];
  assert.equal(round.next([], "c1"), null);
  assert.deepStrictEqual([0, 1, 2, 3].map(() => round.next(drafts, "c1").key), ["m2", "c1\nm1\nq", '["c1",null]', "m2"]);
  assert.equal(round.next(drafts, "c2").key, "m2", "another channel starts at its newest");
  assert.equal(round.next(drafts.slice(1), "c2").key, "c1\nm1\nq", "the last one gone, the round starts at the newest");
  // `#846 draft-chip-honest`. The drafts on screen are passed over, and the round keeps its place among them all:
  // the one just opened is on screen, and the next tap goes on from it, not back to the newest.
  const offered = createDraftRound();
  let onScreen = new Set(['["c1",null]']);
  const offers = (draft) => !onScreen.has(draft.key);
  assert.equal(offered.next(drafts, "c1", offers).key, "m2");
  onScreen = new Set(["m2"]);
  assert.equal(offered.next(drafts, "c1", offers).key, "c1\nm1\nq", "the round went back to the newest");
  onScreen = new Set(["m2", "c1\nm1\nq"]);
  assert.equal(offered.next(drafts, "c1", offers).key, '["c1",null]', "a draft off screen again was not offered");
  onScreen = new Set(drafts.map((draft) => draft.key));
  assert.equal(offered.next(drafts, "c1", offers), null, "with every draft on screen there is nothing to offer");
});

test("`#624 voice-page-modularity`: the outbox pump runs one pass at a time, going round again for a pass asked during one", () => {
  const { createOutboxPump, createSendNumbers } = OUTBOX;
  const pump = createOutboxPump();
  const passes = [];
  let asks = 2;
  const pass = () => {
    passes.push(passes.length);
    if (asks > 0) {
      asks -= 1;
      pump.pump(() => passes.push("nested"));
    }
  };
  pump.pump(pass);
  assert.deepStrictEqual(passes, [0, 1, 2], "a pass asked during one made it go round, never nesting");
  assert.throws(() => pump.pump(() => { throw new Error("start failed"); }), /start failed/);
  pump.pump(() => passes.push("after"));
  assert.equal(passes.at(-1), "after", "a throw leaves the pump idle for the next");
  const numbers = createSendNumbers();
  assert.deepStrictEqual([numbers.next(), numbers.next()], [1, 2]);
});

test("unconfirmed sends: every id in the outbox asked once per load, a later call asks nothing", () => {
  const ids = new Set(["a", "b"]);
  const asked = [];
  const sends = OUTBOX.createUnconfirmedSends({
    ids: () => ids.values(),
    settle: (id) => {
      asked.push(id);
      // An id that turns up during the walk is not part of it: the walk is over the ids as they were.
      if (id === "a") ids.add("a2");
    },
  });
  sends.settleOnce();
  assert.deepStrictEqual(asked, ["a", "b"], "a copy of the ids is walked, so an entry added by an ask is not asked about");
  ids.add("c");
  sends.settleOnce();
  assert.deepStrictEqual(asked, ["a", "b"], "once per load");
});

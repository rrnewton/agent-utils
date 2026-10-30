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
  // Another page takes back what the device kept; anything not a string is not a draft.
  saved.set("vibe-talk.channel-drafts", JSON.stringify({ a: "kept", b: 3, c: "" }));
  const again = make();
  again.load();
  assert.deepStrictEqual([again.get("a"), again.get("b"), again.get("c")], ["kept", undefined, ""]);
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

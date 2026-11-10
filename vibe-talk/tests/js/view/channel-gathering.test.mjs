// Tests for web/src/view/channel/messagedetails.ts and startsread.ts on their own (`#624
// voice-page-modularity`): the details sheet's lines and bookmark, whether the reader's
// own messages start read, and what is still to do. The page over the same factories is
// in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");
const PRESENT_TIME = await moduleForTests("src/present/time/index.ts");
const ANNOTATIONS = await moduleForTests("src/core/annotations/index.ts");

function fakeElement(tag) {
  return {
    tag, className: "", textContent: "", children: [], listeners: {},
    setAttribute(name, value) { this[name] = value; },
    append(...kids) { this.children.push(...kids); },
    addEventListener(kind, fn) { this.listeners[kind] = fn; },
  };
}
globalThis.document = { createElement: (tag) => fakeElement(tag) };

const message = (extra = {}) => ({
  id: "m1", channel_id: "c1", author: "Ann", author_id: "a1", author_is_bot: false,
  content: "hi", timestamp: "2026-01-01T00:00:00Z", spoken_time: "midnight", reply_to: null, ...extra,
});

test("the details sheet names who and when, every id, and bookmarks above the row", () => {
  const calls = [];
  const details = CHANNEL.createMessageDetails({
    bookmarkAbove: () => false,
    toggleBookmarkAt: () => calls.push("toggleBookmark"),
    inThisColumn: (act) => act,
  });
  const row = fakeElement("li");
  row.removeChild = (kid) => { row.children = row.children.filter((held) => held !== kid); };
  details.toggleMessageDetails(row, [message(), message({ id: "m2" })]);
  assert.equal(row.children.length, 1);
  const sheet = row.children[0];
  assert.equal(sheet.className, "msg-details");
  assert.deepEqual(sheet.children.map((kid) => kid.textContent), [
    "Ann",
    sheet.children[1].textContent,
    "id m1 + m2",
    "Bookmark above",
  ]);
  assert.equal(sheet.children[1].textContent, PRESENT_TIME.fullLocalTime(message()), "the when line is the full local time");
  sheet.children[3].listeners.click();
  assert.deepEqual(calls, ["toggleBookmark"]);
  assert.equal(row.children.length, 0, "bookmarking closes the sheet");
  details.toggleMessageDetails(row, [message({ author_is_bot: true })]);
  assert.equal(row.children[0].children[0].textContent, "Ann (bot)");
  details.toggleMessageDetails(row, [message()]);
  assert.equal(row.children.length, 0, "a second toggle takes the sheet away");
});

function makeStarts() {
  const state = {
    on: true, choice: undefined, owner: false, self: false, agents: new Set(),
    threading: true, channel: "c1", held: null, hidden: new Set(),
  };
  const readLedger = ANNOTATIONS.createReadLedger();
  const canon = { channel: "c1", dismissed: new Set() };
  const done = { archived: new Set() };
  const starts = CHANNEL.createStartsRead({
    ownReadSwitch: { on: () => state.on },
    whoIsWho: () => ({
      choiceOf: () => state.choice,
      isOwnerId: () => state.owner,
      isSelfId: () => state.self,
      codingAgentsOf: () => ({ ids: state.agents }),
    }),
    messageChannel: (m) => m.channel_id,
    providerFacts: { threading: () => state.threading },
    heldMessage: () => state.held,
    readLedger,
    selectedChannelId: () => state.channel,
    channelStore: { current: () => canon, open: () => canon },
    doneState: () => done,
    hiddenEmpty: (m) => state.hidden.has(String(m.id)),
  });
  return { starts, state, canon, done, readLedger };
}

test("own messages start read by the server's own order, and only with the switch on", () => {
  const { starts, state } = makeStarts();
  assert.equal(starts.startsRead(message()), false, "nobody has said this one is his");
  state.choice = "me";
  assert.equal(starts.startsRead(message()), true);
  state.choice = "agent";
  assert.equal(starts.startsRead(message()), false, "who-is-who's choice comes first");
  state.choice = undefined; state.owner = true;
  assert.equal(starts.startsRead(message()), true);
  state.owner = false; state.self = true;
  assert.equal(starts.startsRead(message()), true);
  state.agents = new Set(["a1"]);
  assert.equal(starts.startsRead(message()), false, "the bridge's account named a coding agent does not count");
  state.on = false; state.agents = new Set();
  assert.equal(starts.startsRead(message()), false, "the switch off changes the start, nothing else");
  assert.equal(starts.startsRead(null), false);
});

test("noting a start draws it read in the store and Done marks, once", () => {
  const { starts, state, canon, done, readLedger } = makeStarts();
  state.self = true;
  starts.noteStartsRead([message(), message({ id: "m2", channel_id: "c2" })]);
  assert.deepEqual([...canon.dismissed], ["m1"], "only the channel on screen goes into its store");
  assert.deepEqual([...done.archived], ["m1"], "another channel's own message is seeded, not marked");
  assert.equal(readLedger.written("m2"), true, "its start is held as a write made here");
  assert.equal(readLedger.written("m1"), true, "the start is held as a write made here");
  canon.dismissed.clear(); done.archived.clear();
  starts.noteStartsRead([message()]);
  assert.deepEqual([...done.archived], [], "a message already written of is not started again");
});

test("still to do is not done, not noise and not hidden away", () => {
  const { starts, state, done } = makeStarts();
  assert.equal(starts.stillToDo(message()), true);
  done.archived.add("m1");
  assert.equal(starts.stillToDo(message()), false);
  assert.equal(starts.stillToDo(message(), new Set()), true, "another read state can be asked");
  state.hidden.add("m1");
  assert.equal(starts.stillToDo(message(), new Set()), false, "a hidden empty message is not to do");
});

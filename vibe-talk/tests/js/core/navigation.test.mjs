// Tests for web/src/core/navigation/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** Where the reader is: the selection's one owner (web/src/core/navigation). */
const NAVIGATION = await moduleForTests("src/core/navigation/index.ts");

// --- the selection's owner (`#624 voice-page-modularity`) ----------------------------------------

test("`#624 voice-page-modularity`: the selection starts on no channel, and every choice reaches every subscriber in order, read-only", () => {
  const navigation = NAVIGATION.createNavigation();
  assert.deepStrictEqual(navigation.snapshot(), { channelId: "" });
  const heard = [];
  navigation.subscribe((snapshot) => heard.push(["first", snapshot.channelId]));
  const stop = navigation.subscribe((snapshot) => heard.push(["second", snapshot.channelId]));
  navigation.selectChannel("200");
  navigation.selectChannel("200");
  stop();
  navigation.selectChannel("300");
  assert.deepStrictEqual(heard, [["first", "200"], ["second", "200"], ["first", "200"], ["second", "200"], ["first", "300"]],
    "a choice of the channel already chosen must still redraw what shows it");
  const snapshot = navigation.snapshot();
  assert.equal(snapshot.channelId, "300");
  assert.throws(() => { "use strict"; snapshot.channelId = "400"; }, TypeError, "a snapshot could be written to");
  assert.equal(navigation.snapshot().channelId, "300");
});

// --- places and their addresses (`#423 back-key-consistency`, `#462 webapp-deep-links`) ----------

test("`#624 voice-page-modularity`: a place's key ignores where a thread came from, and Settings, the list and a list's thread are over another", () => {
  const { visitKey, placeIsOver } = NAVIGATION;
  const place = (fields) => ({ screen: "main", channel: "C1", layout: "one", view: "main", thread: null, origin: null, side: null, ...fields });
  assert.equal(visitKey(null), "");
  assert.equal(visitKey(place({ view: "thread", thread: "T1", origin: "list" })), visitKey(place({ view: "thread", thread: "T1", origin: "main" })));
  assert.equal(visitKey({ screen: "help", channel: "C1", under: place({}), from: "keys", entry: null }), "help:keys:one:main::");
  assert.equal(placeIsOver(place({})), false);
  assert.equal(placeIsOver(place({ layout: "list" })), true);
  assert.equal(placeIsOver(place({ layout: "two", side: null })), false);
  assert.equal(placeIsOver(place({ layout: "two", side: "T1" })), true);
  assert.equal(placeIsOver(place({ view: "thread", thread: "T1", origin: "list" })), true);
  assert.equal(placeIsOver({ screen: "settings", channel: "C1", under: null, from: null, entry: null }), true);
});

test("`#624 voice-page-modularity`: a place reads back from the query it writes, and a bad escape or an absurd value says nothing", () => {
  const { deepLinkParams, deepLinkPart, deepLinkQuery, DEEP_LINK_VIEWS } = NAVIGATION;
  const ref = { provider: "slack", channel: "C 1", message: "M1", thread: "T1" };
  const query = deepLinkQuery({ screen: "main", channel: "C 1", layout: "one", view: "thread", thread: "T1", origin: null, side: null }, ref);
  assert.equal(query, "c=C%201&v=thread&t=T1&m=slack%3AC%25201%3AM1%3AT1", "the ref's parts are escaped, then the whole value again");
  const params = deepLinkParams(`?${query}`);
  assert.equal(params.get("c"), "C 1");
  assert.equal(params.get("v"), "thread");
  assert.ok(DEEP_LINK_VIEWS.includes(params.get("v")));
  assert.equal(deepLinkQuery({ screen: "main", channel: "C1", layout: "list", view: null, thread: null, origin: null, side: null }, ref), "c=C1&v=threads",
    "a ref to another channel is not carried");
  assert.equal(deepLinkQuery({ screen: "main", channel: "C1", layout: "one", view: "flat", thread: null, origin: null, side: null }), "c=C1&v=all");
  const odd = deepLinkParams("a=%E0%A4%A&b=x+y&c&d=" + "z".repeat(600));
  assert.equal(odd.has("a"), false);
  assert.equal(odd.get("b"), "x y");
  assert.equal(deepLinkPart(odd, "c"), null);
  assert.equal(deepLinkPart(odd, "d"), null);
  assert.equal(deepLinkPart(odd, "b"), "x y");
});

test("`#624 voice-page-modularity`: the UI-state record read back, checked, brought forward and merged, and the views chosen per channel", () => {
  const { createUiState, UI_STATE_KEY, UI_STATE_CHANNELS } = NAVIGATION;
  const store = (entries = {}) => {
    const kept = new Map(Object.entries(entries));
    return { kept, getItem: (key) => (kept.has(key) ? kept.get(key) : null), setItem: (key, value) => kept.set(key, value), removeItem: (key) => kept.delete(key) };
  };
  const record = (fields) => JSON.stringify({ v: 2, identity: "me", view: "discord", todo: false, channel: "C1", place: null, channels: [], ...fields });
  let identity = "me";
  const owner = (device, window) => createUiState({
    deviceStorage: () => device, windowStorage: () => window, identity: () => identity,
    viewKey: (view, thread) => (thread ? `${view}:${thread}` : view),
  });
  // Nothing kept: nothing to reopen on, and settled at once.
  const empty = owner(store(), store());
  assert.equal(empty.saved(), null);
  assert.equal(empty.settled(), true);
  // The window's own record first, the device's choices behind its own, the device's read mode.
  const device = store({ [UI_STATE_KEY]: record({ todo: true, read: "hide", channels: [{ channel: "C2", channelView: "main", thread: null }] }) });
  const window = store({ [UI_STATE_KEY]: record({ channels: [{ channel: "C1", channelView: "flat", thread: null }, { channel: "", channelView: "main", thread: null }] }) });
  const merged = owner(device, window);
  assert.equal(merged.settled(), false);
  assert.equal(merged.saved().todo, true);
  assert.equal(merged.saved().read, "hide");
  assert.deepStrictEqual(merged.saved().channels.map((one) => one.channel), ["C1", "C2"], "a damaged choice was kept, or the device's were lost");
  // Another credential's record, or one that does not parse, is dropped from its store.
  const theirs = store({ [UI_STATE_KEY]: record({ identity: "them" }) });
  assert.equal(owner(theirs, store()).saved(), null);
  assert.equal(theirs.kept.has(UI_STATE_KEY), false);
  const broken = store({ [UI_STATE_KEY]: "{" });
  owner(broken, store());
  assert.equal(broken.kept.has(UI_STATE_KEY), false);
  // A version 1 record is brought forward: the channel's view kept where chosen, the place under its view key.
  const old = store({ [UI_STATE_KEY]: JSON.stringify({ v: 1, identity: "me", view: "discord", todo: false, channel: "C3",
    channelView: "thread", chosen: true, thread: { id: "T1", origin: "flat", summary: null }, place: { atNewest: false, anchor: "9", offset: 4 } }) });
  const forward = owner(old, store()).saved();
  assert.equal(forward.v, 2);
  assert.equal(forward.place.view, "thread:T1");
  assert.deepStrictEqual(forward.channels.map((one) => one.channel), ["C3"]);
  // The views chosen: the newest first, one per channel, a change of credential forgetting them.
  const choices = owner(store(), store());
  for (let at = 0; at < UI_STATE_CHANNELS + 3; at += 1) choices.remember({ channel: `C${at}`, channelView: "main", thread: null });
  choices.remember({ channel: "C5", channelView: "flat", thread: null });
  assert.equal(choices.chosenViewsNow().length, UI_STATE_CHANNELS);
  assert.equal(choices.chosenView("C5").channelView, "flat");
  assert.equal(choices.chosenViewsNow()[0].channel, "C5");
  choices.forgetChosen((channel) => channel === "C5");
  assert.equal(choices.chosenView("C5"), undefined);
  identity = "someone else";
  assert.deepStrictEqual(choices.chosenViewsNow(), []);
  // Forgotten on signing out, in both stores.
  identity = "me";
  const kept = store({ [UI_STATE_KEY]: record({}) });
  const signedOut = owner(kept, store());
  signedOut.holdPlace({ key: "k", place: { view: "main", atNewest: true, anchor: null, offset: 0 } });
  signedOut.forget();
  assert.equal(signedOut.saved(), null);
  assert.equal(signedOut.heldPlace(), null);
  assert.equal(signedOut.settled(), true);
  assert.equal(kept.kept.has(UI_STATE_KEY), false);
});

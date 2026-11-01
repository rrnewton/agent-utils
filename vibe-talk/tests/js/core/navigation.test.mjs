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

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

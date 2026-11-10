// Tests for web/src/view/voice/watchframe.ts on its own (`#624 voice-page-modularity`):
// what one frame of the stream of every channel means — a reset, settings, a message for
// the call, and the page's own send settling. The page over the same factory is in
// ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const VOICE = await moduleForTests("src/view/voice/index.ts");

function makeFrames() {
  const calls = [];
  const state = { canSend: true, relay: "on", resumed: false };
  const owner = VOICE.createWatchFrame({
    watchAll: () => ({
      start: () => calls.push("start"),
      stop: () => calls.push("stop"),
      resumed: () => state.resumed,
    }),
    refreshServerSettings: async () => { calls.push("refreshSettings"); },
    voiceCall: () => ({
      canSendText: () => state.canSend,
      reportUnrelayed: (ids, why) => calls.push(["unrelayed", ids, why]),
      relayToAgent: (message, selfPosted, replayed, options) =>
        calls.push(["relay", message.id, selfPosted, replayed, options]),
    }),
    relayMode: () => state.relay,
    onSettingsFrame: () => calls.push("settings"),
    noteSelfAuthor: (id) => calls.push(["selfAuthor", id]),
    settleFromServer: (key) => calls.push(["settle", key]),
  });
  return { owner, calls, state };
}

const messageFrame = (extra = {}) => ({
  event: "message",
  data: JSON.stringify({
    message: { id: "m1", channel_id: "c1", author: "Ann", author_id: "a1", author_is_bot: false, content: "hi", timestamp: "2026-01-01T00:00:00Z", spoken_time: "midnight", reply_to: null },
    self_posted: false, replayed: false, from_tail: false, untrusted_content_notice: "untrusted", ...extra,
  }),
});

test("start and stop follow the stream handed in", () => {
  const { owner, calls } = makeFrames();
  owner.startWatchAll();
  owner.stopWatchAll();
  assert.deepEqual(calls, ["start", "stop"]);
});

test("a reset refreshes settings and reports the gap to a call that can hear it", () => {
  const { owner, calls, state } = makeFrames();
  assert.equal(owner.onWatchAllFrame({ event: "reset", data: "" }), "reset");
  assert.deepEqual(calls, ["refreshSettings", ["unrelayed", [], "gap"]]);
  calls.length = 0;
  state.relay = "off";
  assert.equal(owner.onWatchAllFrame({ event: "reset", data: "" }), "reset");
  assert.deepEqual(calls, ["refreshSettings"], "with relay off nothing is reported");
  assert.equal(owner.onWatchAllFrame({ event: "settings", data: "" }), undefined);
  assert.deepEqual(calls, ["refreshSettings", "settings"]);
});

test("a message frame is relayed with its tail and resume marks; junk is dropped", () => {
  const { owner, calls, state } = makeFrames();
  state.resumed = true;
  assert.equal(owner.onWatchAllFrame(messageFrame({ from_tail: true })), undefined);
  assert.deepEqual(calls, [["relay", "m1", false, false, { fromTail: true, resumed: true }]]);
  calls.length = 0;
  owner.onWatchAllFrame(messageFrame({ self_posted: true, client_key: "k1" }));
  assert.deepEqual(calls, [
    ["selfAuthor", "a1"], ["settle", "k1"],
    ["relay", "m1", true, false, { fromTail: false, resumed: true }],
  ], "the page's own send settles before it is relayed");
  calls.length = 0;
  owner.onWatchAllFrame({ event: "message", data: "not json" });
  owner.onWatchAllFrame({ event: "message", data: "{}" });
  owner.onWatchAllFrame({ event: "other", data: "{}" });
  assert.deepEqual(calls, [], "unparseable and shapeless frames go nowhere");
});

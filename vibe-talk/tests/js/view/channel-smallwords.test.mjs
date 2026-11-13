// Tests for web/src/view/channel/daytap.ts, richheaders.ts and noiseacts.ts on their own
// (`#624 voice-page-modularity`): saying a message's day, the rich-headers switch kept on
// this device, and tallying/rescuing noise. The page over the same factories is in
// ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");
const PRESENT_TIME = await moduleForTests("src/present/time/index.ts");

const message = (extra = {}) => ({
  id: "m1", channel_id: "c1", author: "Ann", author_id: "a1", author_is_bot: false,
  content: "hi", timestamp: "2026-01-01T00:00:00Z", spoken_time: "midnight", reply_to: null, ...extra,
});

test("a tap or Enter on a titled time says its full day; everything else is the row's", () => {
  const said = [];
  const tap = CHANNEL.createDayTap({ setStatus: (text) => said.push(text) });
  const node = {
    tagName: "TIME", className: "msg-time", title: "",
    getAttribute: (name) => (name === "datetime" ? "2026-01-01T00:00:00Z" : null),
    setAttribute: (_name, value) => { node.title = value; },
  };
  tap.onTimeTap({ target: node });
  assert.deepEqual(said, [PRESENT_TIME.fullLocalAt(new Date("2026-01-01T00:00:00Z"))]);
  tap.onTimeTap({ target: { tagName: "DIV", className: "msg-time", getAttribute: () => null } });
  tap.onTimeTap({ target: { ...node, className: "other" } });
  tap.onTimeTap(null);
  assert.equal(said.length, 1, "only a titled time speaks");
  let stopped = 0;
  tap.onTimeKey({
    key: "Enter", target: node,
    stopPropagation: () => { stopped += 1; }, preventDefault: () => { stopped += 1; },
  });
  assert.equal(said.length, 2);
  assert.equal(stopped, 2, "the key that said the day is taken");
  tap.onTimeKey({ key: "a", target: node, stopPropagation: () => { stopped += 1; }, preventDefault: () => {} });
  assert.equal(said.length, 2, "every other key is the row's");
  tap.sayTimeDay({ getAttribute: () => "not a date" });
  assert.equal(said.length, 2, "a time that names no day says nothing");
});

test("rich headers: absent means on, the choice is kept under the same descriptor", () => {
  const kept = [];
  const held = new Map();
  const shown = [];
  const rich = CHANNEL.createRichHeaders({
    renderRichHeaders: (on) => shown.push(on),
    deviceSettings: () => ({
      get: () => held.get("v") !== false,
      keep: (descriptor, value) => { kept.push([descriptor, value]); held.set("v", value); },
    }),
    richHeadersSetting: "the-descriptor",
  });
  assert.equal(rich.storedRichHeaders(), true, "absent means on");
  rich.applyRichHeaders(false);
  assert.deepEqual(kept, [["the-descriptor", false]], "kept whole, under its own descriptor");
  assert.deepEqual(shown, [false]);
  assert.equal(rich.storedRichHeaders(), false);
});

function makeNoise() {
  const calls = [];
  const canon = { channel: "c1", messages: [message({ noise: true }), message({ id: "m2" })] };
  const state = { threading: true, hiding: false, rows: [message({ noise: true }), message({ id: "m2" })] };
  const acts = CHANNEL.createNoiseActs({
    providerFacts: { threading: () => state.threading },
    readModes: () => ({ hiding: () => state.hiding }),
    timelineRows: () => ({ messages: state.rows }),
    el: () => ({ children: state.rows }),
    rowMessages: (row) => [row],
    selectedChannelId: () => "c1",
    api: async (path, options) => { calls.push(["api", path, options.body.messages]); },
    channelStore: { current: () => canon, open: () => canon },
    eachColumn: (act) => act(),
    guardQuietly: (fn) => () => fn(),
    refreshAfterInboxChange: async () => { calls.push("refresh"); },
    setStatus: (text) => calls.push(["status", text]),
  });
  return { acts, calls, canon, state };
}

test("the tally counts the loaded verdicts", () => {
  const { acts, state } = makeNoise();
  assert.deepEqual(acts.noiseTally(), { matched: 1, total: 2 });
  state.threading = false; state.hiding = true;
  assert.deepEqual(acts.noiseTally(), { matched: 1, total: 2 }, "without threads the tally still counts the loaded rows");
});

test("a rescue is not optimistic: the server first, then every copy told", async () => {
  const { acts, calls, canon } = makeNoise();
  await acts.markNotNoise(["m1"]);
  assert.deepEqual(calls[0], ["api", "/api/v1/channels/c1/not-noise", ["m1"]]);
  assert.equal(canon.messages[0].noise, false, "the held copy is ordinary at once");
  assert.equal(canon.messages[1].noise, undefined);
  assert.match(calls[calls.length - 1][1], /^1 message no longer read automatically/);
  calls.length = 0;
  await acts.markNotNoise([]);
  assert.deepEqual(calls, [], "nothing asked, nothing sent");
});

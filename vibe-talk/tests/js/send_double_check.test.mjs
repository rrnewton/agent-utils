// `#621 send-double-check`: tick states and duration readout.
import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";

import { moduleForTests } from "../../web/build.mjs";
import { readWeb } from "../../web/page_sources.mjs";

const js = readWeb("voice.js");
const css = readWeb("voice.css");

/** How a duration is worded, which moved into web/src/present/notices (`#624 voice-page-modularity`). */
const NOTICES = await moduleForTests("src/present/notices/index.ts");

function loadHelpers() {
  // sendTimingText moved to present/notices as sendTimingTextWords (`#624 voice-page-modularity`).
  assert.ok(typeof NOTICES.sendTimingTextWords === "function" && typeof NOTICES.fmtDuration === "function", "duration helpers present");
  return {
    fmtDuration: NOTICES.fmtDuration,
    sendTimingText: (entry) => NOTICES.sendTimingTextWords(entry, "Slack", entry.sendStartedAt),
  };
}

test("duration readout shows deltas from Send, never absolute times", () => {
  const { fmtDuration, sendTimingText } = loadHelpers();
  assert.equal(fmtDuration(120), "120 ms");
  assert.equal(fmtDuration(1900), "1.9 s");
  const text = sendTimingText({ channel: "c1", serverMs: 120, upstreamMs: 1900, sendStartedAt: Date.now() });
  assert.equal(text, "server 120 ms · Slack +1.9 s · total 2.0 s");
  assert.ok(!/\d{1,2}:\d{2}/.test(text), "no clock time");
});

test("two ticks render gray then green, failure is not green", () => {
  // The ticks are drawn by web/src/view/channel/outboxrows.ts now (`#624 voice-page-modularity`).
  const rows = readWeb("src/view/channel/outboxrows.ts");
  assert.match(rows, /data-tick", "server"/);
  assert.match(rows, /data-tick", "upstream"/);
  assert.match(rows, /entry\.serverTick \? " ok"/);
  assert.match(rows, /entry\.tickFailed \? " bad"/);
  assert.match(css, /\.send-tick \{[^}]*font-size: 9px/);
  assert.match(css, /\.send-tick\.ok \{[^}]*color: #1a9e54/);
  assert.match(css, /\.send-tick\.bad \{[^}]*color: #c0392b/);
});

test("page asks for accepted phase then reads outcome", () => {
  // The POST and its outcome ask are the send service's now (`#624 voice-page-modularity`).
  const dispatch = readWeb("src/core/outbox/dispatch.ts");
  assert.match(dispatch, /reply\?ack=accepted/);
  assert.match(dispatch, /payload\.state === "accepted"/);
  assert.match(dispatch, /askOutgoingOutcome\(entry, job, body\)/);
});

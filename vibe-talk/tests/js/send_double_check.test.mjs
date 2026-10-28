// `#621 send-double-check`: tick states and duration readout.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const js = readFileSync(join(here, "../../web/voice.js"), "utf8");
const css = readFileSync(join(here, "../../web/voice.css"), "utf8");

function loadHelpers() {
  const start = js.indexOf("function fmtDuration");
  const end = js.indexOf("/** Whether `error` means the server's answer was lost");
  assert.ok(start > 0 && end > start, "duration helpers present");
  const ctx = { chatServiceOf: () => "Slack" };
  vm.createContext(ctx);
  vm.runInContext(js.slice(start, end), ctx);
  return ctx;
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
  assert.match(js, /data-tick", "server"/);
  assert.match(js, /data-tick", "upstream"/);
  assert.match(js, /entry\.serverTick \? " ok"/);
  assert.match(js, /entry\.tickFailed \? " bad"/);
  assert.match(css, /\.send-tick \{[^}]*font-size: 9px/);
  assert.match(css, /\.send-tick\.ok \{[^}]*color: #1a9e54/);
  assert.match(css, /\.send-tick\.bad \{[^}]*color: #c0392b/);
});

test("page asks for accepted phase then reads outcome", () => {
  assert.match(js, /reply\?ack=accepted/);
  assert.match(js, /payload\.state === "accepted"/);
  assert.match(js, /askOutgoingOutcome\(entry, job, body\)/);
});

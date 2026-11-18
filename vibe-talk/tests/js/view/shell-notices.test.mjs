// Tests for web/src/view/shell/notices.ts on its own (`#624 voice-page-modularity`):
// the error panel and the connection banner never render without text
// (`#1726 vibe-talk-empty-red-banner`). The page over the same factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");

function makeNotices() {
  const els = new Map();
  const el = (id) => {
    if (!els.has(id)) els.set(id, { hidden: true, textContent: "", attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; } });
    return els.get(id);
  };
  el("error-wrap").hidden = true;
  el("connection-banner").hidden = true;
  el("status-line").hidden = true;
  const timers = [];
  const notices = SHELL.createShellNotices({
    clock: {
      setTimeout: (fn, ms) => { timers.push({ fn, ms }); return timers.length; },
      clearTimeout: () => {},
    },
    el,
    redact: (text) => String(text),
    unresponsive: () => null,
    holdingReader: (mutate) => mutate(),
    detailChanged: () => {},
  });
  return { notices, el, timers };
}

test("#1726: an error with no words still says something, never an empty shell", () => {
  const { notices, el } = makeNotices();
  notices.showError("");
  assert.equal(el("error-wrap").hidden, false);
  assert.match(el("error").textContent, /no reason was given/);
  notices.showError("   ");
  assert.match(el("error").textContent, /no reason was given/);
  notices.showError("the read failed");
  assert.equal(el("error").textContent, "the read failed");
});

test("#1726: a connection banner with no detail stays away", () => {
  const { notices, el } = makeNotices();
  notices.showDetail("");
  assert.equal(el("connection-banner").hidden, true);
  notices.showDetail("  ");
  assert.equal(el("connection-banner").hidden, true);
  notices.showDetail("voice provider · vibe-talk-v1");
  assert.equal(el("connection-banner").hidden, false);
  assert.equal(el("connection-detail").textContent, "voice provider · vibe-talk-v1");
});

test("#1726: appending a detail to nothing leaves no leading separator", () => {
  const { notices, el } = makeNotices();
  notices.showDetail("voice provider");
  notices.addDetail("session valid for about 5 minutes");
  assert.equal(el("connection-detail").textContent, "voice provider · session valid for about 5 minutes");
});

test("#1837: an idle confirmation gets the short clock, action words the full one", () => {
  const { notices, el, timers } = makeNotices();
  const fire = (ms) => {
    for (let i = timers.length - 1; i >= 0; i -= 1) {
      if (timers[i].ms === ms) timers.splice(i, 1)[0].fn();
    }
  };
  notices.setStatus("Ready.");
  assert.equal(el("status-line").hidden, false, "Ready. said nothing");
  fire(2500);
  assert.equal(el("status-line").hidden, true, "Ready. outstayed the idle clock");
  notices.setStatus("1 message marked as dealt with here");
  assert.equal(el("status-line").hidden, false, "the action words said nothing");
  fire(2500);
  assert.equal(el("status-line").hidden, false, "action words went on the idle clock");
  fire(6000);
  assert.equal(el("status-line").hidden, true, "action words outstayed the full clock");
});

test("#1859: an idle confirmation is marked to wait, action words are not", () => {
  const { notices, el } = makeNotices();
  notices.setStatus("Ready.");
  assert.equal(el("status-line").attrs["data-idle"], "", "Ready. was not marked to wait");
  assert.equal(el("status").textContent, "Ready.", "the announcement was lost");
  notices.setStatus("1 message marked as dealt with here");
  assert.equal(el("status-line").attrs["data-idle"], undefined, "action words were marked to wait");
  notices.setStatus("ready");
  assert.equal(el("status-line").attrs["data-idle"], "", "the lowercase twin was not marked");
  // `#1862`: the device-voice confirmations at a read's start wait too.
  notices.setStatus("Reading with Device voice.");
  assert.equal(el("status-line").attrs["data-idle"], "", "the reading notice was not marked to wait");
  notices.setStatus("Reading with the device voice.");
  assert.equal(el("status-line").attrs["data-idle"], "", "the device-voice notice was not marked to wait");
  notices.dismissStatus();
  assert.equal(el("status-line").attrs["data-idle"], undefined, "a taken-away line kept the idle mark");
});

test("#1726: a status line with no words takes itself away", () => {
  const { notices, el } = makeNotices();
  notices.setStatus("Reading…");
  assert.equal(el("status-line").hidden, false);
  notices.setStatus("");
  assert.equal(el("status-line").hidden, true);
});

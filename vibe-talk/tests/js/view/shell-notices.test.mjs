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
    if (!els.has(id)) els.set(id, { hidden: true, textContent: "", attrs: {}, setAttribute(k, v) { this.attrs[k] = v; } });
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
  return { notices, el };
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

test("#1726: a status line with no words takes itself away", () => {
  const { notices, el } = makeNotices();
  notices.setStatus("Reading…");
  assert.equal(el("status-line").hidden, false);
  notices.setStatus("");
  assert.equal(el("status-line").hidden, true);
});

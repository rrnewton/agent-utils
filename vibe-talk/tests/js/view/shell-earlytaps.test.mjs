// Tests for web/src/view/shell/earlytaps.ts (`#2216 vibe-talk-pace-availability-flake`):
// a tap on a dock tile before its listener mounts is recorded once and
// replayed by the mount, never lost and never doubled.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");

function fakeDoc() {
  const listeners = new Map();
  return {
    addEventListener(type, fn, capture) { listeners.set(`${type}:${capture}`, fn); },
    removeEventListener(type, fn, capture) { if (listeners.get(`${type}:${capture}`) === fn) listeners.delete(`${type}:${capture}`); },
    fire(target) { listeners.get("click:true")?.({ target }); },
    listening: () => listeners.size,
  };
}

const tile = (id) => ({ closest: (selector) => (selector === `#${id}` ? {} : null) });
const inner = (id) => ({ closest: (selector) => (selector === `#${id}` ? {} : null) });
const elsewhere = () => ({ closest: () => null });

test("`#2216`: the first tap on a watched tile is taken once, and only once", () => {
  const doc = fakeDoc();
  const taps = SHELL.createEarlyTaps(doc, ["read-aloud", "read-speed"]);
  doc.fire(inner("read-speed"));
  doc.fire(tile("read-speed"));
  assert.equal(taps.take("read-speed"), true, "the early tap was not recorded");
  assert.equal(taps.take("read-speed"), false, "the early tap replayed twice");
  assert.equal(taps.take("read-aloud"), false, "an untapped tile claims a tap");
});

test("`#2216`: taps elsewhere, and taps after stop, are not recorded", () => {
  const doc = fakeDoc();
  const taps = SHELL.createEarlyTaps(doc, ["read-aloud", "read-speed"]);
  doc.fire(elsewhere());
  assert.equal(taps.take("read-aloud"), false);
  taps.stop();
  assert.equal(doc.listening(), 0, "stop left the recorder listening");
  doc.fire(tile("read-aloud"));
  assert.equal(taps.take("read-aloud"), false, "a tap after stop was recorded");
});

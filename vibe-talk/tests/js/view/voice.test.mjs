// The call owners can be constructed before the rest of the page is initialized. Their effects
// belong to explicit actions and initialization seams, not construction (#624 voice-page-modularity).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const VIEW_VOICE = await moduleForTests("src/view/voice/index.ts");

for (const name of ["createVoiceCall", "createVoiceTranscript", "createReadAloud"]) {
  test(`\`#624 voice-page-modularity\`: ${name} waits for explicit actions before reading dependencies or starting effects`, () => {
    const attempts = [];
    const refuse = (what) => () => {
      attempts.push(what);
      throw new Error(`construction reached ${what}`);
    };
    const unreadable = (what) => new Proxy({}, {
      get(_target, key) { return refuse(`${what}.${String(key)}`)(); },
    });
    const replacements = [
      [globalThis, "document", unreadable("document")],
      [globalThis, "window", unreadable("window")],
      [globalThis, "setTimeout", refuse("setTimeout")],
      [globalThis, "setInterval", refuse("setInterval")],
      [globalThis, "requestAnimationFrame", refuse("requestAnimationFrame")],
      [globalThis, "fetch", refuse("fetch")],
      [Date, "now", refuse("Date.now")],
      [Math, "random", refuse("Math.random")],
    ];
    const saved = replacements.map(([object, key]) => Object.getOwnPropertyDescriptor(object, key));
    try {
      for (const [object, key, value] of replacements) {
        Object.defineProperty(object, key, { configurable: true, writable: true, value });
      }
      const owner = VIEW_VOICE[name](unreadable("dependencies"));
      for (const key of Object.keys(owner)) {
        const held = owner[key];
        if (typeof held === "function") {
          assert.equal(owner[key], held, `${name}.${key} changed callback identity`);
        }
      }
      // An owner catching and hiding an eager failure is still an owner that ran too early.
      assert.deepEqual(attempts, [], "construction attempted an effect before its explicit action");
    } finally {
      replacements.forEach(([object, key], index) => {
        if (saved[index]) Object.defineProperty(object, key, saved[index]);
        else delete object[key];
      });
    }
  });
}

test("`#886 voice-page-autoplay`: off by default and kept; on, a read heard to its end goes on to the next unread row below it in its view, and stops at the end or when halted", async () => {
  const { createAutoplay, AUTOPLAY_WORDS } = VIEW_VOICE;
  const toggle = { attributes: {}, getAttribute(name) { return this.attributes[name] ?? null; }, setAttribute(name, value) { this.attributes[name] = value; } };
  const label = { textContent: "" };
  const row = (id, { read = false, card = false, threadRead = false, ids = card ? [] : [id], root = null } = {}) => ({
    id, read, ids, isConnected: true, ...(root ? { cardRoot: { id: root } } : {}),
    getAttribute: (name) => (name === "data-id" ? id : null),
    hasAttribute: (name) => (name === "data-card" ? card : name === "data-thread-read" ? threadRead : false),
  });
  const main = [row("1"), row("2", { read: true }), row("3"), row("4", { ids: ["4", "5"] }), row("6", { read: true })];
  // A card's own id is its thread's; it reads its thread's first message, whose id a chat service may write otherwise.
  const cards = [row("t1", { card: true, root: "r1" }), row("t2", { card: true, threadRead: true, root: "r2" }), row("t3", { card: true, root: "r3" })];
  const panes = { main, cards };
  let order = ["main", "cards"];
  let stored = false;
  const kept = [];
  const read = [];
  const said = [];
  const archived = Promise.resolve();
  const autoplay = createAutoplay({
    byId: (id) => ({ "read-autoplay": toggle, "autoplay-toggle-label": label })[id],
    stored: () => stored,
    keep: (on) => kept.push(on),
    panes: () => order,
    paneRows: (pane) => panes[pane],
    idsOf: (one) => one.ids,
    rowIsRead: (one) => one.read,
    paneRowFor: (pane, id, card) => panes[pane].find((one) => one.id === id && one.hasAttribute("data-card") === card) || null,
    read: (one) => read.push(one.redrawn ? `${one.id} redrawn` : one.id),
    setStatus: (text) => said.push(text),
  });
  autoplay.render();
  assert.deepStrictEqual([autoplay.on(), toggle.attributes["aria-pressed"], label.textContent], [false, "false", AUTOPLAY_WORDS.off]);
  assert.equal(autoplay.nextAfter(["1"]), null, "off, a read went on");
  autoplay.setOn(true);
  assert.deepStrictEqual([kept, toggle.attributes["aria-pressed"], label.textContent], [[true], "true", "Autoplay: on"]);
  stored = false;
  assert.equal(autoplay.on(), true, "the choice made here was lost to the storage's answer");
  // Below the read, skipping what is read: 1 goes on to 3, past the read 2.
  assert.equal(autoplay.nextAfter(["1"])(archived), true);
  assert.deepStrictEqual(read, ["3"]);
  // A combined row read as its messages: 4 and 5 go on past the read 6 to nothing, said once the archive is said.
  const end = autoplay.nextAfter(["4", "5"]);
  assert.equal(end(archived), false, "past the last unread a read went on");
  assert.deepStrictEqual(said, [], "the end was said before the archive's own lines");
  await archived;
  assert.deepStrictEqual(said, ["Autoplay: nothing unread below."]);
  // The message read is found as one of a combined row's messages too: 5 is in 4's row, and goes on to nothing.
  assert.equal(autoplay.nextAfter(["5"])(archived), false);
  // In the thread list, a card found by its thread's first message, on to the next card with unread replies.
  assert.equal(autoplay.nextAfter(["r1"])(archived), true);
  assert.deepStrictEqual(read, ["3", "t3"], "a card's read did not go on past the card whose thread is read");
  // The pane the reader works in first: the same id in two panes goes on in the first.
  panes.cards.unshift(row("3"));
  order = ["cards", "main"];
  autoplay.nextAfter(["3"])(archived);
  order = ["main", "cards"];
  // Halted (Hang up): the read in progress goes on to nothing, once; the next read goes on again.
  autoplay.halt("1");
  assert.equal(autoplay.nextAfter(["1"]), null, "a halted read went on");
  autoplay.nextAfter(["1"])(archived);
  // A halt for another read leaves this one going on; Stop's forgets one.
  autoplay.halt("9");
  autoplay.nextAfter(["1"])(archived);
  autoplay.halt("1");
  autoplay.halt(null);
  autoplay.nextAfter(["1"])(archived);
  // Found again after the archive redrew the list: the new row for the same id is the one read.
  const going = autoplay.nextAfter(["1"]);
  panes.main[2] = { ...row("3"), redrawn: true };
  going(archived);
  assert.deepStrictEqual(read, ["3", "t3", "t1", "3", "3", "3", "3 redrawn"], "a read went on to the wrong row");
  // A row gone from the page by the time it is read: nothing read, and the caller told so.
  const gone = autoplay.nextAfter(["1"]);
  const [left] = panes.main.splice(2, 1);
  left.isConnected = false;
  assert.equal(gone(archived), false, "a row gone from the page was read, or the caller was told a read started");
  assert.equal(read.length, 7, "a row gone from the page was read");
  autoplay.setOn(false);
  assert.deepStrictEqual([kept, toggle.attributes["aria-pressed"], label.textContent], [[true, false], "false", "Autoplay: off"]);
});

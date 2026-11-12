// Tests for web/src/view/shell/promotion.ts (`#1470 toolbar-dynamic-promotion`): the pure
// fit decision fed measured widths, and the controller over a fake bar — no action listed
// twice in any state, and a promoted item is the menu's own element, state included.
// Run with the other module suites: node --test tests/js/view/shell-promotion.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const PROMOTION = await moduleForTests("src/view/shell/promotion.ts");
const { decidePromotion, createPromotion, PROMOTION_ORDER } = PROMOTION;

/** Candidates at fixed measured widths, in the owner's priority order. */
function candidates(overrides = {}) {
  const widths = { discuss: 90, "fold-read": 80, "expand-collapse": 180, summaries: 95 };
  return PROMOTION_ORDER.map((key) => ({
    key,
    width: overrides[key]?.width ?? widths[key],
    available: overrides[key]?.available ?? true,
  }));
}

test("#1470: with room for everything, every candidate is promoted in priority order", () => {
  const promoted = decidePromotion(600, candidates(), new Set());
  assert.deepStrictEqual([...promoted], [...PROMOTION_ORDER]);
});

test("#1470: with room for some, a prefix is promoted and the tail stays in the menu", () => {
  // 90 + 80 fits 200; adding the 180-wide expand/collapse slot does not.
  const promoted = decidePromotion(200, candidates(), new Set());
  assert.deepStrictEqual([...promoted], ["discuss", "fold-read"]);
});

test("#1470: with room for none, nothing is promoted — the phone's ordinary state", () => {
  const promoted = decidePromotion(50, candidates(), new Set());
  assert.deepStrictEqual([...promoted], []);
});

test("#1470: an unavailable candidate is never promoted, however wide the bar", () => {
  const promoted = decidePromotion(900, candidates({ "fold-read": { available: false } }), new Set());
  assert.deepStrictEqual([...promoted], ["discuss", "expand-collapse", "summaries"]);
});

test("#1470: hysteresis holds a promoted set inside the slack instead of flickering", () => {
  const all = new Set(PROMOTION_ORDER);
  const total = 90 + 80 + 180 + 95;
  // Ten pixels short: inside the slack, nothing moves.
  assert.deepStrictEqual([...decidePromotion(total - 10, candidates(), all)], [...PROMOTION_ORDER]);
  // Forty short: past the slack, the lowest priority goes back to the menu.
  assert.deepStrictEqual(
    [...decidePromotion(total - 40, candidates(), all)],
    ["discuss", "fold-read", "expand-collapse"],
  );
  // And from the menu side, that same width does not promote it straight back.
  const demoted = new Set(["discuss", "fold-read", "expand-collapse"]);
  assert.deepStrictEqual([...decidePromotion(total - 40, candidates(), demoted)], [...demoted]);
});

/** A minimal element: children, parentage, classes and a fixed layout width. */
class FakeEl {
  constructor(id, width = 0) {
    this.id = id;
    this.width = width;
    this.hidden = false;
    this.children = [];
    this.parentNode = null;
    this.style = {};
    const classes = new Set();
    this.classList = {
      add: (name) => classes.add(name),
      remove: (name) => classes.delete(name),
      contains: (name) => classes.has(name),
    };
  }
  get offsetWidth() { return this.hidden ? 0 : this.width; }
  get clientWidth() { return this.width; }
  get nextElementSibling() {
    if (!this.parentNode) return null;
    const siblings = this.parentNode.children;
    const index = siblings.indexOf(this);
    return index >= 0 && index + 1 < siblings.length ? siblings[index + 1] : null;
  }
  appendChild(child) {
    child.remove();
    child.parentNode = this;
    this.children.push(child);
    return child;
  }
  insertBefore(child, anchor) {
    child.remove();
    child.parentNode = this;
    const index = anchor ? this.children.indexOf(anchor) : -1;
    if (index >= 0) this.children.splice(index, 0, child);
    else this.children.push(child);
    return child;
  }
  remove() {
    if (!this.parentNode) return;
    const siblings = this.parentNode.children;
    const index = siblings.indexOf(this);
    if (index >= 0) siblings.splice(index, 1);
    this.parentNode = null;
  }
}

/** A fake bar: a pack with two standing members, the menu with its items, Discuss in both. */
function fakeBar(packWidth) {
  const els = new Map();
  const make = (id, width = 0) => {
    const element = new FakeEl(id, width);
    els.set(id, element);
    return element;
  };
  const pack = make("bar-pack", packWidth);
  const menu = make("action-menu", 0);
  make("control-bar", packWidth);
  pack.appendChild(make("discord-channel", 120));
  pack.appendChild(make("thread-select", 100));
  pack.appendChild(make("discuss-open", 90));
  for (const [id, width] of [["expand-all", 85], ["collapse-all", 95], ["summarise", 95], ["discuss-menu-open", 90], ["todo-filter", 80], ["reader-toggle", 90]]) {
    menu.appendChild(make(id, width));
  }
  let view = "discord";
  const promotion = createPromotion({
    el: (id) => els.get(id),
    screen: () => "main",
    view: () => view,
    renderActionMenu: () => {},
  });
  return { els, pack, menu, promotion, setView: (next) => { view = next; } };
}

/** The no-duplicates invariant: every movable stands in exactly one home; Discuss shows exactly one face. */
function assertNeverTwice({ els, pack, menu }) {
  for (const id of ["todo-filter", "expand-all", "collapse-all", "summarise"]) {
    const homes = [pack, menu].filter((home) => home.children.includes(els.get(id)));
    assert.equal(homes.length, 1, `${id} stood in ${homes.length} places`);
  }
  const shown = ["discuss-open", "discuss-menu-open"].filter((id) => !els.get(id).hidden);
  assert.equal(shown.length, 1, `Discuss showed as ${shown.join(", ") || "nothing"}`);
}

test("#1470: a wide bar promotes every action onto the toolbar, none left listed in the menu", () => {
  const bar = fakeBar(900);
  bar.promotion.refresh();
  assertNeverTwice(bar);
  for (const id of ["todo-filter", "expand-all", "collapse-all", "summarise"]) {
    assert.equal(bar.els.get(id).parentNode, bar.pack, `${id} was not promoted`);
  }
  assert.equal(bar.els.get("discuss-open").hidden, false);
  assert.equal(bar.els.get("discuss-menu-open").hidden, true);
});

test("#1470: a narrow bar keeps the actions in the menu and Discuss shows its menu face", () => {
  const bar = fakeBar(260);
  bar.promotion.refresh();
  assertNeverTwice(bar);
  for (const id of ["todo-filter", "expand-all", "collapse-all", "summarise"]) {
    assert.equal(bar.els.get(id).parentNode, bar.menu, `${id} left the menu`);
  }
  assert.equal(bar.els.get("discuss-open").hidden, true);
  assert.equal(bar.els.get("discuss-menu-open").hidden, false);
});

test("#1470: promotion moves the menu's own element — its state is the same element's state", () => {
  const bar = fakeBar(900);
  const fold = bar.els.get("todo-filter");
  fold.pressed = "true";
  bar.promotion.refresh();
  assert.equal(bar.pack.children.at(-1) === fold || bar.pack.children.includes(fold), true);
  assert.equal(fold.pressed, "true", "the promoted toggle lost its state");
  bar.pack.width = 260;
  bar.promotion.refresh();
  assert.equal(fold.parentNode, bar.menu, "demotion did not return the element to the menu");
  assert.equal(fold.pressed, "true", "the demoted toggle lost its state");
  assertNeverTwice(bar);
});

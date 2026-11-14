// Tests for the popup placement arithmetic (`#1544 documents-desktop-control-bar`):
// a popup centres over its button and is clamped inside the viewport.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const { centredPopupLeft } = await moduleForTests("src/view/dom/popup-position.ts");

test("centres the popup over the button", () => {
  assert.equal(centredPopupLeft(500, 200, 1000), 400);
});

test("clamps at the left edge", () => {
  assert.equal(centredPopupLeft(30, 200, 1000), 8);
});

test("clamps at the right edge", () => {
  assert.equal(centredPopupLeft(990, 200, 1000), 792);
});

test("a popup wider than the viewport keeps the margin", () => {
  assert.equal(centredPopupLeft(200, 500, 390), 8);
});

// Tests for the Bookmark chip popover placement (`#2229 bookmark-chip-clear-popover`): above the
// chip, clear of a standing status line, with the measured phone geometries as the cases.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const { bookmarkPopoverTop } = await moduleForTests("src/view/channel/readingjumps.ts");

const HEIGHT = 59; // the popover as measured on the phone (probe, 412x915)

test("above the chip when no status stands", () => {
  // Chip top 726 in a 915 viewport: popover bottom (top + height) stays above the chip.
  const top = bookmarkPopoverTop(726, 754, HEIGHT, 915, null);
  assert.equal(top + HEIGHT, 726 - 8);
});

test("above the status line when it stands, keyboard closed", () => {
  // Probe rects, keyboard closed: toast 649.5-679.9, chip top 726.3.
  const toast = { top: 649.5, bottom: 679.9 };
  const top = bookmarkPopoverTop(726.3, 754.3, HEIGHT, 915, toast);
  assert.ok(top + HEIGHT <= toast.top - 8 + 1, `popover bottom ${top + HEIGHT} covers the toast`);
});

test("above the status line when it stands, keyboard open", () => {
  // Probe rects, keyboard open (visualViewport 579): toast 313.5-343.9, chip top 390.3.
  const toast = { top: 313.5, bottom: 343.9 };
  const top = bookmarkPopoverTop(390.3, 418.3, HEIGHT, 579, toast);
  assert.ok(top + HEIGHT <= toast.top - 8 + 1, `popover bottom ${top + HEIGHT} covers the toast`);
  assert.ok(top >= 8 && top + HEIGHT <= 579, "the popover left the shrunken viewport");
});

test("below the chip where above the toast cannot fit, chip and popover visible", () => {
  // Toast overlapping the above-chip place with no room above it; room below the chip.
  const toast = { top: 40, bottom: 80 };
  const top = bookmarkPopoverTop(120, 148, HEIGHT, 915, toast);
  assert.equal(top, 148 + 8);
});

test("over the status only where no clear place fits", () => {
  // Short viewport, toast above the chip, no room below either: above the chip wins.
  const toast = { top: 8, bottom: 38 };
  const top = bookmarkPopoverTop(120, 148, HEIGHT, 200, toast);
  assert.equal(top, 120 - HEIGHT - 8);
});

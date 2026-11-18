// Tests for web/src/view/channel/windowing.ts on its own (`#1846` step 3): the slice math
// that keeps a 5000-row list to a mounted window.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const WINDOWING = await moduleForTests("src/view/channel/windowing.ts");

test("`#1846` step 3: the slice covers the viewport plus prefetch, capped, at every scroll place", () => {
  const { offsetsFor, sliceFor } = WINDOWING;
  const heights = Array.from({ length: 5000 }, () => 84);
  const offsets = offsetsFor(heights);
  assert.equal(offsets.length, 5001);
  assert.equal(offsets[5000], 5000 * 84);
  // At the foot (newest): the window ends at the last group and holds at most the cap.
  const foot = sliceFor(offsets, 5000 * 84, 800, 1200, 60);
  assert.equal(foot.to, 5000);
  assert.ok(foot.to - foot.from <= 60, `foot window mounts ${foot.to - foot.from}`);
  // At the top and in the middle the same bound holds.
  for (const top of [0, 100000, 210000]) {
    const slice = sliceFor(offsets, top, 800, 1200, 60);
    assert.ok(slice.to - slice.from <= 60, `window at ${top} mounts ${slice.to - slice.from}`);
    assert.ok(slice.from <= slice.to && slice.from >= 0 && slice.to <= 5000);
  }
  // One screenful of scrolling moves the window by about a screenful, not a rebuild.
  const before = sliceFor(offsets, 100000, 800, 1200, 60);
  const after = sliceFor(offsets, 100800, 800, 1200, 60);
  assert.ok(after.from - before.from <= 20, `one screenful moved the window ${after.from - before.from} groups`);
  // No rows, no window.
  assert.deepEqual(sliceFor([0], 0, 800, 1200, 60), { from: 0, to: 0 });
});

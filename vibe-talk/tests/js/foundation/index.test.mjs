// Tests for web/src/foundation/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The glyph widths the fold model counts lines by, and the other foundations (web/src/foundation/). */
const GLYPHS = await moduleForTests("src/foundation/index.ts");

test("`#624 voice-page-modularity`: the reader's writes run one after another, a failure stopping none after it, from the module alone", async () => {
  const { createSerialQueue } = GLYPHS;
  const queue = createSerialQueue();
  const order = [];
  let release;
  const first = queue.run(() => new Promise((resolve) => { release = resolve; }).then(() => { order.push("first"); return 1; }));
  const second = queue.run(async () => { order.push("second"); throw new Error("refused"); });
  const third = queue.run(async () => { order.push("third"); return 3; });
  await Promise.resolve();
  assert.deepStrictEqual(order, [], "a task started before the one ahead of it settled");
  release();
  assert.equal(await first, 1);
  await assert.rejects(second, /refused/);
  assert.equal(await third, 3, "a failure stopped the task after it");
  assert.deepStrictEqual(order, ["first", "second", "third"]);
});

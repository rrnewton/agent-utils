// Tests for web/src/core/channels/hydration.ts (`#1846` step 5): paced, bounded, cancellable.
import { test } from "node:test";
import assert from "node:assert/strict";
import { moduleForTests } from "../../../web/build.mjs";
const H = await moduleForTests("src/core/channels/hydration.ts");
test("#1846 step 5: pacer drains, retain drops out-of-window work", async () => {
  const done = [];
  const pacer = H.createHydrationPacer((id) => done.push(id), { sliceMs: 1000 });
  pacer.offer(["a", "b", "c", "d"]);
  pacer.retain(["a", "b"]);
  assert.equal(pacer.pending(), 2);
  await new Promise((r) => setTimeout(r, 80));
  assert.deepEqual([...done].sort(), ["a", "b"]);
  assert.equal(pacer.pending(), 0);
  pacer.dispose();
});

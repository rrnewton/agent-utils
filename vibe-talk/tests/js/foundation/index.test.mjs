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

test("`#624 voice-page-modularity`: the links a message's text holds as written, from the module alone", () => {
  const { linksIn, trimAddress } = GLYPHS;
  assert.deepStrictEqual(linksIn("see [the log](https://ci.example/run/1) and <https://a.example|A> or https://b.example/x."),
    [{ href: "https://ci.example/run/1", name: "the log" }, { href: "https://a.example", name: "A" }, { href: "https://b.example/x", name: "https://b.example/x" }]);
  assert.deepStrictEqual(linksIn("xhttps://not.a.link [no](ftp://x) [](https://e.example)"),
    [{ href: "https://e.example", name: "https://e.example" }]);
  assert.equal(trimAddress("https://en.wikipedia.org/wiki/Mercury_(planet))."), "https://en.wikipedia.org/wiki/Mercury_(planet)");
  assert.deepStrictEqual(linksIn(null), []);
});

test("`#1898 vibe-talk-nested-slack-link-in-markdown-label`: a link nested in a Markdown label names the outer link by its words", () => {
  const { linksIn, unwrappedLabel } = GLYPHS;
  // The owner's shape: a Slack `<url|label>` inside a Markdown `[label](url)`; the outer
  // address is the link's, and its name is the nested label's words, never the raw markup.
  assert.deepStrictEqual(
    linksIn("[<http://interncache-all.example.test/manifold/|interncache-all.example.test/manifold/>…/ipfsmf-test-site-loader.html#…](<https://interncache-all.example.test/manifold/flat/ipfsmf-test-site-loader.html#bafy>)"),
    [{ href: "https://interncache-all.example.test/manifold/flat/ipfsmf-test-site-loader.html#bafy", name: "interncache-all.example.test/manifold/…/ipfsmf-test-site-loader.html#…" }]);
  // A nested `<url>` names the outer link by the address it carries.
  assert.deepStrictEqual(linksIn("look [<https://a.example/x>](https://b.example/y) end"),
    [{ href: "https://b.example/y", name: "https://a.example/x" }]);
  assert.deepStrictEqual(linksIn("[see <https://c.example|the doc>](https://d.example)"),
    [{ href: "https://d.example", name: "see the doc" }]);
  // Plain Markdown links and plain Slack links keep the names they always had.
  assert.deepStrictEqual(linksIn("Read [the quarterly plan](https://example.test/x) and <https://a.example|A>"),
    [{ href: "https://example.test/x", name: "the quarterly plan" }, { href: "https://a.example", name: "A" }]);
  assert.equal(unwrappedLabel("see <https://c.example|the doc> and <https://a.example/x>"), "see the doc and https://a.example/x");
  assert.equal(unwrappedLabel("no links <@U1> [brackets]"), "no links <@U1> [brackets]");
});

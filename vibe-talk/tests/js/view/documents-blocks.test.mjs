// Tests for the Documents reader's ONE read block model (`#1505
// documents-read-controls-highlight-tap`): `documentReadBlocks(markdown)`
// cuts a document into the blocks it is SPOKEN in, and the reader draws
// and lights the same blocks, so speak index N, highlight index N and
// tap index N are the same block. Held against the real renderer: the
// model's count and words must be the drawn body's, block for block.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const BLOCKS = await moduleForTests("src/view/documents/blocks.ts");
const RENDER = await moduleForTests("src/view/documents/render.ts");

function blocksOf(markdown) {
  assert.equal(typeof BLOCKS.documentReadBlocks, "function", "blocks does not export documentReadBlocks");
  return BLOCKS.documentReadBlocks(markdown);
}

/** The drawn blocks of the rendered HTML, as words: one entry per drawn block. */
function drawnWords(markdown) {
  const html = RENDER.renderDocumentMarkdown(markdown);
  const words = [];
  const token = /<h[1-6][^>]*>(.*?)<\/h[1-6]>|<blockquote><p>(.*?)<\/p><\/blockquote>|<pre><code>(.*?)<\/code><\/pre>|<table>.*?<\/table>|<li>(.*?)<\/li>|<p>(.*?)<\/p>/gs;
  const plain = (raw) => raw.replace(/<[^>]+>/g, " ")
    .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"")
    .replace(/&#39;/g, "'").replace(/&amp;/g, "&").replace(/\s+/g, " ").trim();
  for (const match of html.matchAll(token)) {
    const raw = match.slice(1).find((part) => part !== undefined) ?? "";
    // A table drawn as one block is spoken as its cells; count it once here.
    words.push(match[0].startsWith("<table") ? "table" : plain(raw));
  }
  return words;
}

test("documents blocks: a bulleted list is one block per item, not one chunk", () => {
  const blocks = blocksOf("- first item\n- second item\n- third item\n");
  assert.deepEqual(blocks.map((b) => b.kind), ["item", "item", "item"]);
  assert.deepEqual(blocks.map((b) => b.text), ["first item", "second item", "third item"]);
});

test("documents blocks: nested list items are blocks of their own, in order", () => {
  const markdown = "- outer one\n  - inner one\n  - inner two\n- outer two\n";
  const blocks = blocksOf(markdown);
  assert.deepEqual(blocks.map((b) => b.text), ["outer one", "inner one", "inner two", "outer two"]);
  assert.ok(blocks.every((b) => b.kind === "item"));
  assert.deepEqual(drawnWords(markdown), blocks.map((b) => b.text), "the drawn items differ from the spoken ones");
});

test("documents blocks: a long list keeps speak order == drawn order", () => {
  const markdown = Array.from({ length: 12 }, (_, i) => `- item number ${i + 1} says its piece`).join("\n") + "\n";
  const blocks = blocksOf(markdown);
  assert.equal(blocks.length, 12);
  assert.deepEqual(drawnWords(markdown), blocks.map((b) => b.text));
});

test("documents blocks: headings, code, quote, table and paragraphs", () => {
  const markdown = [
    "# Field notes",
    "",
    "Intro paragraph here.",
    "",
    "## The list section",
    "- alpha",
    "- beta wrapping over",
    "  two source lines",
    "",
    "```",
    "const answer = 42;",
    "```",
    "",
    "> quoted words",
    "> on two lines",
    "",
    "| Name | Count |",
    "| --- | --- |",
    "| alpha | 2 |",
    "",
    "Closing paragraph.",
    "",
  ].join("\n");
  const blocks = blocksOf(markdown);
  assert.deepEqual(blocks.map((b) => b.kind),
    ["heading", "paragraph", "heading", "item", "item", "code", "quote", "table", "paragraph"]);
  assert.equal(blocks[1].text, "Intro paragraph here.");
  assert.equal(blocks[4].text, "beta wrapping over two source lines", "an item continuation did not join its item");
  assert.equal(blocks[5].text, "const answer = 42;", "the code block speaks its fences or nothing");
  assert.equal(blocks[6].text, "quoted words on two lines");
  assert.equal(blocks[8].text, "Closing paragraph.");
  const drawn = drawnWords(markdown);
  assert.equal(drawn.length, blocks.length, `drew ${drawn.length} blocks, the model has ${blocks.length}`);
});

test("documents blocks: heading and prose with no blank line stay two blocks", () => {
  const blocks = blocksOf("# Title\nProse directly under it.\n");
  assert.deepEqual(blocks.map((b) => [b.kind, b.text]),
    [["heading", "Title"], ["paragraph", "Prose directly under it."]]);
});

test("documents blocks: markdown dressing is not spoken", () => {
  const blocks = blocksOf("Read the [guide](https://example.com) with **care** and `code` now.\n");
  assert.deepEqual(blocks.map((b) => b.text), ["Read the guide with care and code now."]);
});

test("documents blocks: a rule draws nothing spoken", () => {
  const blocks = blocksOf("Above.\n\n---\n\nBelow.\n");
  assert.deepEqual(blocks.map((b) => b.text), ["Above.", "Below."]);
});

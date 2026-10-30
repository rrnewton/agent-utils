// Tests for web/src/view/dom/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The message boxes' keys and growth (web/src/view/composer). */
const VIEW_COMPOSER = await moduleForTests("src/view/composer/index.ts");
/** The DOM primitives: walks, the shell's elements, focus (web/src/view/dom). */
const VIEW_DOM = await moduleForTests("src/view/dom/index.ts");

test("`#624 voice-page-modularity`: the DOM walks and the box helpers, on plain nodes, from the module alone", () => {
  const { nodeWithin, shownNode, childByClass } = VIEW_DOM;
  const node = (className, children = [], extra = {}) => {
    const made = { className, children, nodeType: 1, hidden: false, parentNode: null, ...extra };
    for (const child of children) child.parentNode = made;
    return made;
  };
  const author = node("msg-author reading-mark");
  const inText = node("archive");
  const body = node("md", [inText]);
  const meta = node("meta", [author]);
  const row = node("row", [meta, body]);
  assert.deepStrictEqual([nodeWithin(row, author), nodeWithin(meta, inText), nodeWithin(row, row), nodeWithin(row, null)], [true, false, true, false]);
  assert.equal(childByClass(row, "reading-mark"), author, "a class among others was not found");
  assert.equal(childByClass(row, "archive"), null, "a part was looked for inside the message's own text");
  assert.equal(row.partsFound.get("reading-mark"), author, "what was found was not remembered");
  // Remembered, but no longer carrying the class: walked for again.
  author.className = "msg-author";
  assert.equal(childByClass(row, "reading-mark"), null);
  assert.equal(shownNode(author), true);
  meta.hidden = true;
  assert.equal(shownNode(author), false, "a node inside a hidden one counted as shown");
  // The composer's helpers built on them.
  const { isComposer, focusAtEnd } = VIEW_COMPOSER;
  assert.deepStrictEqual([isComposer({ id: "reply-text" }), isComposer({ id: "search-input" }), isComposer(null)], [true, false, false]);
  const calls = [];
  const box = { value: "draft", focus: (options) => calls.push(["focus", options]), scrollIntoView: (options) => calls.push(["scroll", options]),
    setSelectionRange: (start, end) => calls.push(["caret", start, end]) };
  focusAtEnd(box);
  assert.deepStrictEqual(calls, [["focus", { preventScroll: true }], ["scroll", { block: "nearest", inline: "nearest" }], ["caret", 5, 5]]);
});

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

test("`#624 voice-page-modularity`: where a key landed — a field typed in, a control keeping its own keys, a link — read from the target alone", () => {
  const { isTextField, keepsOwnKeys, pressedBySpace, isLink, answersEnter, scrollsItself } = VIEW_DOM;
  const node = (tagName, fields = {}) => ({ tagName, ...fields });
  for (const missing of [null, undefined, "", 0]) {
    assert.equal(isTextField(missing), false);
    assert.equal(keepsOwnKeys(missing), false);
    assert.equal(pressedBySpace(missing), false);
    assert.equal(isLink(missing), false);
    assert.equal(answersEnter(missing), false);
  }
  assert.equal(isTextField(node("input")), true, "an input of no type is text");
  assert.equal(isTextField(node("INPUT", { type: "Search" })), true);
  assert.equal(isTextField(node("INPUT", { type: "checkbox" })), false);
  assert.equal(isTextField(node("textarea")), true);
  assert.equal(isTextField(node("DIV", { isContentEditable: true })), true);
  assert.equal(isTextField(node("DIV", { isContentEditable: "true" })), false);
  assert.equal(keepsOwnKeys(node("select")), true);
  assert.equal(keepsOwnKeys(node("INPUT", { type: "range" })), true);
  assert.equal(keepsOwnKeys(node("DIV", { getAttribute: (name) => (name === "role" ? "slider" : null) })), true);
  assert.equal(keepsOwnKeys(node("DIV", { getAttribute: () => "button" })), false);
  assert.equal(keepsOwnKeys(node("DIV", { getAttribute: "separator" })), false, "a getAttribute that is no function is not called");
  assert.equal(pressedBySpace(node("summary")), true);
  assert.equal(pressedBySpace(node("A")), false);
  assert.equal(isLink(node("a")), true);
  assert.equal(answersEnter(node("A")), true);
  assert.equal(answersEnter(node("DIV", { isContentEditable: true })), true);
  assert.equal(answersEnter(node("LI")), false);
  assert.equal(scrollsItself(node("TEXTAREA", { scrollHeight: 120, clientHeight: 100 })), true);
  assert.equal(scrollsItself(node("TEXTAREA", { scrollHeight: 101, clientHeight: 100 })), false, "a pixel's rounding is not a page");
  assert.equal(scrollsItself(node("DIV", { scrollHeight: 500, clientHeight: 100 })), false);
});

test("`#624 voice-page-modularity`: plain text goes on the clipboard by its API, else through a hidden textarea, and says whether it went", async () => {
  const { copyText } = VIEW_DOM;
  const written = [];
  assert.equal(await copyText("hello", () => ({ writeText: async (text) => { written.push(text); } })), true);
  assert.deepStrictEqual(written, ["hello"]);
  const was = globalThis.document;
  const events = [];
  const focused = { focus: () => events.push("focus back") };
  globalThis.document = {
    activeElement: focused,
    body: { append: (node) => { events.push("append"); node.parentNode = { removeChild: () => events.push("remove") }; } },
    createElement: () => ({ style: {}, setAttribute: () => {}, focus: () => events.push("focus scratch"), select: () => events.push("select") }),
    execCommand: (command) => { events.push(command); return true; },
  };
  try {
    assert.equal(await copyText("x", () => ({ writeText: async () => { throw new Error("NotAllowedError"); } })), true,
      "a refused API falls back to the textarea");
    assert.deepStrictEqual(events, ["append", "focus scratch", "select", "copy", "remove", "focus back"]);
    events.length = 0;
    assert.equal(await copyText("x", () => { throw new ReferenceError("navigator is not defined"); }), true,
      "a host with no clipboard falls back too");
    globalThis.document.execCommand = undefined;
    assert.equal(await copyText("x", () => null), false, "neither works: says so");
  } finally {
    globalThis.document = was;
  }
});

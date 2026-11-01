// Tests for web/src/view/channel/ on its own: the module built as the page build builds it and held
// directly, on plain stand-in nodes (`#624 voice-page-modularity`). The page's behaviour over the same
// module, on the fixture's DOM, is tested in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The channel's rows as the DOM draws them (web/src/view/channel). */
const CHANNEL_VIEW = await moduleForTests("src/view/channel/index.ts");

/** A stand-in element: a tag, a class and children; a string child is a text node. */
const node = (tag, kids = [], className = "") => ({
  nodeType: 1, tagName: tag.toUpperCase(), className,
  childNodes: kids.map((kid) => (typeof kid === "string" ? { nodeType: 3, textContent: kid } : kid)),
});

test("`#624 voice-page-modularity`: a drawn body read into the fold model's runs, from the module alone", () => {
  const { foldRuns, textNodesOf, RESTATED_CLASS } = CHANNEL_VIEW;
  const body = node("div", [node("p", ["Here ", node("code", ["x"]), " is"]), node("pre", ["a\nb"]), node("table", [node("tr", [node("td", ["c"]), node("td", ["d"])])])]);
  assert.deepStrictEqual(foldRuns(body), [["t", "\nHere "], ["m", "x"], ["t", " is\n"], ["p", "a\nb"], ["w", "c d"]]);
  assert.equal(foldRuns(node("div", [node("details", ["?"])])), null, "an element with no rule was read");
  assert.deepStrictEqual(foldRuns(node("div", [node("p", [node("span", ["said"], RESTATED_CLASS), "rest"])]), RESTATED_CLASS),
    [["t", "\nrest\n"]]);
  assert.equal(textNodesOf(node("pre", ["a", node("b", ["c"])]), "|"), "a|c");
});

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

test("`#624 voice-page-modularity`: a time's title is worded again as the pointer reaches it, from the module alone", () => {
  const { freshenDayTitle, rowTime, EDITED_MARK } = CHANNEL_VIEW;
  const timeEl = (className, at, text) => {
    const attrs = new Map([["datetime", at.toISOString()], ["title", "stale"], ["aria-label", "stale"]]);
    return { tagName: "TIME", className, textContent: text, getAttribute: (name) => attrs.get(name) ?? null,
      setAttribute: (name, value) => attrs.set(name, String(value)), attrs };
  };
  const now = new Date();
  const time = timeEl("msg-time", now, "9:10 AM");
  freshenDayTitle({ target: time });
  assert.match(time.attrs.get("title"), /^Today, .*, 9:10 AM$/);
  assert.equal(time.attrs.get("aria-label"), time.attrs.get("title"));
  const edited = timeEl(EDITED_MARK, now, "edited");
  freshenDayTitle({ target: edited });
  assert.match(edited.attrs.get("title"), /^Edited Today, /);
  assert.equal(edited.attrs.get("aria-label"), "stale", "an edited mark's name was worded as a time's");
  const other = timeEl("other", now, "x");
  freshenDayTitle({ target: other });
  assert.equal(other.attrs.get("title"), "stale");
  assert.equal(rowTime(null), null);
});

test("`#624 voice-page-modularity`: where a search's words are in a row, from the module alone", () => {
  const { hitRanges, textHits, textRunsOf, findableParts, anchorsAmong } = CHANNEL_VIEW;
  assert.deepStrictEqual(hitRanges("abcabc", ["bc", "ca"]), [[1, 6]], "overlapping hits were not joined");
  // A character whose lower case is longer maps back to where it stands in the text.
  assert.deepStrictEqual(textHits("İx", ["x"]), [[1, 2]]);
  const body = node("div", [node("p", ["one ", node("em", ["two"])]), node("p", ["three"])]);
  assert.deepStrictEqual(textRunsOf(body).map((run) => run.map(({ node: piece }) => piece.textContent)), [["one ", "two"], ["three"]]);
  const row = { children: [
    { className: "meta", children: [{ className: "msg-author", children: [] }] },
    { className: "msg-text", children: [{ tagName: "A", children: [] }, { tagName: "P", children: [{ tagName: "A", children: [] }] }] },
  ] };
  const { parts, author } = findableParts(row);
  assert.equal(parts.length, 1);
  assert.equal(author.className, "msg-author");
  assert.equal(anchorsAmong(parts).length, 2);
});

test("`#624 voice-page-modularity`: a drawn row read back, from the module alone", () => {
  const { idsOf, rowMessages, rowSentMs, shownRowFor, standingRow, authorLabel, isMessageRow } = CHANNEL_VIEW;
  const row = (attrs, extra = {}) => ({ getAttribute: (name) => (name in attrs ? attrs[name] : null), className: "", hidden: false, ...extra });
  assert.deepStrictEqual(idsOf(row({ "data-ids": "1 2  3" })), ["1", "2", "3"]);
  assert.deepStrictEqual([idsOf(null), rowMessages(null)], [[], []]);
  assert.equal(rowSentMs(row({}, { messages: [{ timestamp: "2026-10-12T00:00:00Z" }] })), Date.parse("2026-10-12T00:00:00Z"));
  assert.equal(authorLabel(row({ "data-author": "Ada", "data-author-bot": "true" })), "Ada (bot)");
  assert.equal(isMessageRow(row({ "data-search": "" })), true);
  // A member of a collapsed run stands for its run's head row, which is shown unless hidden.
  const head = row({ "data-read-run": "head" });
  const member = row({ "data-read-run": "member" });
  const parent = { children: [head, member] };
  member.parentNode = parent;
  assert.equal(shownRowFor(member), head);
  assert.equal(standingRow(member), head);
  head.hidden = true;
  assert.equal(standingRow(member), null);
});

test("`#624 voice-page-modularity`: a drawn body's text, and a card's body cut into units, from the module alone", () => {
  const { renderedText, cardUnits, tableShape, cardStubText, cardMarkTarget } = CHANNEL_VIEW;
  // Stand-in nodes as `node` makes them, with nodeType 1 children.
  const body = node("div", [node("p", ["Plan ", node("strong", ["now"])]), node("ul", [node("li", ["one"]), node("li", ["two"])]),
    node("table", [node("tr", [node("th", ["a"]), node("th", ["b"])]), node("tr", [node("td", ["1"]), node("td", ["2"])])])]);
  // A line end at each block's edge and each row; a cell is not a block, so a row's cells run together.
  assert.equal(renderedText(body), "Plan now\none\ntwo\nab\n12");
  const units = cardUnits(body);
  // The paragraph, each item of the list, and the table whole.
  assert.deepStrictEqual(units.map(({ lines, mark, whole }) => [lines, mark, whole]), [[1, 0, false], [1, 0, false], [1, 0, false], [2, 1, true]]);
  assert.deepStrictEqual(tableShape(body.childNodes[2]), { rows: 2, columns: 2 });
  assert.equal(cardStubText(body.childNodes[2]), "Table (2 columns × 2 rows)");
  assert.equal(cardMarkTarget(body, false).tagName, "P");
  assert.equal(cardMarkTarget(body, true), null, "a mark went after a table, which has no line of words to end");
});

test("`#624 voice-page-modularity`: the links between Main's roots and their cards, from the module alone", () => {
  const { laneOrder, linkAt, rowLinkY, pointerOn, LINK_ROW_INSET_PX } = CHANNEL_VIEW;
  const link = (name, from, to) => ({ name, from: { y: from }, to: { y: to } });
  // Rising first, the higher root further left; then falling, the higher root further right.
  assert.deepStrictEqual(laneOrder([link("f1", 10, 50), link("r1", 80, 20), link("f2", 30, 60), link("r2", 40, 5)]).map(({ name }) => name),
    ["r2", "r1", "f2", "f1"]);
  assert.deepStrictEqual([linkAt(10.3, 0), linkAt(10.8, 0.1)], [10.5, 10.5]);
  assert.equal(rowLinkY({}, { top: 100, height: 60, bottom: 160 }), 100 + LINK_ROW_INSET_PX);
  assert.equal(pointerOn({ matches: () => { throw new Error("no :hover here"); } }), false);
  assert.equal(pointerOn(null), false);
});

test("`#624 voice-page-modularity`: where a gathered reply's stack hangs, and the circle's lift, from the module alone", () => {
  const { stackUnder, setMarkerLift } = CHANNEL_VIEW;
  const attrs = (map) => ({ getAttribute: (name) => map.get(name) ?? null, hasAttribute: (name) => map.has(name),
    setAttribute: (name, value) => map.set(name, value), removeAttribute: (name) => map.delete(name), map });
  assert.deepStrictEqual([
    stackUnder(attrs(new Map([["data-who", "me"], ["data-is-reply", "true"]]))),
    stackUnder(attrs(new Map([["data-who", "coder"]]))),
    stackUnder(attrs(new Map([["data-who", "someone"]]))),
  ], ["me-reply", "coder", "third"]);
  const row = attrs(new Map());
  setMarkerLift(row, true);
  assert.equal(row.map.get("data-reply-lift"), "true");
  setMarkerLift(row, false);
  assert.equal(row.map.has("data-reply-lift"), false);
});

test("`#624 voice-page-modularity`: a card's layout marks and what it stands for in a run, from the module alone", () => {
  const { markCardLayout, cardRunIds, cardGapTally } = CHANNEL_VIEW;
  const marks = new Set();
  const row = { hasAttribute: (name) => marks.has(name), toggleAttribute: (name, on) => (on ? marks.add(name) : marks.delete(name)) };
  markCardLayout(row, "pending", { whole: true });
  assert.deepStrictEqual([...marks].sort(), ["data-card-clamp", "data-summary-pending"]);
  markCardLayout(row, "split");
  assert.deepStrictEqual([...marks], ["data-split-card"]);
  const card = { hasAttribute: (name) => name === "data-card", getAttribute: () => "spaces/A/t" };
  assert.deepStrictEqual(cardRunIds(card), ["card:spaces/A/t"]);
  assert.deepStrictEqual(cardGapTally({ head: { cardMessages: 4, cardMessagesExact: false }, card: true }),
    { messages: 4, threads: 1, exact: false, onlyCards: true });
});

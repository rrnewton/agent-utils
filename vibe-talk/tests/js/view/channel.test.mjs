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

test("`#624 voice-page-modularity`: a row's tap folds it, or reads it aloud or jumps the reading in reading mode, and a gesture's claim on the click is let go once", () => {
  const { createRowGestures, HOLD_MS, SWIPE_COMMIT_PX, SWIPE_START_PX } = CHANNEL_VIEW;
  assert.deepStrictEqual([SWIPE_START_PX, SWIPE_COMMIT_PX, HOLD_MS], [12, 90, 450]);
  const did = [];
  let reading = false;
  let playing = null;
  const gestures = createRowGestures({
    inThisColumn: (fn) => fn, showingLinks: () => false, preparedBlocksOf: () => null,
    readingMode: () => reading, nowPlaying: () => playing, jumpReading: (index) => did.push(["jump", index]),
    readAloud: (ids) => did.push(["read", ids]), idsOf: () => ["7"], toggleDetails: () => did.push(["details"]),
    swiped: () => did.push(["swiped"]),
  });
  const row = { getAttribute: (name) => (name === "data-id" ? "7" : null) };
  const fold = { click: () => did.push(["fold"]) };
  gestures.actOnRow(row, fold);
  reading = true;
  gestures.actOnRow(row, fold);
  playing = { id: "7", segments: [{ part: 0, block: 0 }, { part: 0, block: 1 }], segment: 0 };
  gestures.actOnRow(row, fold, { part: 0, block: 1 });
  gestures.actOnRow(row, fold, { part: 0, block: 0 });
  assert.deepStrictEqual(did, [["fold"], ["read", ["7"]], ["jump", 1], ["read", ["7"]]]);
  assert.equal(gestures.takeClaimedClick(), false);
  gestures.claimClick();
  assert.equal(gestures.takeClaimedClick(), true);
  assert.equal(gestures.takeClaimedClick(), false, "a claim was let go twice");
});

test("`#624 voice-page-modularity`: a pull past the top arms after PULL_ARM_PX and refreshes on release; a sideways drag never does", async () => {
  const { createPullToRefresh, PULL_ARM_PX, PULL_LABELS } = CHANNEL_VIEW;
  const affordance = { hidden: true, attrs: {}, style: { setProperty() {} }, textContent: "",
    setAttribute(name, value) { this.attrs[name] = value; }, getAttribute(name) { return this.attrs[name] ?? null; } };
  const area = { scrollTop: 0 };
  const read = [];
  const pull = createPullToRefresh({
    byId: () => affordance, scrollArea: () => area, onChannelList: () => true, inChannelView: () => true,
    atBottom: () => false, fetchInFlight: () => false, olderInFlight: () => false, takeDeferredOlder: () => read.push("older"),
    undeferOlder: () => {}, loadDiscord: async (options) => { read.push(options.reason); }, newestId: () => null,
    setStatus: (text) => read.push(text), channelContextKey: () => "C", freshness: () => ({ freshness: "fresh", freshAt: 0 }),
    renderFreshness: () => {},
  });
  const touch = (x, y) => ({ touches: [{ clientX: x, clientY: y }], target: null });
  pull.pullStart(touch(10, 100));
  assert.equal(pull.fingerDown(), true);
  pull.pullMove(touch(10, 100 + PULL_ARM_PX - 1));
  assert.equal(affordance.textContent, PULL_LABELS.pull);
  pull.pullMove(touch(10, 100 + PULL_ARM_PX));
  assert.equal(affordance.textContent, PULL_LABELS.armed);
  await pull.pullEnd();
  assert.equal(pull.fingerDown(), false);
  assert.deepStrictEqual(read, ["pull", "refreshed — nothing new since the last read."]);
  // Mostly sideways: refused for the rest of the touch, and letting go reads nothing.
  read.length = 0;
  pull.pullStart(touch(10, 100));
  pull.pullMove(touch(200, 130));
  pull.pullMove(touch(200, 100 + PULL_ARM_PX * 2));
  await pull.pullEnd();
  assert.deepStrictEqual(read, ["older"], "a sideways drag refreshed");
});

test("`#624 voice-page-modularity`: the author filter's rows, button and chip, from the module alone", () => {
  const { authorPasses, rowIsMine, createAuthorFilter, AUTHOR_CHIP_MS } = CHANNEL_VIEW;
  const row = (attrs, className = "") => ({ className, getAttribute: (name) => attrs[name] ?? null });
  const mine = row({ "data-who": "me", "data-search": "" });
  const sending = row({ "data-search": "" }, "msg mine");
  const theirs = row({ "data-who": "coder", "data-search": "" });
  const notice = row({});
  const card = row({ "data-search": "", "data-who": "me" }, "thread-card");
  assert.deepStrictEqual([mine, sending, theirs].map(rowIsMine), [true, true, false]);
  assert.deepStrictEqual([mine, theirs, notice, card].map((one) => authorPasses(one, "everyone")), [true, true, true, true]);
  assert.deepStrictEqual([mine, sending, theirs, notice, card].map((one) => authorPasses(one, "mine")), [true, true, false, false, false]);
  assert.deepStrictEqual([mine, theirs, notice, card].map((one) => authorPasses(one, "theirs")), [false, true, false, false]);
  const element = () => {
    const attrs = new Map();
    return { attrs, hidden: true, className: "author-chip", textContent: "",
      getAttribute: (name) => attrs.get(name) ?? null, setAttribute: (name, value) => attrs.set(name, String(value)) };
  };
  const elements = { "author-filter": element(), "author-filter-tip": element(), "author-filter-chip": element() };
  let open = false;
  const filter = createAuthorFilter({ lookup: (id) => elements[id], searchOpen: () => open });
  assert.deepStrictEqual([filter.chosen(), filter.inForce()], ["everyone", "everyone"]);
  filter.set("mine");
  assert.equal(filter.inForce(), "everyone", "a filter in force with the bar shut");
  open = true;
  assert.equal(filter.inForce(), "mine");
  const button = elements["author-filter"].attrs;
  assert.deepStrictEqual([button.get("data-author"), button.get("aria-pressed"), button.get("aria-describedby")],
    ["mine", "true", "author-filter-tip"]);
  assert.equal(elements["author-filter-tip"].textContent, button.get("title"));
  const realTimeout = globalThis.setTimeout;
  const timers = [];
  globalThis.setTimeout = (run, ms) => { timers.push({ run, ms }); return timers.length; };
  try {
    filter.showChip();
    const chip = elements["author-filter-chip"];
    assert.deepStrictEqual([chip.hidden, chip.textContent, timers[0].ms], [false, "My messages", AUTHOR_CHIP_MS]);
    timers[0].run();
    assert.equal(chip.className, "author-chip author-chip-out");
    timers[1].run();
    assert.deepStrictEqual([chip.hidden, chip.className], [true, "author-chip"]);
  } finally {
    globalThis.setTimeout = realTimeout;
  }
});

test("`#624 voice-page-modularity`: a card's neighbours, whether it stayed beside one, and two places compared, from the module alone", () => {
  const { rowNeighbours, neighbourStayed, sameCardPlace } = CHANNEL_VIEW;
  const card = (id) => ({ getAttribute: (name) => (name === "data-id" ? id : null) });
  const was = rowNeighbours(["a", "b", "c", "d"].map(card));
  assert.deepStrictEqual(was.get("a"), { prev: null, next: "b" });
  assert.deepStrictEqual(was.get("c"), { prev: "b", next: "d" });
  // "b" re-sorted to the foot: it stands beside neither neighbour; "c" keeps "d"; "e" was never noted.
  const now = rowNeighbours(["a", "c", "d", "b", "e"].map(card));
  assert.equal(neighbourStayed("b", now, was), false);
  assert.equal(neighbourStayed("c", now, was), true);
  assert.equal(neighbourStayed("e", now, was), true, "a card whose neighbours were not noted did not stay");
  assert.equal(neighbourStayed("z", now, was), false, "a card not drawn stayed");
  assert.equal(sameCardPlace({ newest: true, id: null, offset: 0 }, { newest: true, id: "x", offset: 9 }), true);
  assert.equal(sameCardPlace({ newest: false, id: "a", offset: 10 }, { newest: false, id: "a", offset: 10.8 }), true);
  assert.equal(sameCardPlace({ newest: false, id: "a", offset: 10 }, { newest: false, id: "a", offset: 12 }), false);
  assert.equal(sameCardPlace(null, { newest: true, id: null, offset: 0 }), false);
});

test("`#624 voice-page-modularity`: what the reader used last, said on the root, and a mouse really moving, from the module alone", () => {
  const { createInputKinds } = CHANNEL_VIEW;
  const attrs = new Map();
  const root = { getAttribute: (n) => attrs.get(n) ?? null, setAttribute: (n, v) => attrs.set(n, v),
    hasAttribute: (n) => attrs.has(n), removeAttribute: (n) => attrs.delete(n) };
  const input = createInputKinds({ root: () => root });
  assert.deepStrictEqual([input.kind(), input.drawnBy(), input.mouseAt()], ["", false, null]);
  input.keys();
  assert.deepStrictEqual([input.kind(), input.drawnBy(), attrs.get("data-input")], ["keys", true, "keys"]);
  // The first pointermove while the keys are in use counts only if it says it moved.
  assert.equal(input.mouseMoved({ pointerType: "mouse", clientX: 5, clientY: 5, movementX: 0, movementY: 0 }), false);
  assert.equal(input.mouseMoved({ pointerType: "mouse", clientX: 5, clientY: 5 }), false, "the same place counted as a move");
  assert.equal(input.mouseMoved({ pointerType: "mouse", clientX: 6, clientY: 5 }), true);
  assert.equal(input.mouseMoved({ pointerType: "pen", clientX: 9, clientY: 9 }), false, "a pen counted as a mouse");
  assert.deepStrictEqual(input.mouseAt(), { x: 6, y: 5 });
  input.mouse();
  assert.deepStrictEqual([input.kind(), attrs.has("data-input")], ["mouse", false]);
  input.touch();
  assert.deepStrictEqual([input.kind(), input.drawnBy(), attrs.get("data-input")], ["touch", false, "touch"]);
  const fresh = createInputKinds({ root: () => root });
  assert.equal(fresh.mouseMoved({ pointerType: "mouse", clientX: 1, clientY: 1 }), true, "the first move before the keys did not count");
});

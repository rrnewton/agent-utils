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

test("`#624 voice-page-modularity`: thread colours are remembered per channel, the server's win, and a new one keeps its distance", async () => {
  const { createThreadHues, THREAD_HUES_KEY, THREAD_HUES_SEEDED_KEY } = CHANNEL_VIEW;
  const store = new Map();
  let channel = "C1";
  const posts = [];
  const make = () => createThreadHues({
    storage: { getItem: (key) => store.get(key) ?? null, setItem: (key, value) => store.set(key, value), removeItem: (key) => store.delete(key) },
    now: () => 1000,
    channelRecord: (id) => (id === "C1" ? { provider: "slack" } : undefined),
    selected: () => channel,
    writeToken: () => true,
    held: () => ({ channel, threads: [], messages: [] }),
    timeOf: () => NaN,
    threadOf: () => null,
    post: async (where, hues) => { posts.push([where, hues]); return { hues: { T1: 17 } }; },
  });
  const turn = () => new Promise((resolve) => setImmediate(resolve));
  const hues = make();
  const first = hues.hue("T1");
  const second = hues.hue("T2");
  assert.equal(first, 210, "the first thread on an empty wheel");
  assert.ok(Math.min(Math.abs(first - second), 360 - Math.abs(first - second)) >= 45, "a new thread keeps its distance");
  assert.equal(hues.hue("T1"), first, "a thread keeps its colour");
  await turn();
  const scope = JSON.stringify(["slack", "C1"]);
  assert.deepStrictEqual(JSON.parse(store.get(THREAD_HUES_KEY)).channels[scope], { T1: [first, 1000], T2: [second, 1000] });
  const again = make();
  again.restore("C1");
  assert.equal(again.hue("T2"), second, "a reload puts the colours back");
  again.noteServerIn([{ thread: { id: "T2", hue: 33 } }], [], "C1");
  assert.equal(again.hue("T2"), 33, "the server's colour wins");
  again.noteServerIn([{ thread: { id: "T2", hue: 99 } }], [], "C1");
  assert.equal(again.hue("T2"), 33, "and is never recoloured");
  const seeding = make();
  await seeding.seed("C1");
  await seeding.seed("C1");
  assert.equal(posts.length, 1, "colours are offered once a load");
  assert.equal(posts[0][0], "C1");
  assert.ok(JSON.parse(store.get(THREAD_HUES_SEEDED_KEY)).scopes[scope] !== undefined);
  await make().seed("C1");
  assert.equal(posts.length, 1, "a scope the server took is not offered again");
  assert.equal(seeding.hue("T1"), 17, "the answer's colours are adopted");
  store.set(THREAD_HUES_KEY, JSON.stringify({ v: 2, channels: {} }));
  const other = make();
  channel = "C2";
  other.hue("T9");
  await turn();
  assert.equal(JSON.parse(store.get(THREAD_HUES_KEY)).v, 2, "another version's memory is not overwritten");
});

test("`#624 voice-page-modularity`: pins are read only for a revision not held, changed at once, rolled back where refused, and said", async () => {
  const { createPinSync } = CHANNEL_VIEW;
  const { createPins } = await moduleForTests("src/core/annotations/index.ts");
  const pins = createPins();
  const said = [];
  const reads = [];
  const writes = [];
  let drawn = 0;
  let refuse = null;
  let list = { channel_id: "C1", revision: 3, limit: 50, pins: [] };
  const guarded = [];
  const sync = createPinSync({
    pins,
    channel: () => "C1",
    forget: (channel) => pins.forget(channel),
    read: async (channel) => { reads.push(channel); return list; },
    write: async (channel, id, request) => {
      writes.push([id, request === null ? "DELETE" : "PUT"]);
      if (refuse) throw refuse;
      const pin = request ? { message_id: id, ...request(), content_html: "", truncated: false, pinned_at_ms: 1 } : null;
      list = { ...list, revision: list.revision + 1, pins: pin ? [...list.pins, pin] : list.pins.filter((one) => one.message_id !== id) };
      return { revision: list.revision, pin, unpinned: [] };
    },
    drawn: () => { drawn += 1; },
    guard: (run) => { guarded.push(run()); },
    online: () => true,
    status: (text) => said.push(text),
    service: () => "Slack",
    now: () => 1000,
  });
  sync.noteRevision(3);
  await Promise.all(guarded);
  assert.deepStrictEqual(reads, ["C1"], "a revision not held is read");
  sync.noteRevision(3);
  assert.equal(guarded.length, 1, "the revision held is not read again");
  const message = { id: "M1", author: "Ada", author_id: "U1", content: "hello", timestamp: "2026-11-02T00:00:00Z" };
  await sync.toggle([message]);
  await Promise.all(guarded);
  assert.deepStrictEqual(writes, [["M1", "PUT"]]);
  assert.deepStrictEqual(reads, ["C1", "C1"], "a revision moved by the change is read again");
  assert.equal(pins.isPinned("C1", "M1"), true);
  assert.equal(said.at(-1), "Pinned here, not in Slack.");
  refuse = { detail: "the store is full" };
  await sync.toggle([message]);
  assert.equal(pins.isPinned("C1", "M1"), true, "a refused unpin is rolled back");
  assert.equal(said.at(-1), "That message was not unpinned: the store is full");
  assert.ok(drawn >= 3);
  const kept = sync.kept({ message_id: "M9", author: "Bo", author_id: "U2", author_is_bot: false, content: "cut", content_html: "",
    truncated: true, timestamp: "t", thread_id: "T1", thread_root: false, pinned_at_ms: 1 }, "C1");
  assert.equal(kept.content, "cut…", "a shortened snapshot says so");
  assert.equal(kept.thread.id, "T1");
  assert.equal(sync.isKept(kept), true);
  assert.equal(sync.isKept(message), false);
  assert.equal(sync.isKept("M9"), false);
});

test("`#624 voice-page-modularity`: the rooms past the right column's foot open from a rested foot, are the foot while open, and close behind the reader", () => {
  const { createFootRooms, OVERSCROLL_REST_MS, OVERSCROLL_IDLE_MS } = CHANNEL_VIEW;
  const box = (top) => ({ style: { height: "" }, getBoundingClientRect: () => ({ top: top(), height: 0 }) });
  const rows = 1000;
  const area = {
    clientHeight: 400, clientTop: 0, scrollTop: 600,
    get scrollHeight() { return rows + (Number.parseFloat(overscrollBox.style.height) || 0) + (Number.parseFloat(overextendBox.style.height) || 0); },
    getBoundingClientRect: () => ({ top: 0 }),
  };
  const overscrollBox = box(() => rows - area.scrollTop);
  const overextendBox = box(() => rows + (Number.parseFloat(overscrollBox.style.height) || 0) - area.scrollTop);
  const card = { getBoundingClientRect: () => ({ top: rows - 100 - area.scrollTop, height: 100 }) };
  const timers = [];
  let context = "c1";
  const rooms = createFootRooms({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    side: () => area, overscrollBox: () => overscrollBox, overextendBox: () => overextendBox,
    context: () => context, offered: () => true, sideUp: () => true, lastCard: () => card,
    paddingBottom: () => 0, pageStep: () => 360,
  });
  assert.equal(rooms.footGap(area), 0, "at the foot");
  rooms.scrolled(1000);
  rooms.wheel({ timeStamp: 1000 + OVERSCROLL_REST_MS - 1, deltaY: 100, deltaMode: 0, ctrlKey: false });
  assert.equal(rooms.overscrollRoom(area), 0, "a turn that may have brought the reader to the foot opens nothing");
  rooms.wheel({ timeStamp: 1000 + 3 * OVERSCROLL_REST_MS, deltaY: 100, deltaMode: 0, ctrlKey: false });
  assert.equal(rooms.overscrollRoom(area), 100, "the last card's top can come to the middle: 200 less the 100 it stands above the end");
  assert.equal(area.scrollTop, 700, "the turn that opened it is scrolled into it");
  assert.equal(rooms.footGap(area), -100, "in the room, the foot is above");
  assert.equal(rooms.footTop(area), 600);
  assert.equal(timers.at(-1).ms, OVERSCROLL_IDLE_MS);
  rooms.followFoot(area);
  assert.equal(area.scrollTop, 700, "a follower in the room stays where they are");
  context = "c2";
  assert.equal(rooms.overscrollRoom(area), 0, "a room left open over something else closes as it is measured");
  assert.equal(overscrollBox.style.height, "");
  area.scrollTop = 600;
  rooms.sizeOverextend(area, 650);
  assert.equal(rooms.overextendRoom(area), 50, "a card held 50px past the foot gets 50px of room");
  area.scrollTop = 620;
  rooms.fitOverextend(area);
  assert.equal(rooms.overextendRoom(area), 20, "going up takes the room from under the reader");
  rooms.toFoot(area);
  assert.equal(rooms.overextendRoom(area), 0);
  assert.equal(area.scrollTop, rows, "to the foot");
});

test("`#624 voice-page-modularity`: a packed card's gap holds and closes by sliding what is below it on the screen, and ends at once", () => {
  const { createPackSlides } = CHANNEL_VIEW;
  const item = (attrs, top, height = 50) => {
    const map = new Map(Object.entries(attrs));
    const node = {
      tagName: "LI", id: "", hidden: false, className: "", played: [],
      getAttribute: (name) => (map.has(name) ? map.get(name) : null),
      hasAttribute: (name) => map.has(name),
      setAttribute: (name, value) => map.set(name, value),
      removeAttribute: (name) => map.delete(name),
      getBoundingClientRect: () => ({ top: top(), bottom: top() + height }),
      getClientRects: () => [{}],
      animate: (frames, options) => {
        const listeners = {};
        const motion = { frames, options, addEventListener: (type, run) => { listeners[type] = run; }, finish: () => listeners.finish() };
        node.played.push(motion);
        return motion;
      },
    };
    return node;
  };
  let shiftUp = 0;
  const card = item({ "data-card": "", "data-id": "T1" }, () => 0);
  const below = item({ "data-card": "", "data-id": "T2" }, () => 60 - shiftUp);
  const offscreen = item({ "data-card": "", "data-id": "T3" }, () => 2000);
  const cards = { children: [card, below, offscreen] };
  const list = { children: [cards] };
  const area = { clientHeight: 500, getBoundingClientRect: () => ({ top: 0, bottom: 500, height: 500 }) };
  const slides = createPackSlides({
    cards: () => cards, list: () => list, area: () => area,
    motionMs: (name) => (name === "--pack-slide" ? 200 : 100), replyDocked: () => false, hidden: () => false,
  });
  assert.equal(slides.itemAfterBlock(card), below);
  const was = { key: slides.slideKey(below), top: 160 };
  slides.slideIntoPlace(card, was);
  assert.equal(below.played.length, 1, "what is below on the screen slides");
  assert.equal(offscreen.played.length, 0, "what is off the screen does not");
  assert.equal(below.played[0].options.duration, 300, "packed: the gap holds, then closes");
  assert.equal(below.played[0].frames[0].transform, "translateY(100px)");
  assert.equal(card.hasAttribute("data-slide-over"), true, "the card is drawn over what slides");
  slides.finishPackSlide();
  assert.equal(card.hasAttribute("data-slide-over"), false, "ended at once, drawn in the list's order again");
  slides.slideIntoPlace(card, { key: "another item", top: 160 });
  assert.equal(below.played.length, 1, "an item that is not the one measured before moves nothing");
});

test("`#624 voice-page-modularity`: the reader is left on the message their eye was on across a change, followed only from the newest line", () => {
  const { createScrollKeeping } = CHANNEL_VIEW;
  let shift = 0;
  const row = (id, top) => ({ id, parentNode: {}, getAttribute: () => null, getBoundingClientRect: () => ({ top: top + shift, bottom: top + shift + 40 }) });
  const rows = [row("a", -80), row("b", -40), row("c", 0), row("d", 40)];
  const log = { id: "discord-log", children: rows };
  const area = { scrollTop: 300, getBoundingClientRect: () => ({ top: 0 }) };
  const chip = { hidden: true, getBoundingClientRect: () => ({ top: 0 }) };
  const els = { "discord-log": log, "pinned-log": { id: "pinned-log", children: [] }, "scroll-area": area, "load-older": chip, transcript: { children: [] } };
  let bottom = false;
  const calls = [];
  const keep = createScrollKeeping({
    el: (id) => els[id], onMain: () => true, currentView: () => "discord", channelView: () => "main", pinnedOnly: () => false,
    heldFilterRow: () => null, searchWalk: () => null, atBottom: () => bottom,
    scrollToNewest: () => calls.push("newest"), renderJumpNewest: () => calls.push("jump"),
    setJumpNewest: (on, view, turns) => calls.push(["chip", view, turns]),
  });
  assert.equal(keep.visibleList(), log);
  assert.equal(keep.scrollAnchor(area), rows[2], "the first row not scrolled off the top");
  keep.preservingScroll(() => { shift = 25; });
  assert.equal(area.scrollTop, 325, "a change above moved the row down 25px, and the reader with it");
  assert.deepStrictEqual(calls, ["jump"]);
  bottom = true;
  keep.preservingScroll(() => {});
  assert.equal(calls.at(-1), "newest", "a reader on the newest line follows it");
  keep.followIfPinned(false, 2);
  assert.deepStrictEqual(calls.at(-1), ["chip", "voice", 2], "a reader scrolled away is told, not moved");
  keep.followIfPinned(true, 1, "voice");
  assert.deepStrictEqual(calls.at(-1), ["chip", "voice", 1], "a turn for a list not on screen raises its chip");
  keep.followIfPinned(true, 1, "discord");
  assert.equal(calls.at(-1), "newest");
});

test("`#624 voice-page-modularity`: the card at the top of a thread moves up from its card, and each part ends with its own animation", () => {
  const { createCardFlight } = CHANNEL_VIEW;
  const node = (top) => {
    const attrs = new Map();
    const style = new Map();
    return {
      style: { setProperty: (name, value) => style.set(name, value), getPropertyValue: (name) => style.get(name) || "" },
      setAttribute: (name, value) => attrs.set(name, value), removeAttribute: (name) => attrs.delete(name),
      hasAttribute: (name) => attrs.has(name), getBoundingClientRect: () => ({ left: 10, top }),
    };
  };
  const timers = [];
  let ms = 300;
  let links = 0;
  let scheduled = 0;
  const flight = createCardFlight({
    clock: { setTimeout: (run, wait) => { timers.push({ run, wait }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    ms: () => ms, drawn: () => true, linksShown: () => false, drawLinks: () => { links += 1; }, scheduleLinks: () => { scheduled += 1; },
    frame: () => true,
  });
  const card = node(0);
  const pane = node(100);
  flight.flyThreadCard(card, { left: 10, top: 240 }, pane);
  assert.equal(card.hasAttribute("data-flying"), true);
  assert.equal(card.style.getPropertyValue("--flight-y"), "240px", "it starts where its card was");
  assert.equal(pane.hasAttribute("data-arriving"), true);
  assert.equal(timers[0].wait, 400, "the timer outlasts the stylesheet's length");
  flight.endCardFlightPart(card, { target: {} });
  assert.equal(card.hasAttribute("data-flying"), true, "an animation of something in the card is not its own");
  flight.endCardFlightPart(card, { target: card });
  assert.equal(card.hasAttribute("data-flying"), false);
  assert.equal(scheduled, 1);
  flight.endCardFlightPart(pane, { target: pane });
  assert.equal(timers[0], null, "the second part ending ends the flight");
  ms = 0;
  flight.flyThreadCard(card, { left: 10, top: 240 }, pane);
  assert.equal(card.hasAttribute("data-flying"), false, "less motion: nothing moves");
  ms = 300;
  flight.flyThreadCard(card, null, pane);
  assert.equal(card.hasAttribute("data-flying"), false, "no card on screen to come from");
  flight.flyThreadCard(card, { left: 10, top: 240 }, pane);
  flight.endCardFlight();
  assert.equal(card.hasAttribute("data-flying") || pane.hasAttribute("data-arriving"), false, "ended wherever it had got to");
  assert.equal(links, 0, "no line followed with the lines not drawn");
});

test("`#624 voice-page-modularity`: the backlog control asks twice, and disarms by itself after its window", () => {
  const { createArming } = CHANNEL_VIEW;
  const timers = [];
  let drawn = 0;
  const arming = createArming({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    ms: () => 4000, drawn: () => { drawn += 1; },
  });
  assert.equal(arming.armed(), false);
  arming.arm();
  assert.equal(arming.armed(), true);
  assert.equal(timers[0].ms, 4000);
  arming.arm();
  assert.equal(timers[0], null, "arming again starts the window afresh");
  timers[1].run();
  assert.equal(arming.armed(), false, "the window passed");
  arming.arm();
  arming.disarm();
  assert.equal(arming.armed(), false);
  assert.equal(timers[2], null);
  assert.equal(drawn, 5);
});

test("`#624 voice-page-modularity`: the code works on the left column except inside inColumn, which always puts it back", () => {
  const { createColumnWork } = CHANNEL_VIEW;
  const columns = { main: { name: "main", left: false }, side: { name: "side", left: false } };
  const work = createColumnWork(columns);
  assert.equal(work.active(), columns.main);
  assert.equal(work.side(), false);
  const seen = work.inColumn(columns.side, () => [work.active().name, work.side()]);
  assert.deepStrictEqual(seen, ["side", true]);
  assert.equal(work.active(), columns.main, "the left column is worked on again");
  assert.equal(columns.side.left, true, "the right column was worked on and left");
  assert.equal(columns.main.left, false);
  assert.throws(() => work.inColumn(columns.side, () => { throw new Error("drawn wrong"); }), /drawn wrong/);
  assert.equal(work.side(), false, "a throw still puts the left column back");
  assert.equal(work.inColumn(columns.main, () => "same"), "same", "the column already worked on runs as it is");
});

test("`#624 voice-page-modularity`: the layout on screen, and each place held for the way back, taken once", () => {
  const { createLayoutShown } = CHANNEL_VIEW;
  const shown = createLayoutShown();
  assert.deepStrictEqual([shown.layout(), shown.sideUp(), shown.fit()], ["one", false, false]);
  shown.setThreadList(true);
  assert.deepStrictEqual([shown.layout(), shown.sideUp(), shown.threadList()], ["list", true, true]);
  shown.setThreadList(false);
  shown.setColumns(true);
  assert.deepStrictEqual([shown.layout(), shown.sideUp(), shown.columns()], ["two", true, true]);
  assert.equal(shown.noteFit(true), true, "the window now draws two columns");
  assert.equal(shown.noteFit(true), false, "and saying so again changes nothing");
  assert.equal(shown.fit(), true);
  shown.askOneColumn({ view: "flat" });
  assert.deepStrictEqual(shown.takeOneColumn(), { view: "flat" });
  assert.equal(shown.takeOneColumn(), null, "the view asked for is shown once");
  for (const [hold, take] of [["holdListPlace", "takeListPlace"], ["holdListCards", "takeListCards"], ["holdLeftColumn", "takeLeftColumn"]]) {
    assert.equal(shown[take](), null, `${take}: nothing held yet`);
    shown[hold]({ channel: "c1" });
    assert.deepStrictEqual(shown[take](), { channel: "c1" }, `${take}: what was held`);
    assert.equal(shown[take](), null, `${take}: spent by the change that took it`);
  }
  const seen = [];
  shown.whileRestoring(() => seen.push(shown.restoring()));
  assert.throws(() => shown.whileRestoring(() => { throw new Error("drawn wrong"); }), /drawn wrong/);
  assert.deepStrictEqual([seen, shown.restoring()], [[true], false], "restoring only while putting a view back, throw or not");
});

test("`#624 voice-page-modularity`: the cards keep what they drew only under the same whole, and never once forgotten", () => {
  const { createCardsDrawn } = CHANNEL_VIEW;
  const drawn = createCardsDrawn();
  assert.equal(drawn.changed(""), false, "nothing drawn reads as drawn from nothing");
  assert.equal(drawn.changed("s1"), true);
  assert.equal(drawn.redraw("s1", "w1"), false, "the first pass keeps nothing");
  drawn.keep({ whole: "w1", threads: new Map([["t1", "card"]]), main: new Map() });
  assert.equal(drawn.changed("s1"), false, "the same signature draws nothing");
  assert.equal(drawn.redraw("s2", "w1"), true, "under the same whole, what was drawn is kept");
  assert.equal(drawn.kept().threads.get("t1"), "card");
  assert.equal(drawn.redraw("s3", "w2"), false, "under another whole it is not");
  drawn.forget();
  assert.equal(drawn.changed("s3"), true, "forgotten: the next pass draws whatever it is");
  assert.equal(drawn.redraw("s3", "w1"), false, "and keeps nothing");
  assert.deepStrictEqual(drawn.count(), { cards: 0, shown: 0 });
  drawn.setCount(5, 2);
  assert.deepStrictEqual(drawn.count(), { cards: 5, shown: 2 });
  const seen = [];
  drawn.whileOpening(() => seen.push(drawn.opening()));
  assert.throws(() => drawn.whileOpening(() => { throw new Error("drawn wrong"); }), /drawn wrong/);
  assert.deepStrictEqual([seen, drawn.opening()], [[true], false]);
});

test("`#624 voice-page-modularity`: a block is read as it nears the screen, through one observer, and at once where nothing can watch", () => {
  const { createUnpackWatch, UNPACK_NEAR_PX } = CHANNEL_VIEW;
  const read = [];
  const root = { name: "side-scroll" };
  const row = (id) => ({ getAttribute: (name) => (name === "data-unpack-thread" ? id : null) });
  assert.equal(typeof globalThis.IntersectionObserver, "undefined", "the premise: node has no observer");
  createUnpackWatch({ root: () => root, read: (id) => read.push(id) }).watch(row("t0"), "t0");
  assert.deepStrictEqual(read, ["t0"], "nothing can watch: read at once");
  const made = [];
  globalThis.IntersectionObserver = class {
    constructor(callback, options) {
      this.callback = callback;
      this.options = options;
      this.watched = new Set();
      made.push(this);
    }
    observe(target) { this.watched.add(target); }
    unobserve(target) { this.watched.delete(target); }
    disconnect() { this.watched.clear(); this.gone = true; }
  };
  try {
    const watch = createUnpackWatch({ root: () => root, read: (id) => read.push(id) });
    const [a, b, c] = [row("t1"), row("t2"), row("t3")];
    watch.watch(a, "t1");
    watch.watch(b, "t2");
    assert.equal(made.length, 1, "one observer for every block");
    assert.deepStrictEqual(made[0].options, { root, rootMargin: `${UNPACK_NEAR_PX}px` });
    made[0].callback([{ isIntersecting: false, target: a }, { isIntersecting: true, target: b }]);
    assert.deepStrictEqual(read, ["t0", "t2"], "only the block near the screen is read");
    assert.deepStrictEqual([...made[0].watched], [a], "and watched no more");
    watch.unwatch(a);
    assert.equal(made[0].watched.size, 0);
    watch.stop();
    assert.equal(made[0].gone, true);
    watch.watch(c, "t3");
    assert.equal(made.length, 2, "stopped, the next block makes a new observer");
  } finally {
    delete globalThis.IntersectionObserver;
  }
});

test("`#624 voice-page-modularity`: the cards note when read runs stand suspended, and dress unpacked rows later only inside the pass", () => {
  const { createCardsDrawn } = CHANNEL_VIEW;
  const drawn = createCardsDrawn();
  assert.equal(drawn.noteSuspended(false), false, "drawn unsuspended, and still so");
  assert.equal(drawn.noteSuspended(true), true, "a filter now chooses rows");
  assert.equal(drawn.noteSuspended(true), false, "and it was noted");
  assert.equal(drawn.dressedAfter(), false);
  const seen = [];
  drawn.whileDressedAfter(true, () => seen.push(drawn.dressedAfter()));
  drawn.whileDressedAfter(false, () => seen.push(drawn.dressedAfter()));
  assert.throws(() => drawn.whileDressedAfter(true, () => { throw new Error("drawn wrong"); }), /drawn wrong/);
  assert.deepStrictEqual([seen, drawn.dressedAfter()], [[true, false], false]);
});

test("`#624 voice-page-modularity`: what stands under the lists, as each keeper noted it, and the send card's moves told to Main's", () => {
  const { createUnderLists } = CHANNEL_VIEW;
  const under = createUnderLists();
  under.cardMoved(40);
  under.cardCame(12);
  assert.deepStrictEqual([under.main(), under.side()], [null, null], "nothing before the first report");
  under.noteMain({ list: 600, dock: 50 });
  under.cardMoved(246);
  assert.deepStrictEqual(under.main(), { list: 600, dock: 296 }, "a move of the card is no growth");
  under.cardCame(12);
  assert.deepStrictEqual(under.main(), { list: 600, dock: 296, gap: 12 });
  under.noteSide({ list: 500, dock: 50 });
  assert.deepStrictEqual(under.side(), { list: 500, dock: 50 }, "the right column's note is its own");
});

test("`#624 voice-page-modularity`: which rows are being built is said only while they are, and their builder's answer comes back", () => {
  const { createRowDrawing } = CHANNEL_VIEW;
  const drawing = createRowDrawing();
  assert.deepStrictEqual([drawing.pinned(), drawing.main()], [false, false]);
  assert.deepStrictEqual(drawing.whilePinned(() => [drawing.pinned(), drawing.main()]), [true, false]);
  assert.deepStrictEqual(drawing.whileMain(() => [drawing.pinned(), drawing.main()]), [false, true]);
  assert.throws(() => drawing.whileMain(() => { throw new Error("drawn wrong"); }), /drawn wrong/);
  assert.deepStrictEqual([drawing.pinned(), drawing.main()], [false, false], "a throw still stops it");
});

test("`#624 voice-page-modularity`: a filter change nested in another is one change, and each walk back knows whether it is still the one under way", () => {
  const { createFilterChanges } = CHANNEL_VIEW;
  const changes = createFilterChanges();
  assert.equal(changes.changing(), false);
  const seen = changes.whileChanging(() => [changes.changing(), changes.whileChanging(() => changes.changing())]);
  assert.deepStrictEqual([seen, changes.changing()], [[true, true], false]);
  assert.throws(() => changes.whileChanging(() => { throw new Error("placed wrong"); }), /placed wrong/);
  assert.equal(changes.changing(), false, "a throw still ends the change");
  assert.equal(changes.endWalk(), false, "no walk to end");
  const first = changes.startWalk("c1");
  assert.deepStrictEqual([first.context, changes.walking(), changes.isWalk(first)], ["c1", true, true]);
  const second = changes.startWalk("c1");
  assert.deepStrictEqual([changes.isWalk(first), changes.isWalk(second)], [false, true], "overtaken by another");
  assert.equal(changes.endWalk(), true);
  assert.deepStrictEqual([changes.walking(), changes.isWalk(second)], [false, false]);
});

test("`#624 voice-page-modularity`: one row menu open at a time, and only the open one's close forgets it", () => {
  const { createRowMenus } = CHANNEL_VIEW;
  const menus = createRowMenus();
  const closedBy = [];
  const menu = (name) => {
    const close = () => { closedBy.push(name); menus.closed(close); };
    return { root: { name }, close, button: { name } };
  };
  const [a, b] = [menu("a"), menu("b")];
  assert.equal(menus.open(), null);
  menus.opened(a);
  assert.equal(menus.open(), a);
  menus.opened(a);
  assert.deepStrictEqual(closedBy, [], "opening the open one again closes nothing");
  menus.opened(b);
  assert.deepStrictEqual([closedBy, menus.open()], [["a"], b], "opening another closes the one open");
  menus.closed(a.close);
  assert.equal(menus.open(), b, "a menu already closed forgets nothing");
  b.close();
  assert.equal(menus.open(), null);
});

test("`#624 voice-page-modularity`: the open reply's record starts with no reply open, from its column, and each record is its own", () => {
  const { createReplyState } = CHANNEL_VIEW;
  const main = { name: "main" };
  const state = createReplyState(main);
  assert.deepStrictEqual(state, {
    target: null, channelId: null,
    routing: { thread: null, branch: null, into: null, unknown: false, removed: false, note: "" },
    column: main, docked: false, pending: null, focusTaken: false, quote: null, list: null, gathered: false, anchor: null,
  });
  // The page writes it as the reply opens and moves; a second record shares nothing with the first.
  state.routing.note = "a new thread";
  state.docked = true;
  const other = createReplyState(main);
  assert.deepStrictEqual([other.routing.note, other.docked], ["", false], "two records share their routing or their state");
});

test("`#624 voice-page-modularity`: the pane the keys moved last starts as the first, and is the one they moved since", () => {
  const { createLastPane } = CHANNEL_VIEW;
  const [main, side] = [{ name: "main" }, { name: "side" }];
  const last = createLastPane(main);
  assert.equal(last.current(), main);
  last.take(side);
  assert.equal(last.current(), side);
  last.take(main);
  assert.equal(last.current(), main);
});

test("`#624 voice-page-modularity`: one summary timer for the soonest moment any source's cards are owed, armed again only when it moves", () => {
  const { createSummaryWatch } = CHANNEL_VIEW;
  const timers = [];
  let fired = 0;
  const watch = createSummaryWatch({
    setTimer: (run, ms) => { const timer = { run, ms, cleared: false }; timers.push(timer); return timer; },
    clearTimer: (timer) => { timer.cleared = true; },
    fired: () => { fired += 1; },
  });
  watch.owe("cards", 5000, 1000);
  assert.deepStrictEqual([timers.length, timers[0].ms, watch.at()], [1, 4000, 5000]);
  watch.owe("thread-card", 9000, 1000);
  assert.equal(timers.length, 1, "a later moment armed another timer");
  watch.owe("thread-card", 3000, 1000);
  assert.deepStrictEqual([timers.length, timers[0].cleared, timers[1].ms], [2, true, 2000], "a sooner moment did not move the timer");
  timers[1].run();
  assert.deepStrictEqual([fired, watch.at()], [1, Infinity]);
  // Fired, every source is forgotten: the next one told arms the timer alone.
  watch.owe("cards", Infinity, 2000);
  assert.equal(timers.length, 2, "nothing owed armed a timer");
});

test("`#624 voice-page-modularity`: the search's and the filter chips' listeners toggle each filter, type a filter change in place, and Enter and Escape act on the field", async () => {
  const { mountFilterChips } = CHANNEL_VIEW;
  const { LINK_KINDS } = await moduleForTests("src/present/rows/index.ts");
  const log = [];
  const elements = new Map();
  const el = (id) => {
    if (!elements.has(id)) {
      const listeners = [];
      elements.set(id, { id, value: "", listeners,
        addEventListener: (type, listener) => listeners.push({ type, listener }),
        removeEventListener: (type, listener) => {
          const at = listeners.findIndex((one) => one.type === type && one.listener === listener);
          if (at >= 0) listeners.splice(at, 1);
        } });
    }
    return elements.get(id);
  };
  const fire = (id, type, event = {}) => { for (const one of el(id).listeners.filter((each) => each.type === type)) one.listener(event); };
  const state = { open: false, pinned: false, links: false, hidden: new Set(["pr"]), raw: false, query: "", typed: true, side: false, older: false };
  const chips = mountFilterChips({
    byId: el,
    searchOpen: () => state.open,
    setSearchOpen: (open) => log.push(`search ${open}`),
    pinnedOnly: () => state.pinned,
    setPinnedOnly: (...args) => log.push(`pinned ${args.join(" ")}`),
    linksOnly: () => state.links,
    setLinksOnly: (on) => log.push(`links ${on}`),
    cycleAuthorFilter: (...args) => log.push(`author ${args.length}`),
    linkKindHidden: (kind) => state.hidden.has(kind),
    setLinkKindShown: (kind, shown) => log.push(`kind ${kind} ${shown}`),
    linksRaw: () => state.raw,
    setLinksRaw: (raw) => log.push(`raw ${raw}`),
    keepingFilterPlace: (change) => { log.push("keep place ("); change(); log.push(")"); },
    liftingHideRead: (change) => { log.push("lift ("); change(); log.push(")"); },
    // The search keeps its own form of the words, so what is asked of it afterwards is its query, not the field's text.
    setQuery: (text) => { state.query = `<${text}>`; log.push(`query ${text}`); },
    query: () => state.query,
    setMarked: (text) => log.push(`marked ${JSON.stringify(text)}`),
    searchTyped: (text) => { log.push(`typed? ${JSON.stringify(text)}`); return state.typed; },
    renderEveryColumnsTools: () => log.push("chips"),
    sideColumnUp: () => state.side,
    refilterSideCards: () => log.push("side cards"),
    markSearchSoon: () => log.push("mark soon"),
    markSearchNow: () => log.push("mark now"),
    searchOlderFromField: () => { log.push("older"); return state.older; },
    settleFocus: () => log.push("settle"),
    swallow: (event) => log.push(`swallow ${event.key}`),
  });
  const run = (act) => { log.length = 0; act(); return [...log]; };
  assert.deepStrictEqual([run(() => fire("search-toggle", "click")), run(() => fire("pinned-filter", "click")), run(() => fire("links-filter", "click")),
    run(() => fire("author-filter", "click", { type: "click" })), run(() => fire("link-raw", "click"))],
  [["search true"], ["pinned true"], ["links true"], ["author 0"], ["raw true"]], "a chip did not toggle, or the pin chip passed more than its one argument");
  assert.deepStrictEqual(LINK_KINDS.map(({ chip }) => run(() => fire(chip, "click"))[0]), LINK_KINDS.map(({ kind }) => `kind ${kind} ${kind === "pr"}`));
  let into = null;
  fire("link-kinds", "focusin", { target: { scrollIntoView: (options) => { into = options; } } });
  fire("link-kinds", "focusin", { target: null });
  assert.deepStrictEqual(into, { block: "nearest", inline: "nearest" });
  el("search-field").value = "foo";
  assert.deepStrictEqual(run(() => fire("search-field", "input")), ["keep place (", "lift (", "query foo", "typed? \"<foo>\"", "chips", ")", ")", "mark soon"]);
  state.typed = false;
  state.side = true;
  el("search-field").value = " ";
  assert.deepStrictEqual(run(() => fire("search-field", "input")), ["keep place (", "lift (", "query  ", "typed? \"< >\"", "marked \"< >\"", "chips", "side cards", ")", ")", "mark soon"],
    "a cleared search kept its marks, or the right column's cards were not filtered again");
  assert.deepStrictEqual([run(() => fire("search-field", "keydown", { key: "Enter" })), run(() => { state.older = true; fire("search-field", "keydown", { key: "Enter" }); }),
    run(() => fire("search-field", "keydown", { key: "Escape" })), run(() => fire("search-field", "keydown", { key: "Escape", isComposing: true })),
    run(() => fire("search-field", "keydown", { key: "Enter", isComposing: true })), run(() => fire("search-field", "keydown", { key: "a" }))],
  [["mark now", "older"], ["mark now", "older", "swallow Enter"], ["search false", "settle", "swallow Escape"], [], [], []]);
  chips.unmount();
  assert.deepStrictEqual([...elements.values()].filter((one) => one.listeners.length > 0).map((one) => one.id), [], "a listener was left mounted");
});

test("`#624 voice-page-modularity`: watching the lists answers a scroll at once, decides once per burst of resizes after the task, leaves the right column's own rooms unwatched, and stops", () => {
  const { mountListWatch } = CHANNEL_VIEW;
  const log = [];
  const element = (id, children = []) => {
    const listeners = [];
    return { id, children, listeners,
      addEventListener: (type, listener) => listeners.push({ type, listener }),
      removeEventListener: (type, listener) => {
        const at = listeners.findIndex((one) => one.type === type && one.listener === listener);
        if (at >= 0) listeners.splice(at, 1);
      } };
  };
  const area = element("scroll-area", [element("pane-discord"), element("pane-voice")]);
  const overscroll = element("side-overscroll");
  const overextend = element("side-overextend");
  const side = element("side-scroll", [element("side-log"), overscroll, overextend, element("side-cards")]);
  // The right column's elements are found only through leftById, as the page finds them in either column.
  const ids = { "scroll-area": area };
  const leftIds = { "side-scroll": side, "side-overscroll": overscroll, "side-overextend": overextend };
  const watchers = [];
  const asked = [];
  const tasks = [];
  const state = { resize: true, back: false, returnRow: null, onScreen: false, sideShowing: false };
  const deps = {
    byId: (id) => ids[id],
    leftById: (id) => leftIds[id],
    canWatchSizes: () => { asked.push("can watch?"); return state.resize; },
    sizeWatcher: (report) => {
      asked.push("size watcher");
      const watcher = { report, watched: [], stopped: false, observe(box) { this.watched.push(box.id); }, disconnect() { this.stopped = true; } };
      watchers.push(watcher);
      return watcher;
    },
    afterThisTask: (act) => tasks.push(act),
    placeChipsOverComposer: () => log.push("chips off the box"),
    renderJumpNewest: () => log.push("jump"),
    replyBackOffered: () => state.back,
    replyReturnRow: () => state.returnRow,
    dropReplyBack: () => log.push("drop back"),
    rowOnScreen: (row) => { log.push(`on screen? ${row}`); return state.onScreen; },
    renderScrollTools: () => log.push("tools"),
    maybeLoadOlder: () => log.push("older"),
    maybeLoadOlderTurns: () => log.push("older turns"),
    requestVisibleSummaries: (...args) => log.push(`summaries ${args.length}`),
    fitGrowingBoxes: () => log.push("fit boxes"),
    scheduleColumnLinks: () => log.push("links"),
    fitOverscroll: (on) => log.push(`fit room ${on.id}`),
    sideShowing: () => state.sideShowing,
    renderSideJumpNewest: () => log.push("side jump"),
  };
  const watch = mountListWatch(deps);
  const scroll = () => { log.length = 0; for (const one of area.listeners.filter((each) => each.type === "scroll")) one.listener({}); return [...log]; };
  const runTasks = () => { log.length = 0; for (const act of tasks.splice(0)) act(); return [...log]; };
  assert.deepStrictEqual(scroll(), ["chips off the box", "jump", "older", "older turns", "summaries 0"]);
  state.back = true;
  assert.deepStrictEqual(scroll(), ["chips off the box", "jump", "drop back", "tools", "older", "older turns", "summaries 0"], "a way back to a reply that has gone was kept");
  state.returnRow = "r";
  assert.deepStrictEqual(scroll(), ["chips off the box", "jump", "on screen? r", "older", "older turns", "summaries 0"], "a way back to a reply off screen was dropped");
  state.onScreen = true;
  assert.deepStrictEqual(scroll(), ["chips off the box", "jump", "on screen? r", "drop back", "tools", "older", "older turns", "summaries 0"]);
  assert.deepStrictEqual(watchers.map((one) => one.watched), [["scroll-area", "pane-discord", "pane-voice"], ["side-scroll", "side-log", "side-cards"]],
    "a list or a part of it went unwatched, or the right column's own rooms were watched");
  const [main, right] = watchers;
  main.report();
  main.report();
  right.report();
  assert.equal(tasks.length, 2, "a burst of resizes asked for more than one decision per list");
  assert.deepStrictEqual(runTasks(), ["jump", "chips off the box", "fit boxes", "links", "fit room side-scroll", "links"], "the decisions were made inside the report, or the right column jumped while not showing");
  state.sideShowing = true;
  right.report();
  main.report();
  assert.deepStrictEqual(runTasks(), ["fit room side-scroll", "side jump", "links", "jump", "chips off the box", "fit boxes", "links"], "a later burst was not decided again");
  watch.unmount();
  assert.deepStrictEqual([area.listeners.length, watchers.map((one) => one.stopped)], [0, [true, true]], "the scroll listener or a size watcher outlived unmount");
  state.resize = false;
  watchers.length = 0;
  asked.length = 0;
  const bare = mountListWatch(deps);
  assert.deepStrictEqual([asked, area.listeners.length], [["can watch?"], 1],
    "a browser with no ResizeObserver lost the scroll listener, or a watcher was asked of it");
  bare.unmount();
});

test("`#624 voice-page-modularity`: the keepers hold a reader on the newest line as the dock grows, keep the right column's reader on its foot only where nothing else holds it, measure the arrows once per burst, and stop", () => {
  const { mountListKeepers, createUnderLists } = CHANNEL_VIEW;
  const log = [];
  const box = (id, extra = {}) => ({ id, clientHeight: 500, clientWidth: 400, scrollTop: 100, scrollHeight: 2000, children: [], listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener, options }); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); }, ...extra });
  const main = box("scroll-area");
  const sideLog = box("side-log");
  const overscroll = box("side-overscroll");
  const overextend = box("side-overextend");
  const side = box("side-scroll", { children: [sideLog, overscroll, overextend] });
  const log2 = box("discord-log");
  const els = { "scroll-area": main, "discord-log": log2 };
  const lefts = { "side-scroll": side, "side-log": sideLog, "side-overscroll": overscroll, "side-overextend": overextend };
  const watchers = [];
  const timers = [];
  const state = { sizes: true, screen: "main", view: "discord", dock: 50, bottom: new Set(), quick: false, landing: false, filter: false,
    writing: false, written: false, sideUp: true, showing: true, mode: "thread", opening: false, columns: true, walk: false, held: null };
  const columns = { main: { name: "main", mode: "main" }, side: { name: "side", get mode() { return state.mode; } } };
  const was = globalThis.setTimeout;
  globalThis.setTimeout = (act) => { timers.push(act); return timers.length; };
  try {
    const underLists = createUnderLists();
    const keepers = mountListKeepers({
      canWatchSizes: () => state.sizes,
      sizeWatcher: (report) => { const watcher = { report, watched: [], stopped: false, observe(target) { this.watched.push(target.id); }, disconnect() { this.stopped = true; } }; watchers.push(watcher); return watcher; },
      el: (id) => els[id],
      leftEl: (id) => lefts[id],
      screens: { screen: () => state.screen, view: () => state.view },
      underLists,
      cardsDrawn: { opening: () => state.opening },
      columns,
      layoutShown: { columns: () => state.columns },
      bottomSlackPx: 24,
      heightUnderLists: () => state.dock,
      quickReplyHolds: () => state.quick,
      placeQuickReply: () => log.push("quick reply placed"),
      entryLandingHolds: () => state.landing,
      entryState: () => ({ landing: { row: "landing-row", first: "landing-first" } }),
      placeEntryLanding: (row, first) => log.push(`landing ${row} ${first}`),
      keepFilterHold: () => state.filter,
      heldFilterRow: () => state.held,
      searchWalkAnchored: () => state.walk,
      voiceCall: () => ({ writingToAgent: () => state.writing, keepComposerInView: () => log.push("call box kept") }),
      keepWrittenBox: (area) => { if (state.written) log.push(`box kept ${area.id}`); return state.written; },
      atBottom: (area, slack) => { log.push(`at bottom? ${area.id} ${slack}`); return state.bottom.has(area.id); },
      followFoot: (area) => log.push(`follow ${area.id}`),
      sideColumnUp: () => state.sideUp,
      sideShowing: () => state.showing,
      inColumn: (column, act) => { log.push(`in ${column.name}`); return act(); },
      fitOverextend: (area) => log.push(`fit room ${area.id}`),
      holdOverscrollReader: (area) => log.push(`hold ${area.id}`),
      channelContextKey: () => "c1",
      pinsView: () => ({ only: false }),
      measureReplyArrows: () => log.push("arrows"),
    });
    assert.deepStrictEqual(watchers.map((one) => one.watched), [["scroll-area"], ["side-scroll"], ["side-log"], ["discord-log"], ["side-log"]],
      "a list went unwatched, or the right column's own rooms were watched");
    const [mainDock, sideDock, footKeeper, arrows, sideArrows] = watchers;
    const run = (act) => { log.length = 0; act(); return [...log]; };
    // Main's list: the first report notes; the dock growing under a reader at the foot keeps them on it.
    assert.deepStrictEqual(run(() => mainDock.report()), []);
    state.dock = 92;
    state.bottom = new Set(["scroll-area"]);
    main.scrollTop = 100;
    assert.deepStrictEqual(run(() => mainDock.report()), ["at bottom? scroll-area 66"]);
    assert.equal(main.scrollTop, 2000, "a reader on the newest line was left above it as the dock grew");
    state.dock = 130;
    state.bottom = new Set();
    main.scrollTop = 100;
    assert.deepStrictEqual(run(() => mainDock.report()), ["at bottom? scroll-area 62"]);
    assert.equal(main.scrollTop, 100, "a reader up the list was moved");
    for (const [hold, said] of [["quick", "quick reply placed"], ["landing", "landing landing-row landing-first"]]) {
      state[hold] = true;
      state.dock += 10;
      assert.deepStrictEqual(run(() => mainDock.report()), [said], `the dock keeper did not leave ${hold} to its own rule`);
      state[hold] = false;
    }
    state.filter = true;
    state.dock += 10;
    assert.deepStrictEqual(run(() => mainDock.report()), [], "a filter's hold was moved");
    state.filter = false;
    state.writing = true;
    main.clientHeight = 300;
    assert.deepStrictEqual(run(() => mainDock.report()), ["call box kept"], "the call's box was not kept in view as the list shrank");
    state.writing = false;
    state.written = true;
    assert.deepStrictEqual(run(() => mainDock.report()), ["box kept scroll-area"]);
    state.written = false;
    state.screen = "settings";
    state.dock += 40;
    assert.deepStrictEqual(run(() => mainDock.report()), [], "growth off the main screen was followed");
    state.screen = "main";
    // `#269 approval-card-inline` x `#281 view-keys`. A send card coming up docked under the thread list: the reader at
    // Main's foot is judged by how far from it they were just before, not by how near it they are left.
    main.clientHeight = 500;
    mainDock.report();
    for (const [gap, kept] of [[20, true], [80, false]]) {
      underLists.cardCame(gap);
      state.dock += 60;
      main.scrollTop = 100;
      state.bottom = new Set(["scroll-area"]);
      assert.deepStrictEqual(run(() => mainDock.report()), [], "the card's coming was judged by where the reader is left");
      assert.equal(main.scrollTop, kept ? 2000 : 100, `a reader ${gap}px from the foot as the card came was ${kept ? "not " : ""}taken to it`);
    }
    state.bottom = new Set();
    // The right column under the same dock: the cards follow their foot unless they are opening; a thread, in its column.
    assert.deepStrictEqual(run(() => sideDock.report()), ["in side"], "the right column's first report did more than note");
    state.dock += 30;
    state.mode = "list";
    state.bottom = new Set(["side-scroll"]);
    assert.deepStrictEqual(run(() => sideDock.report()), ["at bottom? side-scroll 54", "follow side-scroll"]);
    state.opening = true;
    state.dock += 30;
    assert.deepStrictEqual(run(() => sideDock.report()), [], "the cards followed their foot while opening");
    state.opening = false;
    state.mode = "thread";
    state.dock += 30;
    assert.deepStrictEqual(run(() => sideDock.report()), ["in side", "at bottom? side-scroll 54", "follow side-scroll"]);
    state.landing = true;
    state.dock += 30;
    assert.deepStrictEqual(run(() => sideDock.report()), ["in side", "landing landing-row landing-first"]);
    state.landing = false;
    state.showing = false;
    state.dock += 30;
    assert.deepStrictEqual(run(() => sideDock.report()), [], "a column not showing was kept");
    state.showing = true;
    state.screen = "help";
    state.dock += 30;
    sideDock.report();
    state.screen = "main";
    state.bottom = new Set(["side-scroll"]);
    assert.deepStrictEqual(run(() => sideDock.report()), ["in side", "at bottom? side-scroll 54", "follow side-scroll"],
      "growth while another screen was up was not reckoned from the column as the reader left it");
    // The right column's foot keeper: a reader on the newest line, the column the same size and showing the same, is kept on it.
    const scrolled = () => { for (const one of side.listeners.filter((each) => each.type === "scroll")) one.listener(); };
    assert.deepStrictEqual(side.listeners.map((one) => [one.type, one.options]), [["scroll", { passive: true }]]);
    state.bottom = new Set(["side-scroll"]);
    footKeeper.report();
    scrolled();
    state.bottom = new Set();
    assert.deepStrictEqual(run(() => footKeeper.report()), ["fit room side-scroll", "hold side-scroll", "in side", "at bottom? side-scroll 1", "in side", "follow side-scroll", "at bottom? side-scroll undefined"]);
    scrolled();
    state.bottom = new Set(["side-scroll"]);
    footKeeper.report();
    side.clientWidth = 300;
    state.bottom = new Set();
    assert.ok(!run(() => footKeeper.report()).includes("follow side-scroll"), "a resize of the column was taken for its content changing");
    state.bottom = new Set(["side-scroll"]);
    footKeeper.report();
    state.walk = true;
    state.bottom = new Set();
    assert.ok(!run(() => footKeeper.report()).includes("follow side-scroll"), "a reader the search's walk holds was taken to the foot");
    state.walk = false;
    // The arrows: one measure per burst, after the task; the right column's only while it shows, in its column.
    arrows.report();
    arrows.report();
    sideArrows.report();
    assert.equal(timers.length, 2, "a burst asked for more than one measure");
    assert.deepStrictEqual(run(() => { for (const act of timers.splice(0)) act(); }), ["arrows", "in side", "arrows"]);
    state.showing = false;
    sideArrows.report();
    assert.deepStrictEqual(run(() => { for (const act of timers.splice(0)) act(); }), [], "the right column's arrows were measured while it was not showing");
    keepers.unmount();
    assert.deepStrictEqual([side.listeners.length, watchers.every((one) => one.stopped)], [0, true], "a listener or a watcher outlived unmount");
    // Without a ResizeObserver: nothing watched, and no scroll listener for the foot keeper either.
    mountListKeepers({ canWatchSizes: () => false, sizeWatcher: () => assert.fail("a watcher was made"), el: (id) => els[id], leftEl: (id) => lefts[id] });
    assert.equal(side.listeners.length, 0, "the foot keeper listened without a watcher to keep");
  } finally {
    globalThis.setTimeout = was;
  }
});

test("`#624 voice-page-modularity`: the lists' gestures and scrolls are every one passive, reach the act each names, and come down again", () => {
  const { mountListGestures } = CHANNEL_VIEW;
  const log = [];
  const target = (id) => ({ id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener, options }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const main = target("scroll-area");
  const leftMain = target("left-scroll-area");
  const side = target("side-scroll");
  const doc = target("document");
  const fire = (on, type, event) => { for (const one of on.listeners.filter((each) => each.type === type)) one.listener(event); };
  const was = globalThis.document;
  globalThis.document = doc;
  try {
    const gestures = mountListGestures({
      byId: (id) => ({ "scroll-area": main })[id],
      leftById: (id) => ({ "scroll-area": leftMain, "side-scroll": side })[id],
      pullStart: (event) => log.push(`pull start ${event.id}`),
      pullMove: (event) => log.push(`pull move ${event.id}`),
      pullEnd: (...args) => { log.push(`pull end ${args.length}`); return "the refresh"; },
      pullCancel: (...args) => log.push(`pull cancel ${args.length}`),
      scheduleColumnLinks: (...args) => log.push(`links ${args.length}`),
      footWheel: (event) => log.push(`foot wheel ${event.id}`),
      footScrolled: (timeStamp) => log.push(`foot scrolled ${timeStamp}`),
      finishPackSlide: (...args) => log.push(`slide ended ${args.length}`),
    });
    assert.deepStrictEqual(log, ["listen scroll-area touchstart passive", "listen scroll-area touchmove passive", "listen scroll-area touchend passive",
      "listen scroll-area touchcancel passive", "listen left-scroll-area scroll passive", "listen side-scroll scroll passive", "listen side-scroll wheel passive",
      "listen side-scroll wheel passive", "listen side-scroll keydown passive", "listen side-scroll scroll passive", "listen document visibilitychange"],
      "a listener was added out of the page's order, or one that only reads was not passive");
    log.length = 0;
    fire(main, "touchstart", { id: "t1" });
    fire(main, "touchmove", { id: "t2" });
    const ended = main.listeners.find((one) => one.type === "touchend").listener({ id: "t3" });
    assert.equal(ended, "the refresh", "the touchend listener did not answer the refresh it started, which the page suite waits for");
    fire(main, "touchcancel", { id: "t4" });
    fire(leftMain, "scroll", {});
    fire(side, "wheel", { id: "w" });
    fire(side, "keydown", {});
    fire(side, "scroll", { timeStamp: 42 });
    fire(side, "scroll", undefined);
    fire(doc, "visibilitychange", {});
    assert.deepStrictEqual(log, ["pull start t1", "pull move t2", "pull end 0", "pull cancel 0", "links 0", "foot wheel w", "slide ended 0", "slide ended 0",
      "links 0", "foot scrolled 42", "links 0", "foot scrolled -Infinity", "links 0"],
      "a gesture reached the wrong act, or a scroll with no time was not read as the earliest");
    gestures.unmount();
    assert.deepStrictEqual([main, leftMain, side, doc].map((one) => one.listeners.length), [0, 0, 0, 0], "a listener outlived unmount");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the list's tools reach the act each names, in the page's order, the action menu's items close it after their own acts, and come down again", () => {
  const { mountListControls } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [], checked: false,
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); },
    focus(options) { log.push(`focus ${id} ${options && options.preventScroll}`); } });
  const press = (id, type = "click") => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({}));
  let hidden = false;
  let inside = true;
  const controls = mountListControls({
    byId: (id) => target(id),
    leftById: (id) => target(`left ${id}`),
    loadOlder: () => { log.push("older"); return "older asked"; },
    searchInAll: () => log.push("search in All"),
    serverMore: () => log.push("server more"),
    searchAllChannels: (on) => log.push(`all channels ${on}`),
    searchFurther: () => log.push("further"),
    loadOlderTurns: () => log.push("older turns"),
    setAllFolded: (folded) => log.push(`folded ${folded}`),
    cycleReadMode: (...args) => log.push(`read mode ${args.length}`),
    clearBacklog: () => log.push("clear backlog"),
    undoDismissal: () => log.push("undo"),
    toggleSummaries: () => log.push("summaries"),
    readerHidden: () => hidden,
    setReaderHidden: (wanted) => { hidden = wanted; log.push(`reader hidden ${wanted}`); },
    closeSpeedPopover: () => log.push("pace closed"),
    focusInActionMenu: () => inside,
    closeActionMenu: () => log.push("menu closed"),
    jumpToNewest: () => log.push("newest"),
  });
  assert.deepStrictEqual(log, ["listen load-older click", "listen left search-in-all click", "listen server-more click",
    "listen search-all-channels change", "listen search-further click", "listen load-older-turns click", "listen collapse-all click",
    "listen expand-all click", "listen todo-filter click", "listen clear-backlog click", "listen undo-dismiss click", "listen summarise click",
    "listen reader-toggle click", "listen expand-all click", "listen collapse-all click", "listen summarise click", "listen reader-toggle click",
    "listen jump-newest click"], "a listener was added out of the page's order");
  log.length = 0;
  assert.deepStrictEqual(press("load-older"), ["older asked"], "the control did not answer what it started, which the page suite waits for");
  press("left search-in-all");
  press("server-more");
  nodes["search-all-channels"].checked = true;
  press("search-all-channels", "change");
  press("search-further");
  press("load-older-turns");
  press("todo-filter");
  press("clear-backlog");
  press("undo-dismiss");
  press("jump-newest");
  assert.deepStrictEqual(log, ["older", "search in All", "server more", "all channels true", "further", "older turns", "read mode 0",
    "clear backlog", "undo", "newest"]);
  log.length = 0;
  // The menu's items: each one's act, then the menu closed and the keys back on the hamburger where they were in it.
  press("expand-all");
  inside = false;
  press("collapse-all");
  press("summarise");
  inside = true;
  press("reader-toggle");
  press("reader-toggle");
  assert.deepStrictEqual(log, ["folded false", "menu closed", "focus action-menu-open true", "folded true", "menu closed", "summaries", "menu closed",
    "reader hidden true", "pace closed", "menu closed", "focus action-menu-open true", "reader hidden false", "menu closed", "focus action-menu-open true"],
    "an item's act came after the menu closed, Hide reader left the pace popover open, or Show reader closed it again");
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the thread's back gesture commits only a long enough swipe right in a thread, the right column's controls reach their acts in the page's order, and come down again", () => {
  const { mountSideControls, SWIPE_COMMIT_PX } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = {}) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  let inThread = true;
  let gesture = null;
  const controls = mountSideControls({
    byId: (id) => target(id),
    leftById: (id) => target(`left ${id}`),
    inThread: () => inThread,
    backGesture: () => gesture,
    setBackGesture: (start) => { gesture = start; },
    claimClick: () => log.push("click claimed"),
    closeThread: () => { log.push("thread closed"); return "closing"; },
    toggleColumns: () => log.push("columns"),
    closeSide: () => log.push("side closed"),
    threadCardIds: ["thread-card", "side-thread-card"],
    threadCardPressed: (card, event) => log.push(`card ${card.id} ${event.n}`),
    endCardFlightPart: (node, event) => log.push(`flight ${node.id} ${event.n}`),
    loadOlderThreads: () => log.push("older threads"),
    searchReplies: () => log.push("replies"),
    nextUnpack: () => log.push("unpack"),
    toggleMainRows: () => log.push("main"),
    nextOrder: () => log.push("order"),
  });
  assert.deepStrictEqual(log, ["listen scroll-area pointerdown", "listen scroll-area pointerup", "listen scroll-area pointercancel",
    "listen column-toggle click", "listen side-close click", "listen left thread-card click", "listen left side-thread-card click",
    "listen left thread-card animationend", "listen left side-thread-card animationend", "listen left pane-discord animationend",
    "listen left side-pane animationend", "listen side-more-threads click", "listen left side-search-replies click", "listen left side-unpack click",
    "listen left side-main click", "listen left side-order click"], "a listener was added out of the page's order");
  log.length = 0;
  const swipe = (down, up) => { fire("scroll-area", "pointerdown", down); fire("scroll-area", "pointerup", up); };
  const touch = (x, y, target = { tagName: "li" }) => ({ pointerType: "touch", clientX: x, clientY: y, target });
  swipe(touch(10, 10), touch(10 + SWIPE_COMMIT_PX, 20));
  assert.deepStrictEqual(log, ["click claimed", "thread closed"], "a long enough swipe right did not leave the thread");
  assert.equal(gesture, null, "the gesture outlived its pointerup");
  log.length = 0;
  swipe(touch(10, 10), touch(9 + SWIPE_COMMIT_PX, 10));
  swipe(touch(10, 10), touch(10 + SWIPE_COMMIT_PX, 20 + SWIPE_COMMIT_PX));
  swipe({ ...touch(10, 10), pointerType: "mouse" }, touch(500, 10));
  swipe(touch(10, 10, { tagName: "textarea" }), touch(500, 10));
  inThread = false;
  swipe(touch(10, 10), touch(500, 10));
  inThread = true;
  fire("scroll-area", "pointerdown", touch(10, 10));
  fire("scroll-area", "pointercancel");
  fire("scroll-area", "pointerup", touch(500, 10));
  assert.deepStrictEqual(log, [], "a short, steep, mouse, in-a-box, out-of-thread or cancelled swipe left the thread");
  fire("column-toggle", "click");
  fire("side-close", "click");
  fire("left side-thread-card", "click", { n: 1 });
  fire("left side-pane", "animationend", { n: 2 });
  fire("side-more-threads", "click");
  fire("left side-search-replies", "click");
  fire("left side-unpack", "click");
  fire("left side-main", "click");
  fire("left side-order", "click");
  assert.deepStrictEqual(log, ["columns", "side closed", "card left side-thread-card 1", "flight left side-pane 2", "older threads", "replies", "unpack", "main", "order"]);
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

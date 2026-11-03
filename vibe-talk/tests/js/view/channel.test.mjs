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

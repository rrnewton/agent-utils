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
  const box = (top) => ({ style: { height: "" }, getBoundingClientRect: () => ({ top: top(), height: 0 }), getClientRects: () => [{}] });
  const rows = 1000;
  const area = {
    clientHeight: 400, clientTop: 0, scrollTop: 600,
    get scrollHeight() { return rows + (Number.parseFloat(overscrollBox.style.height) || 0) + (Number.parseFloat(overextendBox.style.height) || 0); },
    getBoundingClientRect: () => ({ top: 0 }),
  };
  const overscrollBox = box(() => rows - area.scrollTop);
  const overextendBox = box(() => rows + (Number.parseFloat(overscrollBox.style.height) || 0) - area.scrollTop);
  const card = { hasAttribute: () => false, getBoundingClientRect: () => ({ top: rows - 100 - area.scrollTop, height: 100 }) };
  const timers = [];
  let context = "c1";
  let heldTop = 100;
  const heldRow = { getBoundingClientRect: () => ({ top: heldTop, bottom: heldTop + 50, height: 50 }) };
  const roomDeps = {
    el: () => ({ children: [] }), leftEl: () => ({ children: [card] }),
    layoutColumns: () => true, layoutThreadList: () => false,
    screens: () => ({ screen: () => "main", view: () => "discord" }),
    sideMode: () => "cards", readMode: () => "show",
    sideContext: () => [context, false, ""], inSideColumn: (act) => act(),
    shownChannelList: () => ({ children: [] }),
    toPosition: (area, top) => { area.scrollTop = top; },
    finishPackSlide: () => {}, cardRow: () => heldRow, rowShown: () => true,
    itemAfterBlock: () => null, slideKey: () => "k", slideIntoPlace: () => {},
  };
  const rooms = createFootRooms({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    side: () => area, overscrollBox: () => overscrollBox, overextendBox: () => overextendBox,
    sideUp: () => true, ...roomDeps,
    paddingBottom: () => 0, pageStep: () => 360,
    toFoot: (area) => { area.scrollTop = area.scrollHeight; }, nudge: (area, delta) => { area.scrollTop += delta; }, wheelBy: (area, px) => { area.scrollTop += px; },
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
  // A card packed where it stands is held on its screen line, past the foot where
  // that takes room held for it (`keepingCardStill`, #459).
  assert.equal(rooms.keepingCardStill("T1", () => { heldTop = 80; }), true, "the card was not held");
  assert.equal(area.scrollTop, 980, "the reader was not put back with the card on its line");
  assert.equal(rooms.overextendRoom(area), 380, "the room held for the card is not the room it takes");
  // `#1144 scrollback-typing-jump`: the keyboard's room under a composer at the foot is no part of
  // the rows' foot: resting on the keys is at it, and up from it is measured without the room.
  const mainArea = { clientHeight: 400, clientTop: 0, scrollTop: 880, scrollHeight: 1400, getBoundingClientRect: () => ({ top: 0 }) };
  const mainRooms = createFootRooms({
    clock: { setTimeout: () => 0, clearTimeout: () => {} },
    side: () => area, overscrollBox: () => overscrollBox, overextendBox: () => overextendBox,
    sideUp: () => true, ...roomDeps,
    paddingBottom: () => 0, pageStep: () => 360,
    toFoot: (area) => { area.scrollTop = area.scrollHeight; }, nudge: (area, delta) => { area.scrollTop += delta; }, wheelBy: (area, px) => { area.scrollTop += px; },
    keyboardRoom: (one) => (one === mainArea ? 120 : 0),
  });
  assert.equal(mainRooms.footTop(mainArea), 880, "the rows' foot leaves the keyboard's room out");
  assert.equal(mainRooms.footGap(mainArea), 0, "resting on the composer over the keys is at the rows' foot");
  mainArea.scrollTop = 800;
  assert.equal(mainRooms.footGap(mainArea), 80, "scrolled up is measured from the rows' foot, room or none");
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
  // `#1209 selection-survives-updates`: a text selection in a row is kept across the rebuild and
  // put back after the reader is.
  const kept = { id: "b", start: 2, end: 5 };
  const order = [];
  const keep2 = createScrollKeeping({
    el: (id) => els[id], onMain: () => true, currentView: () => "discord", channelView: () => "main", pinnedOnly: () => false,
    heldFilterRow: () => null, searchWalk: () => null, atBottom: () => false,
    scrollToNewest: () => order.push("newest"), renderJumpNewest: () => order.push("jump"),
    setJumpNewest: () => {},
    selectionIn: () => { order.push("capture"); return kept; },
    restoreSelectionIn: (area2, sel) => order.push(["restore", sel]),
  });
  keep2.preservingScroll(() => order.push("mutate"));
  assert.deepStrictEqual(order, ["capture", "mutate", "jump", ["restore", kept]],
    "the selection was not kept across the change, or not put back after the reader");
  keep.followIfPinned(true, 1, "discord");
  assert.equal(calls.at(-1), "newest");
});

test("`#1229 keyed-row-patching`: a redraw keeps the node of every row whose messages are the very objects it was drawn from", () => {
  const { patchRows } = CHANNEL_VIEW;
  const list = {
    children: [],
    insertBefore(row, ref) {
      const at = this.children.indexOf(row);
      if (at >= 0) this.children.splice(at, 1);
      const to = ref ? this.children.indexOf(ref) : this.children.length;
      this.children.splice(to < 0 ? this.children.length : to, 0, row);
      row.parentNode = this;
      return row;
    },
    removeChild(row) {
      const at = this.children.indexOf(row);
      if (at >= 0) this.children.splice(at, 1);
      row.parentNode = null;
      return row;
    },
  };
  const rowFor = (messages) => ({
    messages,
    parentNode: null,
    getAttribute: (name) => (name === "data-ids" ? messages.map((m) => String(m.id)).join(" ") : null),
  });
  const build = (group) => rowFor(group);
  const a = { id: "1" }, b = { id: "2" }, c = { id: "3" }, d = { id: "4" };
  patchRows(list, [[a], [b], [c]], build);
  const [rowA, rowB, rowC] = list.children;
  // The same objects, regrouped and joined by a new one: only the new group is built.
  patchRows(list, [[a, b], [c], [d]], build);
  assert.equal(list.children.length, 3);
  assert.equal(list.children[1], rowC, "an unchanged row was rebuilt instead of kept");
  assert.notEqual(list.children[0], rowA, "a regrouped row kept a node drawn for another grouping");
  assert.equal(list.children[0].messages[0], a);
  // A new object for the same id is a changed row: rebuilt, and the gone one removed.
  const c2 = { id: "3", edited: true };
  patchRows(list, [[a, b], [c2]], build);
  assert.equal(list.children.length, 2, "a row that left the list stayed in it");
  assert.notEqual(list.children[1], rowC, "a changed row kept its old node");
  assert.equal(list.children[1].messages[0], c2);
  // `kept` rows stand above the patched part, untouched.
  const head = list.children[0];
  patchRows(list, [[c2]], build, [head]);
  assert.deepStrictEqual(list.children, [head, list.children[1]]);
  assert.equal(list.children[0], head, "a kept row above the patch was moved or rebuilt");
  // A row built for one view is built again for another: what a build puts on a row is the
  // view's (a thread's tag in Main, not in the thread's own list).
  const { noteRowBuilt } = CHANNEL_VIEW;
  const forMain = list.children[1];
  noteRowBuilt(forMain, "main"); // as its build in Main noted it (`addThreadDecoration`)
  patchRows(list, [[c2]], build, [head], (drawn, incoming) => drawn === incoming, "thread");
  assert.equal(list.children[0], head, "a kept row above the patch was moved or rebuilt");
  assert.equal(list.children.length, 2);
  const forThread = list.children[1];
  assert.notEqual(forThread, forMain, "a row built for Main was kept for the thread's list");
  noteRowBuilt(forThread, "thread"); // as a build in that view notes it (`addThreadDecoration`)
  patchRows(list, [[c2]], build, [head], (drawn, incoming) => drawn === incoming, "thread");
  assert.equal(list.children[1], forThread, "a row built for the view it is drawn in was rebuilt");
});

test("`#1251 scroll-single-owner` + `#1215 collapse-anchor-below`: a collapse holds its row, the next row, its head, or the foot", () => {
  const { createScrollKeeping } = CHANNEL_VIEW;
  const tops = new Map();
  const mk = (id) => ({
    id, parentNode: null, nextElementSibling: null, getAttribute: () => null,
    getBoundingClientRect: () => ({ top: tops.get(id)[0], bottom: tops.get(id)[1] }),
  });
  const a = mk("a");
  const b = mk("b");
  const c = mk("c");
  a.nextElementSibling = b;
  b.nextElementSibling = c;
  const list = { id: "discord-log", children: [a, b, c] };
  for (const one of [a, b, c]) one.parentNode = list;
  const area = { scrollTop: 300, scrollHeight: 900, clientHeight: 200, getBoundingClientRect: () => ({ top: 0 }) };
  const els = { "discord-log": list, "pinned-log": { id: "pinned-log", children: [] }, "scroll-area": area, "load-older": { hidden: true, getBoundingClientRect: () => ({ top: 0 }) }, transcript: { children: [] } };
  let bottom = false;
  const keep = createScrollKeeping({
    el: (id) => els[id], onMain: () => true, currentView: () => "discord", channelView: () => "main", pinnedOnly: () => false,
    heldFilterRow: () => null, searchWalk: () => null, atBottom: () => bottom,
    scrollToNewest: () => {}, renderJumpNewest: () => {}, setJumpNewest: () => {},
  });
  // Head on screen: the row itself is held, so a shift above it is corrected in full.
  tops.set("a", [-40, 40]);
  tops.set("b", [40, 120]);
  tops.set("c", [120, 200]);
  keep.preservingCollapse(area, b, 0, 200, () => tops.set("b", [10, 50]));
  assert.equal(area.scrollTop, 270, "a collapse with its head on screen did not hold the row");
  // Head above the viewport, foot on screen: the next row is held (`#1215`); holding the
  // collapsed row itself, whose top never moves, would leave the next row thrown up by 60px.
  area.scrollTop = 300;
  tops.set("b", [-80, 40]);
  tops.set("c", [40, 120]);
  keep.preservingCollapse(area, b, 0, 200, () => { tops.set("b", [-80, -20]); tops.set("c", [-20, 60]); });
  assert.equal(area.scrollTop, 240, "a collapse with its head off screen did not hold the next row");
  // Taller than the room (neither its top nor the next row on screen): centered (`#1215`(c)).
  area.scrollTop = 300;
  tops.set("b", [-80, 500]);
  keep.preservingCollapse(area, b, 0, 200, () => tops.set("b", [-80, 60]));
  assert.equal(area.scrollTop, 190, "a row taller than the screen was not centered in the room");
  // Following the foot: the foot is held, whatever the row.
  area.scrollTop = 300;
  bottom = true;
  keep.preservingCollapse(area, b, 0, 200, () => {});
  assert.equal(area.scrollTop, 900, "a reader following the foot was not kept on it");
});

test("`#1251 scroll-single-owner`: a redraw that replaces the anchor row is held by the row re-found by id", () => {
  const { createScrollKeeping } = CHANNEL_VIEW;
  let shift = 0;
  const row = (id, top) => ({
    id, parentNode: null, getAttribute: (name) => (name === "data-id" ? id : null),
    getBoundingClientRect: () => ({ top: top + shift, bottom: top + shift + 40 }),
  });
  const oldRows = [row("a", -80), row("b", -40), row("c", 0), row("d", 40)];
  const list = { id: "discord-log", children: oldRows };
  for (const one of oldRows) one.parentNode = list;
  const area = { scrollTop: 300, getBoundingClientRect: () => ({ top: 0 }) };
  const els = { "discord-log": list, "pinned-log": { id: "pinned-log", children: [] }, "scroll-area": area, "load-older": { hidden: true, getBoundingClientRect: () => ({ top: 0 }) }, transcript: { children: [] } };
  const keep = createScrollKeeping({
    el: (id) => els[id], onMain: () => true, currentView: () => "discord", channelView: () => "main", pinnedOnly: () => false,
    heldFilterRow: () => null, searchWalk: () => null, atBottom: () => false,
    scrollToNewest: () => {}, renderJumpNewest: () => {}, setJumpNewest: () => {},
  });
  // The redraw replaces every row (the held node detached) and the content above the anchor
  // grows by 25px in the replacement: the reader is corrected by the re-found row, not left
  // where the shorter interim list clamped the scroll.
  keep.preservingScroll(() => {
    shift = 25;
    const fresh = [row("a", -80), row("b", -40), row("c", 0), row("d", 40)];
    for (const one of oldRows) one.parentNode = null;
    for (const one of fresh) one.parentNode = list;
    list.children = fresh;
  });
  assert.equal(area.scrollTop, 325, "a replaced anchor row was not re-found by id and held");
});

test("`#1057 newest-count-chip`: the rows below the reading position, from the last row wholly on screen", () => {
  const { rowsBelowReadingPosition } = CHANNEL_VIEW;
  const row = (top, bottom) => ({ getBoundingClientRect: () => ({ top, bottom }) });
  // Head at 0, foot at 100: the first row is scrolled off, the next two stand wholly on screen,
  // the fourth is cut at the foot and the fifth is below it.
  const rows = [row(-40, 0), row(0, 40), row(40, 80), row(80, 120), row(120, 160)];
  assert.equal(rowsBelowReadingPosition(rows, 0, 100), 2, "the cut row and the one under it stand below");
  assert.equal(rowsBelowReadingPosition(rows.slice(0, 3), 0, 100), 0, "the last row on screen wholly is the foot");
  assert.equal(rowsBelowReadingPosition([], 0, 100), 0);
  // Within a pixel of either edge still stands wholly on screen, as the keys read it.
  assert.equal(rowsBelowReadingPosition([row(-1, 101), row(101, 141)], 0, 100), 1);
  // Nothing wholly on screen — one row taller than the room: the reader is on it, and what
  // follows it is below.
  assert.equal(rowsBelowReadingPosition([row(-50, 150), row(150, 190), row(190, 230)], 0, 100), 2);
  // Wholly below the room keeps every row below.
  assert.equal(rowsBelowReadingPosition([row(200, 240), row(240, 280)], 0, 100), 2);
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

test("`#624 voice-page-modularity`: the ColumnWork port's reader sees the working column as it switches, and switching stays the port's", () => {
  const { createColumnWork } = CHANNEL_VIEW;
  const columns = { main: { name: "main", left: false }, side: { name: "side", left: false } };
  const port = createColumnWork(columns);
  const reader = port;
  assert.deepStrictEqual([reader.active().name, reader.side()], ["main", false]);
  port.inColumn(columns.side, () => {
    assert.deepStrictEqual([reader.active().name, reader.side()], ["side", true], "the reader did not follow the switch");
  });
  assert.deepStrictEqual([reader.active().name, reader.side()], ["main", false]);
  assert.deepStrictEqual(Object.keys(reader).sort(), ["active", "inColumn", "side", "useColumn"],
    "the port grew a second way to the working column");
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
  const card = { hidden: false, getAttribute: () => "dock", offsetHeight: 30 };
  const under = createUnderLists({
    el: () => ({ offsetHeight: 50 }),
    leftEl: (id) => (id === "post-confirm" ? card : { clientHeight: 500 }),
    screens: () => ({ screen: () => "main" }),
  });
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
  under.cardMovedUnderLists(-246);
  assert.deepStrictEqual(under.main(), { list: 600, dock: 50, gap: 12 }, "the card's move back is no growth either");
  under.cardCameUnderLists(7);
  assert.deepStrictEqual(under.main(), { list: 600, dock: 50, gap: 7 }, "the card's coming was not told by its wrapper");
  assert.equal(under.heightUnderLists(), 80, "the dock and the docked card are not both measured");
  under.cardsPlacedUnderLists();
  assert.deepStrictEqual(under.side(), { list: 500, dock: 80 }, "the cards placed were not noted as they stand");
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
    intoView: (element, block, inline) => { if (typeof element.scrollIntoView === "function") element.scrollIntoView(inline === undefined ? { block } : { block, inline }); },
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
  const main = box("scroll-area", { children: [box("pane-discord")] });
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
    writing: false, written: false, writingIn: false, sideUp: true, showing: true, mode: "thread", opening: false, columns: true, threadList: false, walk: false, held: null };
  const columns = { main: { name: "main", mode: "main" }, side: { name: "side", get mode() { return state.mode; } } };
  const was = globalThis.setTimeout;
  globalThis.setTimeout = (act) => { timers.push(act); return timers.length; };
  try {
    const underLists = createUnderLists({
      el: () => ({ offsetHeight: 0 }),
      leftEl: () => ({ hidden: true, getAttribute: () => null, clientHeight: 0 }),
      screens: () => ({ screen: () => "main" }),
    });
    const keepers = mountListKeepers({
      canWatchSizes: () => state.sizes,
      sizeWatcher: (report) => { const watcher = { report, watched: [], stopped: false, observe(target) { this.watched.push(target.id); }, disconnect() { this.stopped = true; } }; watchers.push(watcher); return watcher; },
      el: (id) => els[id],
      leftEl: (id) => lefts[id],
      screens: { screen: () => state.screen, view: () => state.view },
      underLists,
      cardsDrawn: { opening: () => state.opening },
      columns,
      layoutShown: { columns: () => state.columns, threadList: () => state.threadList },
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
      writingIn: () => state.writingIn,
      atBottom: (area, slack) => { log.push(`at bottom? ${area.id} ${slack}`); return state.bottom.has(area.id); },
      followFoot: (area) => log.push(`follow ${area.id}`),
      toFoot: (area) => { area.scrollTop = area.scrollHeight; }, toPosition: (area, top) => { area.scrollTop = top; },
      sideColumnUp: () => state.sideUp,
      sideShowing: () => state.showing,
      inColumn: (column, act) => { log.push(`in ${column.name}`); return act(); },
      fitOverextend: (area) => log.push(`fit room ${area.id}`),
      holdOverscrollReader: (area) => log.push(`hold ${area.id}`),
      channelContextKey: () => "c1",
      pinsView: () => ({ only: false }),
      measureReplyArrows: () => log.push("arrows"),
    });
    assert.deepStrictEqual(watchers.map((one) => one.watched), [["scroll-area"], ["side-scroll"], ["side-log"], ["pane-discord"], ["discord-log"], ["side-log"]],
      "a list went unwatched, or the right column's own rooms were watched");
    const [mainDock, sideDock, footKeeper, mainFootKeeper, arrows, sideArrows] = watchers;
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
    // `#1144 scrollback-typing-jump`. Main's foot keeper: follow is one state, noted from the reader's
    // own scrolls; while writing at the foot, late growth is followed; a caret reveal never turns it on.
    const fireMain = (type) => { for (const one of main.listeners.filter((each) => each.type === type)) one.listener(); };
    main.clientWidth = 400;
    main.clientHeight = 500;
    main.scrollTop = 1500;
    state.bottom = new Set(["scroll-area"]);
    fireMain("wheel");
    fireMain("scroll");
    state.bottom = new Set();
    state.writingIn = true;
    assert.ok(run(() => mainFootKeeper.report()).includes("follow scroll-area"),
      "a writer at Main's foot was left above it as its rows grew");
    state.writingIn = false;
    assert.ok(!run(() => mainFootKeeper.report()).includes("follow scroll-area"),
      "a reader at the foot not writing was followed (that reader is offered the jump)");
    fireMain("touchstart");
    fireMain("scroll");
    state.writingIn = true;
    assert.ok(!run(() => mainFootKeeper.report()).includes("follow scroll-area"),
      "follow stayed on after the reader's own scroll up the list");
    state.bottom = new Set(["scroll-area"]);
    fireMain("input");
    fireMain("scroll");
    state.bottom = new Set();
    assert.ok(!run(() => mainFootKeeper.report()).includes("follow scroll-area"),
      "a caret reveal in the instant after typing turned follow on");
    state.writingIn = false;
    // `#1251 scroll-single-owner`: the first report after a view switch may only
    // re-note (`invalidateDock`): neither keeper makes a decision from a reading
    // taken while the other list stood in the same scroller, dock growth included.
    keepers.invalidateDock();
    assert.equal(keepers.followingMain(), false, "a pre-switch foot note still read as following");
    // A scroll of the scroller, and a report while the other list stands (these rows
    // hidden), spend nothing: the re-take is still owed when Main's list comes back.
    state.bottom = new Set(["scroll-area"]);
    fireMain("scroll");
    assert.equal(keepers.followingMain(), false, "a scroll under the other list seeded Main's follow");
    log2.hidden = true;
    assert.deepStrictEqual(run(() => mainFootKeeper.report()), []);
    assert.equal(keepers.followingMain(), false, "a report under the other list spent the re-take");
    log2.hidden = false;
    state.bottom = new Set(["scroll-area"]);
    main.scrollTop = 100;
    assert.deepStrictEqual(run(() => mainFootKeeper.report()), ["at bottom? scroll-area 12"]);
    assert.equal(main.scrollTop, 100, "the foot report after a switch followed a stale note");
    // The next report judges by the same rules again.
    state.bottom = new Set();
    state.writingIn = true;
    assert.ok(run(() => mainFootKeeper.report()).includes("follow scroll-area"),
      "the foot keeper stopped keeping after the invalidation");
    state.writingIn = false;
    // A change of layout is a change of what the list shows too: a foot note from the
    // other layout does not read as following until the keeper re-seeds it in this one.
    state.threadList = true;
    assert.equal(keepers.followingMain(), false, "a foot note from the other layout still read as following");
    state.bottom = new Set(["scroll-area"]);
    assert.deepStrictEqual(run(() => mainFootKeeper.report()), ["at bottom? scroll-area 12"]);
    assert.equal(keepers.followingMain(), true, "the re-seeded foot note did not read as following");
    state.threadList = false;
    state.bottom = new Set();
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
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); },
    focus(options) { log.push(`focus ${id} ${options && options.preventScroll}`); } });
  const press = (id, type = "click") => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({}));
  let hidden = false;
  let inside = true;
  let autoplaying = false;
  const controls = mountListControls({
    byId: (id) => target(id),
    leftById: (id) => target(`left ${id}`),
    loadOlder: () => { log.push("older"); return "older asked"; },
    searchInAll: () => { log.push("search in All"); return "in All asked"; },
    serverMore: () => { log.push("server more"); return "more asked"; },
    searchAllChannels: (on) => log.push(`all channels ${on}`),
    searchFurther: () => { log.push("further"); return "further asked"; },
    loadOlderTurns: () => { log.push("older turns"); return "turns asked"; },
    setAllFolded: (folded) => log.push(`folded ${folded}`),
    cycleReadMode: (...args) => log.push(`read mode ${args.length}`),
    clearBacklog: () => { log.push("clear backlog"); return "cleared"; },
    undoDismissal: () => { log.push("undo"); return "undone"; },
    toggleSummaries: () => log.push("summaries"),
    readerHidden: () => hidden,
    setReaderHidden: (wanted) => { hidden = wanted; log.push(`reader hidden ${wanted}`); },
    closeSpeedPopover: () => log.push("pace closed"),
    autoplayOn: () => autoplaying,
    setAutoplay: (wanted) => { autoplaying = wanted; log.push(`autoplay ${wanted}`); },
    focusInActionMenu: () => inside,
    closeActionMenu: () => log.push("menu closed"),
    jumpToNewest: () => log.push("newest"),
  });
  assert.deepStrictEqual(log, ["listen load-older click", "listen left search-in-all click", "listen server-more click",
    "listen search-all-channels change", "listen search-further click", "listen load-older-turns click", "listen collapse-all click",
    "listen expand-all click", "listen todo-filter click", "listen clear-backlog click", "listen undo-dismiss click", "listen summarise click",
    "listen reader-toggle click", "listen autoplay-toggle click", "listen expand-all click", "listen collapse-all click", "listen summarise click",
    "listen reader-toggle click", "listen autoplay-toggle click", "listen discuss-menu-open click", "listen todo-filter click", "listen jump-newest click"], "a listener was added out of the page's order");
  log.length = 0;
  assert.deepStrictEqual(press("load-older"), ["older asked"], "the control did not answer what it started, which the page suite waits for");
  // Each control answers what its act started, which the page suite waits for.
  assert.deepStrictEqual([press("left search-in-all"), press("server-more")], [["in All asked"], ["more asked"]]);
  nodes["search-all-channels"].checked = true;
  press("search-all-channels", "change");
  assert.deepStrictEqual([press("search-further"), press("load-older-turns")], [["further asked"], ["turns asked"]]);
  press("todo-filter");
  assert.deepStrictEqual([press("clear-backlog"), press("undo-dismiss")], [["cleared"], ["undone"]]);
  press("jump-newest");
  assert.deepStrictEqual(log, ["older", "search in All", "server more", "all channels true", "further", "older turns", "read mode 0",
    "menu closed", "focus action-menu-open true", "clear backlog", "undo", "newest"]);
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
  log.length = 0;
  // `#886 voice-page-autoplay`. Autoplay on, then off, each closing the menu after it.
  press("autoplay-toggle");
  press("autoplay-toggle");
  assert.deepStrictEqual(log, ["autoplay true", "menu closed", "focus action-menu-open true", "autoplay false", "menu closed",
    "focus action-menu-open true"], "the menu's Autoplay did not flip it before closing the menu");
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the thread's back gesture commits only a long enough swipe right in a thread, the right column's controls reach their acts in the page's order, and come down again", () => {
  const { mountSideControls, SWIPE_COMMIT_PX } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
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

test("`#890 voice-page-linked-highlight`: the linked message is marked on every drawing of its row until Esc, its × or another link puts the mark out", () => {
  const { createLinkedMark, LINKED_MARK_WORDS } = CHANNEL_VIEW;
  const element = (tag, className = "") => {
    const attrs = new Map();
    const listeners = [];
    const el = {
      tagName: tag.toUpperCase(), className, children: [], textContent: "", listeners,
      setAttribute: (name, value) => attrs.set(name, String(value)), getAttribute: (name) => attrs.get(name) ?? null,
      hasAttribute: (name) => attrs.has(name), removeAttribute: (name) => attrs.delete(name),
      append: (...kids) => el.children.push(...kids),
      insertBefore: (kid, ref) => { const at = ref ? el.children.indexOf(ref) : -1; el.children.splice(at < 0 ? el.children.length : at, 0, kid); },
      removeChild: (kid) => { el.children.splice(el.children.indexOf(kid), 1); },
      addEventListener: (type, listener) => listeners.push({ type, listener }),
    };
    return el;
  };
  const was = globalThis.document;
  globalThis.document = { createElement: (tag) => element(tag) };
  try {
    const state = { channel: "c1", shown: true, rows: [], dismissed: 0 };
    // Each in a list, as a row drawn and shown is.
    const list = { id: "discord-log" };
    const row = (...ids) => { const one = element("li", "discord-message"); one.ids = ids; one.parentNode = list; one.append(element("div", "meta")); return one; };
    const mark = createLinkedMark({
      rowsDrawn: () => state.rows,
      idsOf: (one) => one.ids,
      channel: () => state.channel,
      rowShown: () => state.shown,
      dismissed: (one) => { state.dismissed += 1; state.last = one; },
    });
    const marker = (one) => one.children.find((kid) => kid.className === "linked-marker");
    const linked = (one) => one.getAttribute("data-linked") === "true" && Boolean(marker(one));
    const [a, b, c] = [row("1"), row("2", "3"), row("4")];
    state.rows = [a, b, c];
    assert.equal(mark.linked(), null);
    mark.set("c1", "3");
    assert.deepStrictEqual(mark.linked(), { channel: "c1", id: "3" });
    assert.deepStrictEqual([linked(a), linked(b), linked(c)], [false, true, false], "the row holding the linked message is not the one marked");
    assert.equal(b.children[0], marker(b), "the note is not at the row's head");
    // After a reply's arrow and a thread's strip, which lead the row.
    const reply = row("3");
    reply.insertBefore(element("button", "thread-strip"), reply.children[0]);
    reply.insertBefore(element("button", "reply-jump"), reply.children[0]);
    mark.drawOn(reply);
    assert.deepStrictEqual(reply.children.map((kid) => kid.className), ["reply-jump", "thread-strip", "linked-marker", "meta"],
      "the note went ahead of the arrow or the strip");
    const words = marker(b).children;
    assert.equal(words[0].textContent, LINKED_MARK_WORDS.note);
    assert.equal(words[1].getAttribute("aria-label"), LINKED_MARK_WORDS.clear, "the × says nothing to a screen reader");
    // Drawn again: the row the list draws now holding it is marked, once; one no longer holding it is not.
    const again = row("2", "3");
    mark.drawOn(again);
    mark.drawOn(again);
    assert.equal(again.children.filter((kid) => kid.className === "linked-marker").length, 1, "a row drawn twice took two notes");
    b.ids = ["2"];
    mark.drawOn(b);
    assert.equal(linked(b), false, "a reused row that no longer holds the message kept the mark");
    b.ids = ["2", "3"];
    mark.drawOn(b, ["2", "3"]);
    assert.equal(linked(b), true);
    // Another channel's row with the same id is not the linked message.
    state.channel = "c2";
    const elsewhere = row("3");
    mark.drawOn(elsewhere);
    assert.equal(linked(elsewhere), false, "a message of the same id in another channel was marked");
    state.channel = "c1";
    // Another link moves it.
    mark.set("c1", "4");
    assert.deepStrictEqual([linked(a), linked(b), linked(again), linked(c)], [false, false, false, true], "the old mark stayed when a new link moved it");
    assert.equal(state.dismissed, 0, "another link moving the mark was taken for the reader putting it out");
    // Esc: only where the mark is on screen.
    state.shown = false;
    assert.equal(mark.clearOnEscape(), false, "Esc put out a mark out of sight");
    assert.equal(linked(c), true);
    state.shown = true;
    assert.equal(mark.clearOnEscape(), true);
    assert.equal(linked(c), false);
    assert.equal(mark.linked(), null);
    assert.equal(mark.clearOnEscape(), false, "a second Esc found a mark");
    assert.equal(state.dismissed, 1, "Esc putting the mark out was not said once");
    assert.deepStrictEqual(state.last, { channel: "c1", id: "4" }, "Esc said another message was put out");
    // The ×.
    mark.set("c1", "1");
    marker(a).children[1].listeners.find((one) => one.type === "click").listener({});
    assert.deepStrictEqual([linked(a), mark.linked()], [false, null], "the × did not put the mark out");
    assert.equal(state.dismissed, 2, "the × putting the mark out was not said");
    assert.equal(mark.clear(), false, "a mark already out was put out again");
    mark.set("c1", "1");
    mark.clear();
    assert.equal(state.dismissed, 2, "putting the mark out for the page (signing out) was taken for the reader's");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the view chips, the thread's Back and the picker reach what each names in the page's order, a touch on the picker answering nothing, and come down again", () => {
  const { mountChannelViewControls } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({ type }));
  const controls = mountChannelViewControls({
    lookup: (id) => target(id),
    changeView: (...args) => { log.push(`view ${args.join(" ")}`); return "changing"; },
    closeThread: (...args) => { log.push(`back ${args.length}`); return "closing"; },
    selectThread: (...args) => { log.push(`select ${args.length}`); return "selecting"; },
    refreshThreadDirectory: (...args) => { log.push(`refresh ${args.length}`); return "refreshing"; },
  });
  assert.deepStrictEqual(log, ["listen channel-view-main click", "listen channel-view-threads click", "listen channel-view-flat click",
    "listen thread-back click", "listen thread-select change", "listen thread-select focus", "listen thread-select pointerdown"],
    "a control was heard out of the page's order");
  log.length = 0;
  assert.deepStrictEqual(["main", "threads", "flat"].flatMap((view) => fire(`channel-view-${view}`, "click")), ["changing", "changing", "changing"]);
  assert.deepStrictEqual(fire("thread-back", "click"), ["closing"]);
  assert.deepStrictEqual(fire("thread-select", "change"), ["selecting"]);
  // The read goes on behind a touch: nothing waits on it.
  assert.deepStrictEqual([...fire("thread-select", "focus"), ...fire("thread-select", "pointerdown")], [undefined, undefined]);
  assert.deepStrictEqual(log, ["view main", "view threads", "view flat", "back 0", "select 0", "refresh 0", "refresh 0"]);
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the right column list's controls reach what each names in the page's order, answering what each answers, and come down again", () => {
  const { mountSideListControls } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({ type }));
  const act = (name) => (...args) => { log.push(`${name} ${args.length}`); return name; };
  const controls = mountSideListControls({
    lookup: (id) => target(id),
    loadOlder: act("older"), clearBacklog: act("clear"), jumpNewest: act("newest"), listScrolled: act("scrolled"),
  });
  assert.deepStrictEqual(log, ["listen load-older click", "listen clear-backlog click", "listen jump-newest click", "listen scroll-area scroll"]);
  log.length = 0;
  assert.deepStrictEqual([fire("load-older", "click"), fire("clear-backlog", "click"), fire("jump-newest", "click"), fire("scroll-area", "scroll")].flat(),
    ["older", "clear", "newest", "scrolled"]);
  assert.deepStrictEqual(log, ["older 0", "clear 0", "newest 0", "scrolled 0"], "an act was handed the event");
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the ways back reach the bookmark, the reading place (the left column's chip) and the reply, in the page's order, and come down again", () => {
  const { mountReadingJumps } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id) => nodes[id].listeners.map((one) => one.listener({ type: "click" }));
  const act = (name) => (...args) => { log.push(`${name} ${args.length}`); return name; };
  const jumps = mountReadingJumps({ lookup: (id) => target(id), leftLookup: (id) => target(`left:${id}`),
    jumpToBookmark: act("bookmark"), jumpToUnread: act("unread"), jumpToReading: act("reading"), backToReply: act("reply") });
  assert.deepStrictEqual(log, ["listen jump-bookmark click", "listen jump-unread click", "listen left:jump-reading click", "listen back-to-reply click"]);
  log.length = 0;
  assert.deepStrictEqual([...fire("jump-bookmark"), ...fire("jump-unread"), ...fire("left:jump-reading"), ...fire("back-to-reply")], ["bookmark", "unread", "reading", "reply"]);
  assert.deepStrictEqual(log, ["bookmark 0", "unread 0", "reading 0", "reply 0"]);
  jumps.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: what the page kept of its sends is read back in the page's order, the Done writes sent and handed over with the connection and the page, and the Undo and Forget buttons heard, then all come down", () => {
  const { mountKeptSends } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({ type }));
  const act = (name) => (...args) => { log.push(`${name} ${args.length}`); return name; };
  const was = globalThis.window;
  globalThis.window = target("window");
  try {
    const kept = mountKeptSends({
      lookup: (id) => target(id),
      loadReplyDrafts: act("reply drafts"), loadOwnPosts: act("own posts"), loadOutgoingMessages: act("outbox"),
      sendLeftOverInboxWrites: act("send done writes"), leaveInbox: act("hand over"), loadDismissedSends: act("dismissed"),
      renderDismissedSends: act("draw dismissed"), undoSendDismiss: act("undo send"), undoBookmarkRemoval: act("undo bookmark"),
      forgetDismissedSends: act("forget"),
    });
    assert.deepStrictEqual(log, ["reply drafts 0", "own posts 0", "outbox 0", "send done writes 0", "listen window online", "listen window pagehide",
      "dismissed 0", "draw dismissed 0", "listen undo-send-dismiss click", "listen undo-bookmark click", "listen forget-dismissed-sends click"],
      "read back or heard out of the page's order");
    log.length = 0;
    assert.deepStrictEqual([fire("window", "online"), fire("window", "pagehide"), fire("undo-send-dismiss", "click"),
      fire("undo-bookmark", "click"), fire("forget-dismissed-sends", "click")].flat(),
      ["send done writes", "hand over", undefined, "undo bookmark", "forget"], "a listener answered what its act did not");
    assert.deepStrictEqual(log, ["send done writes 0", "hand over 0", "undo send 0", "undo bookmark 0", "forget 0"]);
    kept.unmount();
    assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
  } finally {
    globalThis.window = was;
  }
});

test("`#624 voice-page-modularity`: the reply box's listeners are mounted in the page's order, each answering its own act, the size watcher fitted and stopped, then all come down", () => {
  const { mountReplyBox } = CHANNEL_VIEW;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener, options) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}${options && options.passive ? " passive" : ""}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = { type }) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  const act = (name) => (...args) => { log.push(`${name} ${args.length}`); return name; };
  const watchers = [];
  const was = globalThis.document;
  globalThis.document = target("document");
  try {
    const box = mountReplyBox({
      lookup: (id) => target(id),
      leftLookup: (id) => target(`left:${id}`),
      canWatchSizes: () => true,
      sizeWatcher: (report) => { const watcher = { report, seen: [], disconnected: false,
        observe(node) { this.seen.push(node.id); log.push(`observe ${node.id}`); },
        disconnect() { this.disconnected = true; } };
        watchers.push(watcher); return watcher; },
      cancelReply: act("cancel"), answerReplyDiscard: act("discard"), draftChipAsk: act("chip"),
      sendReply: act("send"), onBoxKey: act("key"), watchBoxInsertions: (node) => log.push(`watch ${node.id}`),
      rememberDraft: act("remember"), renderReplyDestination: act("destination"),
      noteFocusLeaving: act("focus leaving"), noteFocusIn: act("focus in"), fitInlineRoom: act("fit"), pressViewSwitch: act("view"),
    });
    assert.deepStrictEqual(log, ["listen reply-cancel click", "listen reply-discard-yes click", "listen reply-discard-no click",
      "listen left:scroll-area scroll passive", "listen left:side-scroll scroll passive", "listen reply-send click", "listen reply-text keydown",
      "watch reply-text", "listen reply-branch change", "listen reply-to-main change", "listen reply-text input", "listen reply-compose focusout",
      "listen document focusin", "observe left:column-reply", "observe left:side-reply", "listen view-switch click"], "mounted out of the page's order");
    log.length = 0;
    fire("reply-cancel", "click"); fire("reply-discard-yes", "click"); fire("reply-discard-no", "click");
    fire("left:scroll-area", "scroll"); fire("left:side-scroll", "scroll"); fire("reply-send", "click");
    assert.deepStrictEqual(fire("reply-text", "keydown", { key: "Enter" }), ["key"], "the box's keys answered what its act did not");
    fire("reply-branch", "change"); fire("reply-to-main", "change"); fire("reply-text", "input"); fire("reply-compose", "focusout"); fire("document", "focusin"); fire("view-switch", "click");
    watchers[0].report();
    assert.deepStrictEqual(log, ["cancel 0", "discard 1", "discard 1", "chip 0", "chip 0", "send 0", "key 1", "remember 0",
      "destination 0", "destination 0", "remember 0", "focus leaving 1", "focus in 0", "view 0", "fit 0"]);
    box.unmount();
    assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
    assert.ok(watchers[0].disconnected, "the size watcher outlived unmount");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the reply box's discard answers carry their own values, and no size watcher is made where sizes cannot be watched", () => {
  const { mountReplyBox } = CHANNEL_VIEW;
  const seen = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id) => nodes[id].listeners.map((one) => one.listener({ type: "click" }));
  const idle = () => {};
  const was = globalThis.document;
  globalThis.document = target("document");
  try {
    const box = mountReplyBox({
      lookup: (id) => target(id), leftLookup: (id) => target(`left:${id}`),
      canWatchSizes: () => false,
      sizeWatcher: () => { throw new Error("a watcher was made where sizes cannot be watched"); },
      cancelReply: idle, answerReplyDiscard: (discard) => seen.push(discard), draftChipAsk: idle,
      sendReply: idle, onBoxKey: idle, watchBoxInsertions: idle, rememberDraft: idle,
      renderReplyDestination: idle, noteFocusLeaving: idle, noteFocusIn: idle, fitInlineRoom: idle, pressViewSwitch: idle,
    });
    fire("reply-discard-yes"); fire("reply-discard-no");
    assert.deepStrictEqual(seen, [true, false]);
    box.unmount();
    assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
  } finally {
    globalThis.document = was;
  }
});

test("quoteSelectionNow opens the quick reply for held words, and does nothing with none", () => {
  globalThis.document = { documentElement: { clientWidth: 800 } };
  globalThis.window = { innerHeight: 800, visualViewport: undefined };
  const { createQuoting } = CHANNEL_VIEW;
  const el = (extra = {}) => ({
    style: {}, hidden: true, textContent: "", value: "",
    getBoundingClientRect: () => ({ top: 0, bottom: 40, left: 0, right: 300, width: 300, height: 40 }),
    focus() {}, ...extra,
  });
  const byIdMap = new Map();
  for (const id of ["quick-reply", "quick-reply-quote", "quick-reply-text", "quick-reply-state",
    "quick-reply-to-main", "quick-reply-to-main-row",
    "selection-chip", "read-steer-quote", "scroll-area", "side-scroll", "discord-log"]) {
    byIdMap.set(id, el(id === "quick-reply-to-main" ? { checked: false } : {}));
  }
  const quoting = createQuoting({
    byId: (id) => byIdMap.get(id),
    inputKind: () => "touch",
    drawReplyLabel() {},
    selectedChannelId: () => "c1",
    replyRoute: () => ({ thread: null, branch: null, into: null, unknown: false, removed: false }),
    readOnly: () => false,
    replyMarksRead: () => null,
    queueOutgoingMessage: async () => {},
    toStream() {},
    drafts: { get: () => undefined, set() {}, delete() {}, save: () => true },
    noteDraft() {},
    closed() {},
  });
  assert.equal(quoting.quoteSelectionNow(), false, "q with nothing held did something");
  const row = el({ messages: [{ id: "m1" }], replyButton: el({ isConnected: true }) });
  quoting.holdQuote({ row, button: row.replyButton, text: "held words", timer: null });
  assert.equal(quoting.quoteSelectionNow(), true, "q with held words did not open the quick reply");
  assert.equal(quoting.quickReplyOpen(), true);
  assert.equal(byIdMap.get("quick-reply-quote").textContent, "held words");
});

test("`#624 voice-page-modularity`: the two columns are made fresh, each with its own records, the right one a list not from cards", () => {
  const { createColumns, COLUMN_IDS } = CHANNEL_VIEW;
  let made = 0;
  const fresh = () => ({ n: ++made });
  const columns = createColumns({
    older: fresh, arrows: fresh, load: fresh, entry: fresh, pins: fresh, tools: fresh,
    filter: fresh, list: fresh, rows: fresh, selection: (view = "thread") => ({ view }), composer: fresh,
    done: fresh, place: fresh, gather: fresh,
  });
  assert.equal(columns.main.name, "main");
  assert.equal(columns.side.name, "side");
  assert.deepStrictEqual([columns.side.mode, columns.side.fromCards, columns.side.left], ["list", false, false]);
  assert.equal(columns.main.selection.view, "main");
  assert.equal(columns.side.selection.view, "thread");
  assert.notEqual(columns.main.older, columns.side.older, "the columns shared a record");
  assert.equal(made, 26, "a field group was made twice or not at all");
  assert.equal(COLUMN_IDS["scroll-area"], "side-scroll");
  assert.equal(COLUMN_IDS["column-reply"], "side-reply");
});

test("`#624 voice-page-modularity`: the reply session routes, words the destination, and sends a ticked Post to main top-level with its answer kept", async () => {
  const { createReplySession } = CHANNEL_VIEW;
  const log = [];
  const queued = [];
  const elements = {};
  const element = (id) => (elements[id] ||= { id, checked: false, hidden: false, value: "", textContent: "", children: [],
    classList: { contains: () => false }, setAttribute() {}, getAttribute: () => null, focus() {}, append() {},
    getBoundingClientRect: () => ({ top: 0, bottom: 10 }) });
  const message = { id: "m1", author: "Ann", thread: { is_root: true } };
  const defaults = {
    el: element, leftEl: element,
    mainColumn: { name: "main", composer: { key: "" } },
    columns: { main: { name: "main", composer: { key: "" } }, side: { name: "side", composer: { key: "" } } },
    columnWork: { active: () => ({ name: "main", composer: { key: "" } }), side: () => false },
    inColumn: (_c, act) => act(), eachColumn: (act) => act({ name: "main" }), inThisColumn: (fn) => fn,
    screens: { screen: () => "main", view: () => "discord" },
    selection: () => ({ view: "main", threadId: null }),
    selectedChannelId: () => "C1",
    channelStore: { current: () => ({ scope: "", threads: new Map(), unplaced: new Map(), messages: [] }) },
    providerFacts: { threading: () => true },
    threadOf: () => "t1", heldThreadRecord: () => ({ id: "t1", is_root: true }),
    unansweredThread: () => false, channelRepliesInThread: () => false, removedUpstream: () => false,
    threadReplies: () => ({ count: 2 }), openThreadRootId: () => null,
    knownChannel: () => ({ writable: true }), setStatus: (t) => log.push(`status:${t}`),
    nudge: (scroller, delta) => { scroller.scrollTop += delta; }, intoView: (element, block, inline) => { if (typeof element.scrollIntoView === "function") element.scrollIntoView(inline === undefined ? { block } : { block, inline }); },
    setTextIfChanged() {}, setHiddenIfChanged() {},
    replyDrafts: { get: () => "", set() {}, delete() {}, has: () => false, ids: () => [], startsThread: () => false, setStartsThread() {}, save: () => true, load() {} },
    draftLedger: { has: () => false, touch() {}, drop() {}, save() {} },
    draftQueries: { draftsHere: () => [], draftHeld: () => false, draftLive: () => null },
    draftRound: { next: () => null }, replySight: { keep() {}, crossed: () => false },
    quoteOwner: () => ({ quickDraftMessage: () => "", quickReplyOpenKey: () => null, quickReplyOpen: () => false, openQuickDraft() {}, openSentenceReply() {}, closeQuickReply() {} }),
    dictation: { stop() {}, quietKeyboard() {}, start() {}, showKeyboard() {} },
    queueOutgoingMessage: (entry) => { queued.push(entry); return Promise.resolve(); },
    replyMarksRead: () => null, quoteBlock: (q) => q, fitGrowingBox() {}, scheduleBackSync: () => log.push("back"),
    markCardBesideReply() {}, voiceTranscript: () => ({ markCardBesideReply() {} }),
    gatherState: () => ({ gathered: null }), gatheredParent: () => null, putRepliesBack() {},
    rowsChosen: () => false, renderChannelRows() {}, renderScrollTools() {}, renderOutgoingEverywhere() {},
    renderOutgoingMessages() {}, outbox: { storageOkay: () => true, values: () => [] },
    openThread() {}, closeThread() {}, jumpToMessageLink: () => Promise.resolve(),
    focusAtEnd() {}, focusList() {}, followFoot() {}, restoreScroll() {}, captureScroll: () => null,
    atBottom: () => true, shownNode: () => true, sideShowing: () => false, sideColumnUp: () => false,
    layoutShown: { columns: () => false }, pinsView: () => ({ only: false }),
    channelContexts: { values: () => [] }, rowMessages: () => [message], idsOf: () => ["m1"],
    rowShown: () => true, paneRowFor: () => null, nodeWithin: () => false, paneColumn: () => null,
    gatherReport: () => ({ gathered: 0 }), channelContextKey: () => "k", repliesTo: () => new Set(),
    focusLost: () => false, isComposer: () => false,
    channelPane: {}, sidePane: {}, rowIn: () => null,
  };
  const session = createReplySession(defaults);
  const route = session.replyRoute(message, true);
  assert.equal(route.into, "t1", "an answered thread was not routed into");
  session.replyState.routing = route;
  session.replyState.target = message;
  session.replyState.channelId = "C1";
  session.renderReplyDestination();
  assert.equal(elements["reply-destination"].textContent,
    "Posts in this message's thread, as you chose. It has a reply already, so yours will not be the first.");
  element("reply-to-main-row").hidden = false;
  element("reply-to-main").checked = true;
  session.renderReplyDestination();
  assert.equal(elements["reply-destination"].textContent,
    "Posts in the main channel as a new thread, as a reply to this message.");
  element("reply-text").value = "promoted";
  await session.sendReply();
  assert.deepStrictEqual(queued, [{ channel: "C1", threadId: null, postToMain: true, replyTo: "m1",
    text: "promoted", marksRead: null }], "the ticked send did not go top-level with its answer kept");
});

test("`#624 voice-page-modularity`: a reply that did not go is reported in the column it was sent from, and the unseen one is let go when it goes", () => {
  const { createColumnReplies } = CHANNEL_VIEW;
  const log = [];
  const entries = new Map();
  const main = { name: "main" };
  const side = { name: "side" };
  const owner = createColumnReplies({
    el: () => ({ hidden: false, children: [{ getAttribute: () => "e1" }] }),
    outbox: { has: (id) => entries.has(id), get: (id) => entries.get(id) },
    retryOf: () => null, retryPending: () => false,
    inPlaceDrawn: () => undefined, layoutColumns: () => true,
    isSide: (c) => c === side, isMain: (c) => c === main, sideShowing: () => true,
    inColumn: (_c, act) => act(), columnKey: (c) => c.name,
    activeColumn: () => side, setStatus: (t) => log.push(t), renderJumpNewest: () => log.push("jump"),
  });
  const failed = { id: "e1", state: "failed", postedCount: 0 };
  entries.set("e1", failed);
  owner.columnReplies.set("e1", side);
  owner.reportColumnReply(failed);
  assert.deepStrictEqual(log, ["Your reply was not sent. Retry is at the foot of the right column.", "jump"]);
  assert.equal(owner.unseenColumnReply(), failed, "the unseen reply in the active column was not the failed one");
  owner.reportColumnReply({ id: "e1", state: "sent", postedCount: 1 });
  entries.delete("e1");
  assert.equal(owner.columnReplies.has("e1"), false, "a sent reply kept its column");
  assert.equal(owner.unseenColumnReply(), null);
});

test("`#624 voice-page-modularity`: a column's reply arrows start fresh, and a landed row is put out by the next landing", () => {
  const { createReplyArrows, freshReplyArrows } = CHANNEL_VIEW;
  const active = freshReplyArrows();
  assert.deepStrictEqual(active, { back: null, landed: null, tap: null, settling: null, held: 0, owed: false, countsDrawn: false });
  const log = [];
  const rows = new Map();
  const owner = createReplyArrows({
    el: (id) => (id === "discord-log" ? { children: [...rows.values()] } : id === "scroll-area" ? { hidden: false, children: [], scrollTop: 0, clientHeight: 100, getBoundingClientRect: () => ({ top: 0, bottom: 100, height: 100 }) } : { hidden: true, children: [] }),
    leftEl: () => ({ hidden: true, getBoundingClientRect: () => ({ top: 0, bottom: 0, height: 0 }) }),
    inThisColumn: (fn) => fn,
    guardQuietly: (fn) => fn,
    gatherFromReply: () => log.push("gather"),
    filterState: () => ({ hold: null }),
    endFilterWalk: () => {}, endSearchWalks: () => {},
    selectLanded: (row) => log.push(`select:${row.getAttribute("data-id")}`),
    setSearchOpen: () => {}, openReadRun: () => {}, renderScrollTools: () => log.push("tools"),
    nudge: (area, delta) => { area.scrollTop += delta; },
    setStatus: (text) => log.push(text),
    channelContextKey: () => "ctx",
    heldMessage: () => null,
    inView: () => false,
    selection: () => ({ view: "main", threadId: null }),
    threading: () => true,
    olderPages: () => ({ moreAbove: false, cursor: null }),
    viewKey: () => "main",
    channelCanon: () => ({ views: new Map() }),
    selectedChannelId: () => "C1",
    changeChannelView: async () => {},
    channelReadsBusy: () => false,
    loadOlder: () => null,
    walkCursorOwed: () => false,
    hidingRead: () => false,
    timelineMessages: () => [],
    hiddenEmpty: () => false,
    readHiding: () => false,
    layoutColumns: () => false,
    columnSteps: () => ({ pause: () => {}, resume: () => {} }),
    activeArrows: () => active,
    replyJumpTicket: () => { let n = 0; return { current: () => n, bump: () => { n += 1; } }; },
  });
  const row = { getAttribute: (k) => (k === "data-ids" ? "m1" : k === "data-id" ? "m1" : null), setAttribute: () => {}, removeAttribute: () => log.push("unland"), className: "", children: [], getBoundingClientRect: () => ({ top: 10, bottom: 30, left: 0 }) };
  rows.set("m1", row);
  assert.equal(owner.rowHolding("m1"), row, "the row holding the message was not found");
  assert.equal(owner.landOnAnswered("m1", "m2"), true, "the jump did not land on the row on screen");
  assert.deepStrictEqual(log, ["select:m1", "tools", "Here is the message it answers."], "the landing did not light the row and say where it is");
  assert.equal(owner.landOnAnswered("nope", "m2"), false, "a jump landed on a row that is not on screen");
  assert.equal(freshReplyArrows().countsDrawn, false, "a fresh column shared its arrows record");
});

test("`#624 voice-page-modularity`: replies are gathered by pointer and by thread, to any depth, and a fresh column gathers nothing", () => {
  const { createReplyGathering, freshGatherState, freshListState } = CHANNEL_VIEW;
  assert.deepStrictEqual(freshGatherState(), { gathered: null });
  assert.deepStrictEqual(freshListState(), { rowsGathered: false, outgoingKey: "", newestId: null, runRows: 0, backlog: 0 });
  const gatherRecord = freshGatherState();
  const listRecord = freshListState();
  const owner = createReplyGathering({
    el: () => ({ children: [] }),
    inThisColumn: (fn) => fn,
    threading: () => true,
    selection: () => ({ view: "flat", threadId: null }),
    channelContextKey: () => "ctx",
    knownThreadChoice: () => null,
    selectedChannelId: () => "C1",
    channelCanon: () => ({ channel: "C1", scope: null, messages: [] }),
    loadCanon: () => {},
    heldMessage: () => null,
    heldThreadRecord: (message) => (message.thread ? { id: "t1", is_root: message.thread.is_root === true } : null),
    rowMessages: () => [],
    glom: (messages) => [ [...messages] ],
    discordNode: () => ({}),
    unpackMessages: () => [], unpackHeldMessages: () => null, unpackNeedsRead: () => false,
    queueUnpackRead: () => {}, unpackFilter: () => null,
    layoutColumns: () => false, sideColumn: () => null,
    openThread: () => {}, searchOpen: () => false, setSearchOpen: () => {},
    seekMessage: async () => {}, rowHolding: () => null,
    setStatus: () => {}, renderChannelRows: () => {}, renderScrollTools: () => {},
    countOf: (n) => `${n} replies`,
    hidingRead: () => false,
    threadReplies: () => ({}),
    timelineRows: () => ({ messages: [] }),
    renderJumpNewest: () => {}, renderReplyCounts: () => {}, measureReplyArrows: () => {},
    nudge: (area, delta) => { area.scrollTop += delta; }, glideTo: (area, top) => { area.scrollTop = top; },
    cardRow: () => null, finishPackSlide: () => {}, glideBehavior: () => "auto",
    threadHue: () => "", openThreadRootId: () => null, openThreadChoice: () => null,
    rootInward: () => false, isOpenThreadRoot: () => false,
    replyArrows: () => ({ settling: null }),
    takeClaimedClick: () => false,
    drawnTop: () => 0, floatingClearance: () => 0,
    activeGather: () => gatherRecord, activeList: () => listRecord,
  });
  const messages = [
    { id: "a" },
    { id: "b", reply_to: "a" },
    { id: "c", reply_to: "b" },
    { id: "d" },
    { id: "e", thread: { id: "t1", is_root: true } },
    { id: "f", thread: { id: "t1", is_root: false } },
  ];
  assert.deepStrictEqual([...owner.repliesTo("a", messages)].sort(), ["b", "c"],
    "a reply's replies were not gathered with it");
  assert.deepStrictEqual([...owner.repliesTo("e", messages)].sort(), ["f"],
    "a thread's replies were not gathered under its root");
  assert.equal(owner.gatheredParent(), null, "a fresh column already had a gathering");
});

test("`#624 voice-page-modularity`: a new collapsed-lines count is put on between the scroll keepers, and off the main screen it is simply put on", () => {
  const { createReadingColumn } = CHANNEL_VIEW;
  const log = [];
  let screen = "settings";
  const owner = createReadingColumn({
    foldsOf: () => [],
    setFolded: () => {}, isFolded: () => true,
    collapsedLines: { current: () => 3, apply: (value) => { log.push(`apply:${value}`); return 5; } },
    savedCollapsedLines: (n) => log.push(`saved:${n}`),
    stepCollapsedLinesPreference: (d) => log.push(`step:${d}`),
    screen: () => screen,
    mainColumn: () => "main", sideColumn: () => "side",
    inColumn: (column, act) => { log.push(`in:${column}`); return act(); },
    captureScroll: () => { log.push("capture"); return "mark"; },
    restoreScroll: () => log.push("restore"),
    sideShowing: () => false,
    keepingCardPlace: (act) => { log.push("cards"); act(); },
    defaultCollapsedLines: 3,
    documentById: () => null,
  });
  assert.equal(owner.collapsedLinesChanged("5"), 5, "off the main screen the count was not put on at once");
  assert.deepStrictEqual(log, ["apply:5", "saved:5"]);
  log.length = 0;
  screen = "main";
  assert.equal(owner.collapsedLinesChanged("7"), 5);
  assert.deepStrictEqual(log, ["in:main", "capture", "cards", "apply:7", "saved:5", "in:main", "restore"],
    "on the main screen the count was not put on between the scroll keepers");
  owner.stepCollapsedLines(-1);
  assert.equal(log[log.length - 1], "step:-1", "the slider's step did not go through the preference");
});

test("`#624 voice-page-modularity`: a sent reply marks what it answers Done on the server only for another channel, and the service is not asked where its marker is wider", async () => {
  const { createSentRead } = CHANNEL_VIEW;
  const log = [];
  const owner = createSentRead({
    el: () => ({ children: [] }),
    replyColumn: () => "main", mainColumn: () => "main", sideColumn: () => "side",
    sideShowing: () => false,
    inColumn: (_c, act) => act(),
    rowHolding: () => null,
    threading: () => true,
    channelCanon: () => ({ channel: "C1", scope: null, dismissed: new Set() }),
    selectedChannelId: () => "C1",
    readHiding: () => false,
    doneArchived: () => new Set(),
    inboxLedger: () => ({ add: (...args) => { log.push(["add", ...args.slice(0, 3)]); return "kept"; }, settle: (kept) => log.push(["settle", kept]) }),
    inboxWriteOwner: () => "owner",
    queueInboxWrite: async (act) => { await act(); },
    inboxPost: async (path, body) => { log.push(["post", path, body]); },
    toggleArchived: async () => { log.push(["toggle"]); },
    bookmarkAnchor: () => null,
    setStatus: (text) => log.push(["status", text]),
    doneSavingHas: () => false,
    doneSaved: async () => true,
    providerCount: () => 0,
    providerOfChannel: () => null,
    upstreamReadMark: () => true,
    heldUnansweredRoot: () => null,
    outboxValues: () => [],
    chatServiceOf: () => "chat",
    api: async () => ({ outcome: "scope_unknown" }),
    readableFailure: () => "why",
    serviceReadWords: () => "words",
  });
  assert.equal(await owner.markAnsweredDone("C2", "m9"), true, "the other channel's Done was not kept");
  assert.deepStrictEqual(log, [["add", "dismiss", "C2", ["m9"]], ["post", "/api/v1/channels/C2/dismiss", { messages: ["m9"] }], ["settle", "kept"]],
    "the other channel's Done did not go through the inbox write, kept and settled");
  assert.equal(owner.serviceMarkNotAsked("thread_channel"),
    "this channel is one thread of a larger space there, and its read marker covers the whole space.");
  assert.equal(owner.serviceMarkNotAsked("marked"), null);
  const mark = await owner.markReadInService("C1", "m1");
  assert.deepStrictEqual(mark, { service: "chat", asked: true, marked: false, already: false,
    why: "it cannot say whether its read marker covers more than this channel." },
  "the service was asked where it cannot say what its marker covers");
});

test("`#624 voice-page-modularity`: entering a thread lands on its first unread message, opened, and a thread with nothing unread is left where it was", () => {
  const { createThreadEntry, freshEntryState } = CHANNEL_VIEW;
  assert.deepStrictEqual(freshEntryState(), { threadOwed: null, threadAwaitsRead: false, readOwed: null, landing: null, backGesture: null });
  const record = freshEntryState();
  const log = [];
  const rows = [];
  const list = { children: rows };
  const area = { scrollTop: 0, clientHeight: 100, getBoundingClientRect: () => ({ top: 0, bottom: 100 }) };
  const entry = createThreadEntry({
    el: (id) => (id === "discord-log" ? list : id === "scroll-area" ? area : { hidden: true, getBoundingClientRect: () => ({ height: 0, top: 0, bottom: 0 }) }),
    activeEntry: () => record,
    channelContextKey: () => "ctx",
    selection: () => ({ view: "thread", threadId: "t1" }),
    channelViewsHas: () => true,
    viewKey: () => "thread:t1",
    applyReadRuns: () => log.push("runs"),
    atBottom: () => false,
    renderScrollTools: () => log.push("tools"),
    requestVisibleSummaries: () => log.push("summaries"),
    floatingClearance: () => 0,
    drawnTop: (row) => row.top,
    openThreadRootId: () => null,
    isOpenThreadRoot: () => false,
    rowIsRead: (row) => row.read === true,
    toFoot: () => log.push("foot"),
    nudge: (area, delta) => { area.scrollTop += delta; }, toPosition: (area, top) => { area.scrollTop = top; },
    setJumpNewest: (wanted) => log.push(`jump:${wanted}`),
    screen: () => "main",
    view: () => "discord",
    pinsView: () => ({ only: false }),
  });
  const read = { read: true, top: 40, hidden: false, className: "", parentNode: list, getAttribute: () => null, hasAttribute: () => false, getBoundingClientRect: () => ({ top: 40, height: 20, bottom: 60 }) };
  const unread = { read: false, top: 80, hidden: false, className: "", parentNode: list, getAttribute: (k) => (k === "data-collapsed" ? "true" : null), hasAttribute: () => false, getBoundingClientRect: () => ({ top: 80, height: 20, bottom: 100 }) };
  const after = { read: true, top: 120, hidden: false, className: "", parentNode: list, getAttribute: () => null, hasAttribute: () => false, getBoundingClientRect: () => ({ top: 120, height: 20, bottom: 140 }) };
  rows.push(read, unread, after);
  assert.equal(entry.landOnFirstUnread(), true, "the entry did not land on the first unread message");
  assert.deepStrictEqual(log, ["tools", "summaries"], "the landing was not drawn after");
  assert.equal(record.landing.first, unread, "the landing did not keep the first unread row");
  rows.length = 0;
  rows.push(read);
  assert.equal(entry.landOnFirstUnread(), false, "a thread with nothing unread was landed somewhere");
});

test("`#624 voice-page-modularity`: the layout button says back to the thread list out of two columns chosen from it, and a narrow window is told so", () => {
  const { createTwoColumns } = CHANNEL_VIEW;
  const log = [];
  const button = { hidden: false, title: "", attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, getClientRects: () => [{}] };
  const shown = { columns: false, threadList: false, layout: () => "one", noteFit: () => false, takeListCards: () => null, takeLeftColumn: () => null, sideUp: () => false, setColumns() {}, setThreadList() {}, askOneColumn() {}, takeOneColumn: () => null, whileRestoring() {}, holdListCards() {}, holdLeftColumn() {}, holdListPlace() {} };
  const choice = { picked: "two", chosen: () => choice.picked, beforeColumns: () => "list", choose: (l) => { choice.picked = l; log.push(`choose:${l}`); }, noteListBefore() {}, listBefore: () => ({ layout: "one" }), restoreBeforeColumns() {} };
  const owner = createTwoColumns({
    el: () => button, leftEl: () => button,
    layoutChoice: choice, layoutShown: shown,
    mainColumn: () => ({ name: "main" }), sideColumn: () => ({ name: "side", mode: "list" }),
    activeColumn: () => ({ name: "main" }), columnSide: () => false,
    useColumn: () => {}, inColumn: (_c, act) => act(),
    screen: () => "main", view: () => "discord", threading: () => true,
    selectedChannelId: () => "C1",
    selection: () => ({ view: "main", threadId: null, thread: null, origin: null }),
    channelCanon: () => ({ channel: "C1", scope: null, threads: [] }),
    settleFocus: () => log.push("focus"), setStatus: (t) => log.push(t),
    noteDeepLinkReaderMove: () => {}, guardQuietly: (fn) => fn,
    renderThreadSelect: () => {}, scheduleColumnLinks: () => {}, scheduleBackSync: () => {},
    cardAnchor: () => null, renderSideCards: () => {},
    readerPlace: () => ({ pinned: true, seen: [] }), keepReaderPlace: () => true,
    changeChannelView: () => {}, viewOnScreen: () => null,
    rememberView: () => {}, settleUiState: () => {}, saveUiState: () => {},
    configApplied: () => true, hydrateChannelScope: () => {}, restoreChannelComposer: () => {},
    selectSideThread: () => {}, showSideList: () => log.push("cards"),
    renderSearchInAll: () => {}, refreshSearchCount: () => {},
    closeReply: () => {}, replyState: () => ({ docked: false, target: null, pending: null, focusTaken: false }),
    nodeWithin: () => false, focusLost: () => false,
    endSearchWalk: () => {}, endRepliesWalk: () => {},
    dropReplyArrowTap: () => {}, putRepliesBack: () => {},
    rememberChannelDraft: () => {}, rememberChannelContext: () => {},
    reopenPendingReply: () => {}, renderMainToggle: () => {}, renderOrderToggle: () => {},
    placeMainCard: () => {}, renderUnpackToggle: () => {}, renderRepliesSearch: () => {},
    noteLinkedSelection: () => {}, closeOverscroll: () => {}, renderSideFreshness: () => {},
    setTextIfChanged: () => {}, nameSideRegion: () => {},
    paintThreadCard: () => {}, replyTally: () => null,
    knownThreadChoice: () => null, openThreadChoice: () => null,
    threadName: () => "thread", threadStillKnown: () => false,
    defaultChannelView: () => "main",
    openingChannelView: () => ({ view: "main", thread: null, summary: null, origin: null }),
    followIfPinned: () => {}, scrollToNewest: () => {},
    pinsView: () => ({ only: false }),
    leaveThreadList: () => {},
    catchUpHeldView: () => false, readFilterKeeps: () => () => true,
    settleAfterRead: () => {}, timelineRows: () => ({ messages: [] }),
    hiddenEmpty: () => false, hidingRead: () => false,
    channelReadPosition: () => ({}), syncUnreadReplies: () => {},
    placeCardAnchor: () => true, cardsPlacedUnderLists: () => {},
    sameCardPlace: () => false, liveFollowersMain: () => false,
    transcriptPlacePostCard: () => {}, transcriptCardShown: () => false,
    reopenedViewsIncludes: () => false, channelOnScreen: () => true,
  });
  owner.renderColumnToggle();
  assert.equal(button.attrs["aria-pressed"], "true", "two columns chosen from the list was not pressed on the button");
  assert.equal(button.title, "Back to the thread list");
  assert.equal(owner.columnsOffered(), true);
  assert.equal(owner.COLUMNS_TOO_NARROW, "Two columns need a wider window; they come back when it is wider.");
});

test("`#624 voice-page-modularity`: a card says its thread's replies and unread by the tally it is handed, and a fresh pass draws nothing twice", () => {
  const { createSideCards } = CHANNEL_VIEW;
  const owner = createSideCards({
    leftEl: () => ({ children: [], getAttribute: () => null, setAttribute: () => {} }),
    cardView: (choice) => ({ text: `words of ${choice.id}`, mode: "card", cut: null }),
    messageText: (m) => m.content,
    stillToDo: () => false,
    channelCanon: () => ({ channel: "C1", dismissed: new Set() }),
    sideColumn: () => ({}),
    inColumn: (_c, act) => act(),
    unseenReplies: () => 0,
    repliesPartlyHeld: () => false,
    exactReplyCount: () => 2,
    pendingInto: () => 0,
    threadCount: (count, exact) => exact === false ? `about ${count} replies` : `${count} replies`,
    unpackHeldMessages: () => [],
    unpackSignature: () => "", unpackReadSignature: () => "", unpackFilter: () => null,
    foldedOneIds: () => [],
    viewKey: () => "thread:t1",
    threadChoices: () => [],
    inThreadOrder: (choices) => [...choices],
    cardOrderOnScreen: () => "latest",
    watchPendingSummaries: () => {},
    cardIsSummarized: () => false,
    briefAge: () => "1h",
    threadName: (choice) => `name ${choice.id}`,
    unpackWatch: () => ({ unwatch: () => {}, stop: () => {} }),
    dressChannelRow: () => {}, answeredBy: () => null,
    replyTally: () => ({ threads: new Map() }),
    readMode: () => "show", readRunsSuspended: () => false,
    cardPackingMode: () => "", combineOn: () => false,
    nowPlaying: () => null, pendingRead: () => null, markReadingCard: () => {},
    mainRowsOnScreen: () => false, mainRowMessages: () => [],
    messageLook: () => "", readLook: () => "",
    olderThreadsToRead: () => false, keepingCardPlace: (act) => act(),
    sideCard: () => ({}), countSideCard: () => {}, paintLitCard: () => {},
    unpackBlock: () => [], redrawUnpackBlock: () => {},
    doneArchived: () => new Set(), setDoneArchived: () => {}, caughtUpDone: (held) => held,
    mainRunsAmong: () => [], mainRows: () => [],
    foldCardRuns: () => {}, filterSideCards: () => {},
    cardsCycle: () => false, sideCardsShown: () => false,
    outboxValues: () => [], eachColumn: () => {}, renderOutgoingMessages: () => {},
    replyState: () => ({ docked: false, pending: null }),
    placeInlineReply: () => {}, reopenPendingReply: () => {},
    setTextIfChanged: () => {}, threadPagesLoading: () => false,
    selectedChannelId: () => "C1", cardState: () => "whole",
  });
  const tally = { threads: new Map([["t1", { unread: 3, held: 2 }]]) };
  const choice = { id: "t1", count: 5, root: { id: "r1", content: "first line\nsecond line" } };
  assert.equal(owner.sideCardUnread(choice, tally), "3 unread");
  assert.equal(owner.sideCardReplies(choice, tally), "5 replies", "the card did not say the count it was handed");
  assert.deepStrictEqual(owner.cardMessageCount(choice, tally), { messages: 6, exact: true });
  assert.equal(owner.cardLines({ content: "a  b\n\nc" }), "a b\nc", "a card's lines were not folded as its root's are");
  assert.equal(owner.threadIsRead(choice, ""), true);
});

test("`#624 voice-page-modularity`: a filter change keeps the reader's message where it is, and the reader's own act lets it go", () => {
  const { createFilterPlace, freshFilterState } = CHANNEL_VIEW;
  assert.deepStrictEqual(freshFilterState(), { hold: null });
  const record = freshFilterState();
  const log = [];
  const area = { scrollTop: 100 };
  const row = { parentNode: null, className: "", getAttribute: () => null, hasAttribute: () => false, getBoundingClientRect: () => ({ top: 140 }) };
  const list = { children: [row] };
  row.parentNode = list;
  const owner = createFilterPlace({
    el: () => area,
    activeFilter: () => record,
    activeColumn: () => "main",
    columnSide: () => false,
    mainColumn: () => "main", sideColumn: () => "side",
    inColumn: (_c, act) => act(),
    eachColumn: (act) => act(),
    screen: () => "main", view: () => "discord",
    renderOlderControl: () => {}, dismissStatus: () => {},
    filterPlace: () => "main",
    pinsView: () => ({ only: false }),
    visibleList: () => list,
    listEmptied: () => false,
    atBottom: () => false,
    quickReplyHolds: () => false,
    endSearchWalks: () => {},
    scrollToNewest: () => log.push("newest"),
    nudge: (area, delta) => { area.scrollTop += delta; }, toPosition: (area, top) => { area.scrollTop = top; },
    renderJumpNewest: () => log.push("jump"),
    guardQuietly: (fn) => fn,
    setStatus: (text) => log.push(text),
    sidePane: () => ({ marks: new Map(), context: () => "main", head: () => 0, foot: () => 0 }),
    channelPane: () => ({ marks: new Map(), context: () => "main", head: () => 0, foot: () => 0 }),
    markOf: () => null, markedRow: () => null, rowInView: () => false,
    takeRow: () => {}, rowOf: (_p, r) => r, paneRows: () => [],
    releaseRow: () => {}, rowTimeTabStop: () => {}, drawSelection: () => {},
    hidingRead: () => false, threading: () => true,
    timelineRows: () => ({ messages: [] }),
    olderPages: () => ({ moreAbove: false, cursor: null }),
    loadOlder: () => null, walkCursorOwed: () => false,
    channelContextKey: () => "ctx",
    scrollAnchor: () => null,
    listPlaceTop: () => 100,
    floatingClearance: () => 0,
    messageTop: () => 150,
    rowMessages: () => [],
    clearEntryLanding: () => log.push("entry-spent"),
    sideShowing: () => false,
  });
  record.hold = { key: JSON.stringify(["main", false]), row, ids: ["m1"], offset: 40, at: 100, origin: { newest: false, row, ids: ["m1"], selected: null, drawn: false, focused: false, time: 1, offset: 40, alternate: null }, mark: null };
  assert.equal(owner.heldFilterRow(), row, "the held row was not the one the change kept");
  owner.readerActed();
  assert.equal(record.hold, null, "the reader's own act did not let the hold go");
  assert.equal(freshFilterState().hold, null, "a fresh column shared its filter hold");
});

test("`#624 voice-page-modularity`: the relay mode is read, drawn and switched from the module alone", () => {
  const { createLiveState } = CHANNEL_VIEW;
  const stored = new Map();
  const status = [];
  const calls = [];
  const els = new Map();
  const el = (id) => {
    if (!els.has(id)) els.set(id, { attrs: new Map(), textContent: "", title: "", value: "",
      setAttribute(name, value) { this.attrs.set(name, String(value)); } });
    return els.get(id);
  };
  const owner = createLiveState({
    deviceStorage: () => ({ getItem: (k) => stored.get(k) ?? null, setItem: (k, v) => stored.set(k, v) }),
    el,
    setStatus: (text) => status.push(text),
    renderChannelFreshness: () => calls.push("fresh"),
    connectionLive: () => calls.push("live"),
    liveFollow: { start: (id, from) => calls.push(["start", id, from]), stop: () => calls.push("stop"),
      whenAttached: (then) => { calls.push("wait"); then(); } },
  });
  assert.equal(owner.relayMode(), "gist", "an absent choice did not fall through to the default");
  stored.set(owner.RELAY_KEY, "off");
  assert.equal(owner.relayMode(), "off", "an old off did not stay off");
  stored.set(owner.RELAY_KEY, "on");
  assert.equal(owner.relayMode(), "gist", "an old on did not fall through to summarizing");
  owner.chooseReadNew("full");
  assert.equal(stored.get(owner.RELAY_KEY), "full");
  assert.equal(el("read-new").attrs.get("data-mode"), "full");
  assert.equal(el("read-new-label").textContent, "Full");
  assert.equal(el("relay-to-agent").value, "full");
  assert.deepStrictEqual(status, ["New messages: read out in full during a call"]);
  owner.renderLiveState();
  owner.startChannelStream("c1");
  owner.whenLiveAttached(() => calls.push("then"));
  owner.stopChannelStream();
  assert.deepStrictEqual(calls, ["fresh", "live", ["start", "c1", null], "wait", "then", "stop"]);
});

test("`#624 voice-page-modularity`: Done's state, saving wait and refused-write settle, from the module alone", async () => {
  const { createDoneAct, freshDoneState } = CHANNEL_VIEW;
  const a = freshDoneState(); const b = freshDoneState();
  a.archived.add("m1");
  assert.equal(b.archived.size, 0, "a fresh column shared its Done set");
  let timer = 0;
  const clock = { setTimeout: () => ++timer, clearTimeout: () => {}, now: () => 0 };
  const noop = () => {};
  const owner = createDoneAct({
    api: async () => ({}), clockOnUse: () => clock, dismissStatus: noop, doneHeldHere: () => false,
    doneSettlesHere: () => null, el: () => ({ children: [], hidden: false }),
    followDoneEverywhere: noop, guardQuietly: (fn) => fn, heldDonePart: (h) => h,
    hidingRead: () => false, holdingMessage: (fn) => fn(), holdingReplyArrows: (fn) => fn(),
    idsOf: () => [], inColumn: (_c, fn) => fn(), inOtherColumn: (fn) => fn(),
    listState: () => ({ backlog: 0, rowsGathered: false }), listedMessages: () => [],
    loadTodo: async () => {}, markHeldDone: () => null, messageText: () => "", nextUndoAt: () => 0,
    pinsView: () => ({ only: false }), readSpeaksFor: () => true, refreshAfterLiveMutation: noop,
    renderCachedTimeline: noop, renderChannelRows: noop, renderChannelSeam: noop,
    renderReadModeControl: noop, renderReplyLinks: noop, renderScrollTools: noop, renderSideColumn: noop,
    retryingWrite: (fn) => fn(), rowMessages: () => [], saveChannelScope: noop,
    selectedChannelId: () => "c1", selection: () => ({ view: "flat" }), sendBookmark: noop,
    setStatus: noop, settleHeldDone: noop, singleDoneBump: () => null, stageBookmark: () => null,
    statusSays: () => false, syncUnreadReplies: noop, threadForMessageId: () => null,
    timeOf: () => 0, todoSummary: () => null, token: () => "t", tokenFingerprint: () => "f",
    undoLabel: () => "Undo", undoName: () => "", voiceTranscript: () => ({ CLEAR_ARMED_MS: 100 }),
    writeInBatches: async () => [], channelStore: () => ({ current: () => ({ channel: "c1", dismissed: new Set() }) }),
    providerFacts: { threading: () => false },
    columnWork: () => ({ active: () => ({ done: a }), side: () => false }),
    columns: () => ({ side: { mode: "thread" } }),
    readModes: () => ({ hiding: () => false, mode: () => "show" }),
    readLedger: () => ({ writtenIds: () => [] }),
    lastDismissal: () => ({ current: () => null, set: noop }),
    inboxLedger: () => ({ add: () => 1, settle: noop, beat: noop, takeLeftOver: () => [] }),
  });
  assert.equal(owner.doneState(), a, "doneState was not the column's own record");
  owner.noteArchived({ dismissed: ["m2"] }, false);
  assert.deepStrictEqual([...a.archived].sort(), ["m1", "m2"]);
  const settle = owner.noteDoneSaving(["m1"]);
  let answered = null;
  const waiting = owner.doneSaved(["m1"]).then((v) => { answered = v; });
  await Promise.resolve();
  assert.equal(answered, null, "doneSaved answered before the write settled");
  settle();
  await waiting;
  assert.equal(answered, true, "doneSaved did not answer Done once settled");
  owner.noteInboxWrite(["m3"]); owner.noteInboxWrite(["m3"]);
  const stands = owner.settleInboxWrite(["m3"], true, new Set());
  assert.deepStrictEqual([...stands], ["m3"], "a refused write with a later one on its way took the act back");
});

test("`#624 voice-page-modularity`: a thread's count comes from its summary before its own record, and Main is inherited from All's cover", () => {
  const { createChannelViews } = CHANNEL_VIEW;
  const canon = {
    channel: "C1",
    threads: [{ id: "t1", reply_count: 4, reply_count_exact: false, root: { id: "r1" } }],
    messages: [],
    views: new Map([["flat", { live: true, at: 10, notice: "note", cursor: "c", newest: "n", fullAt: 1 }]]),
    dismissed: new Set(),
  };
  const owner = createChannelViews({
    el: () => ({ hidden: false, children: [], setAttribute: () => {}, replaceChildren: () => {} }),
    leftEl: () => ({ hidden: false }),
    columnSide: () => false,
    channelCanon: () => canon,
    view: () => "discord",
    threading: () => true,
    entryState: () => ({ threadOwed: null, threadAwaitsRead: false, readOwed: null, landing: null, backGesture: null }),
    selection: () => ({ view: "main", threadId: null, thread: null, origin: null }),
    readMode: () => "show",
    transcriptShown: () => false,
    selectedChannelId: () => "C1",
    heldMessage: () => null,
    providerOfChannel: () => null,
    providersSize: () => 0,
    repliesInThreadCapability: () => false,
    threadOf: (m) => (m && m.thread ? String(m.thread.id) : null),
    rowMessages: () => [],
    renderSideColumn: () => {}, renderThreadSelect: () => {}, renderThreadHeading: () => {},
    followReplyStore: () => {}, markListView: () => {}, paintAllThreadFocus: () => {},
    pinsView: () => ({ only: false }),
    renderOutgoingMessages: () => {}, syncCanonDismissed: () => {},
    timelineRows: () => ({ messages: [], threads: [] }),
    olderPages: () => ({ moreAbove: false, cursor: null }),
    captureScroll: () => null,
    loadState: () => ({ freshAt: 0, freshness: "fresh", generation: 0 }),
    doneState: () => ({ archived: new Set() }),
    idsOf: () => [], shownRowFor: () => null, atBottom: () => true,
    noteDeepLinkReaderMove: () => {}, putDockedReplyAway: () => {},
    replyDocked: () => false, replyColumn: () => null, activeColumn: () => null,
    putRepliesBack: () => {}, dropReplyArrowTap: () => {}, rememberChannelDraft: () => {},
    setPinnedOnly: () => {}, viewOnScreen: () => null, saveUiState: () => {},
    scheduleBackSync: () => {}, voiceCall: () => ({ renderControls: () => {} }),
    threadOutrunsStore: () => false, heldViewRows: () => [],
    drawProjection: () => false, drawHeldRows: () => false,
    setChannelFreshness: () => {}, saveChannelScope: () => {},
    walkCursorOwed: () => false, maybeLoadOlder: () => {},
    allReachesThread: () => false, storeLacksReplies: () => false,
    projectView: () => null, channelReadPosition: () => ({}),
    readFilterKeeps: () => () => true, settleAfterRead: () => {},
    hiddenEmpty: () => false, hidingRead: () => false,
    renderChannelLoading: () => {}, restoreChannelComposer: () => {},
    renderOlderControl: () => {}, restoreScroll: () => {},
    renderCachedTimeline: () => {}, syncUnreadReplies: () => {},
    renderChannelFreshness: () => {}, payThreadEntry: () => {},
    rowsMissDone: () => false, scrollToNewest: () => {}, renderJumpNewest: () => {},
    nudge: (area, delta) => { area.scrollTop += delta; }, toPosition: (area, top) => { area.scrollTop = top; },
    threadCards: () => [], loadDiscordTimeline: async () => {},
    api: async () => null,
    deviceStorage: () => ({ getItem: () => null, setItem: () => {}, removeItem: () => {} }),
    tokenScope: () => "read",
    channelRecord: () => null, timeOfRecord: () => 0,
    threadOfMessage: () => null,
    layoutShown: () => ({ columns: () => false, threadList: () => false, restoring: () => false }),
    stopReading: () => {}, setReadingMode: () => {},
    readRunsForget: () => {},
    viewKey: (view = "main", threadId = null) => (view === "thread" && threadId ? `thread:${threadId}` : view),
    rememberView: () => {}, forgetChosenView: () => {}, settleUiState: () => {},
    payReadCursor: () => {}, readsBusy: () => false, readContext: () => "ctx",
    channelContexts: () => new Map(), listState: () => ({ newestId: null }),
  });
  assert.deepStrictEqual(owner.threadReplies("t1", null), { count: 4, exact: false },
    "the count did not come from the thread's summary");
  assert.equal(owner.threadCount(1, true), "1 reply");
  assert.equal(owner.threadCount(null, false), "Replies");
  assert.equal(owner.inheritAllCover("main"), true, "Main was not inherited from All's cover");
  const cover = canon.views.get("main");
  assert.equal(cover.fromAll, true);
  assert.equal(cover.notice, "", "All's notice came with Main's cover");
  assert.equal(cover.cursor, null, "Main kept All's forward cursor");
});

test("`#624 voice-page-modularity`: reply counts by pointer and the unanswered root, from the module alone", () => {
  const { createReplyCounts } = CHANNEL_VIEW;
  const canon = { channel: "c1", scope: null, dismissed: new Set(), views: new Map(), threads: [],
    messages: [
      { id: "r1", thread: { id: "t1", is_root: true, reply_count: 0, reply_count_exact: true, root_message_id: "r1" } },
      { id: "a1", reply_to: "r1", timestamp: "2026-01-01T00:00:01Z", thread: { id: "t1", is_root: false, root_message_id: "r1" } },
      { id: "a2", reply_to: "r1", timestamp: "2026-01-01T00:00:02Z" },
      { id: "b1", reply_to: "r1", timestamp: "2026-01-01T00:00:03Z", thread: { id: "t1", is_root: false, root_message_id: "r1", broadcast: true } },
      { id: "o1", timestamp: "2026-01-01T00:00:04Z" },
      { id: "a3", reply_to: "o1", timestamp: "2026-01-01T00:00:05Z" },
    ] };
  const noop = () => {};
  const owner = createReplyCounts({
    answeredElsewhere: () => false, cardSplit: () => false,
    el: () => ({ children: [], hidden: false }), focusMinimally: noop,
    gatherCount: () => ({ count: 0, exact: true }), guardQuietly: (fn) => fn,
    hiddenEmpty: () => false, hidingRead: () => false,
    idsOf: (row) => row.ids || [], inColumn: (_c, fn) => fn(), inThisColumn: (fn) => fn,
    inView: () => false, knownChannel: () => null,
    listState: () => ({ outgoingKey: "" }), openReadRun: noop, openThread: noop,
    openThreadChoice: () => null, openThreadRootId: () => null, outgoingReplyKey: () => "",
    paintThreadStrip: noop, pendingInto: () => 0, quoteLabelFits: () => true,
    renderScrollTools: noop, replyArrows: () => ({ countsDrawn: false }), revealRow: noop,
    rootInward: () => false, rowHolding: () => null, rowMessages: (row) => row.messages || [],
    rowShown: () => true, seekMessage: async () => {}, selectedChannelId: () => "c1",
    selection: () => ({ view: "main", threadId: null }), sentFromThisPage: () => false,
    setSearchOpen: noop, setStatus: noop, shownNode: () => true,
    stillToDo: () => true, threadCount: (n) => `${n}`, threadHue: () => "1",
    threadReplyCount: () => null, timeOf: (m) => Date.parse(m && m.timestamp) || NaN,
    viewKey: (view, threadId) => (view === "thread" && threadId ? `thread:${threadId}` : view),
    channelStore: () => ({ current: () => canon }), providerFacts: { threading: () => true }, broadcastSupported: () => true,
    columns: () => ({ main: null, side: null }), layoutShown: () => ({ columns: () => false }),
    outbox: () => ({ values: () => [] }), outboxDispatch: () => ({ retryPending: () => false }),
    quoteOwner: () => ({ held: () => null }),
  });
  assert.equal(owner.unansweredRoot(canon.messages[0], 0, 0, null), true, "a root with a zero count was not unanswered");
  assert.equal(owner.unansweredRoot(canon.messages[0], 0, 1, null), false, "a held reply left the root unanswered");
  const rootRow = { ids: ["r1"], messages: [canon.messages[0]] };
  const { replies } = owner.heldReplies([rootRow]);
  // a1 (thread-only) and b1 (broadcast) name their own thread's root: the chips count them, not
  // Reply (N). a2 is main-only (`#1165`): naming the root from the channel alone counts nowhere.
  assert.deepStrictEqual([...(replies.get("r1") || new Map()).keys()], [], "a main-only reply counted toward the thread");
  assert.deepStrictEqual([...(replies.get("o1") || new Map()).keys()], ["a3"], "a reply to an ordinary message no longer counted");
  const tally = owner.replyTally();
  assert.equal(tally.threads.get("t1").held, 2, "the tally did not hold the thread-only and broadcast replies");
});

test("`#624 voice-page-modularity`: removals and the placing wait, from the module alone", () => {
  const { createPlacing } = CHANNEL_VIEW;
  const canon = { channel: "c1", scope: null, messages: [], threads: [], views: new Map(), dismissed: new Set(),
    hasThreads: false, unplaced: new Set(), unplacedSince: new Map(), unplacedAnswered: new Set(),
    removed: new Map(), complete: null, replyGaps: new Map(), passedOver: new Set(), settledByDone: new Map() };
  canon.messages.push({ id: "m1" });
  let waits = true;
  const owner = createPlacing({
    channelStore: () => ({ current: () => canon }), providerFacts: { threading: () => true },
    broadcastSupported: () => false,
    unplacedArrivals: () => ({ count: () => 0 }),
    placingReadInFlight: () => ({ busy: () => false, begin: () => {}, end: () => {} }),
    readOrder: () => ({ asked: () => 7 }),
    readHolds: () => ({ channelWaits: () => waits }),
    liveEvents: () => ({ seen: () => 0, news: () => 0, removals: () => 0 }),
    abandonedReads: () => ({ holdKey: () => "k", abandon: () => null }),
    channelReads: () => ({ busy: () => false }), copyLedger: () => ({}),
    timelineReader: () => ({ readTimeline: async () => ({}) }),
    columns: () => ({ main: null, side: null }), jumpNewestWanted: () => ({ discord: false }),
    timelineReconcileMs: 1, readTimeoutMs: () => 1, hiddenNow: () => false,
    eachColumn: () => {}, el: () => ({ children: [] }),
    foldTimelineDelta: () => true, foldTimelinePage: () => {},
    hiddenEmpty: () => false, hidingRead: () => false, inColumn: (_c, fn) => fn(),
    isCursorRefusal: () => false, noteArchived: () => {}, projectView: () => null,
    readFilterKeeps: () => () => true, renderCachedTimeline: () => {},
    renderChannelNavigation: () => {}, renderScrollTools: () => {}, saveChannelScope: () => {},
    selectedChannelId: () => "c1", selection: () => ({ view: "main", threadId: null }),
    sentNumberOf: () => 0, settleAfterRead: () => {}, syncUnreadReplies: () => {},
    timelineRows: () => ({ messages: [] }), viewKey: (view) => view,
    within: async (_ms, fn) => fn(), channelReadPosition: () => ({}), catchUpFromAll: () => {},
  });
  assert.equal(owner.placeUnplacedWaits("c1"), true, "the placing read did not wait on a held read");
  waits = false;
  assert.equal(owner.placeUnplacedWaits("c1"), false, "the placing read still waited with nothing held");
  canon.removed.set("m1", 1);
  assert.equal(owner.removedUpstream("m1"), true, "a removed message was not said removed upstream");
  assert.equal(owner.removedUpstream("other"), false, "a message nobody removed was said removed upstream");
});

test("`#1144 scrollback-typing-jump`: the scroll trace wraps, rings and restores, from the module alone", () => {
  const { createScrollTrace, SCROLL_TRACE_MAX_ENTRIES } = CHANNEL_VIEW;
  // A stand-in Element.prototype: scrollTop kept per node, the three methods moving it.
  const proto = {};
  Object.defineProperty(proto, "scrollTop", {
    configurable: true,
    get() { return this._top || 0; },
    set(value) { this._top = value; },
  });
  const heldTop = Object.getOwnPropertyDescriptor(proto, "scrollTop");
  proto.scrollTo = function (...args) {
    const top = typeof args[0] === "object" && args[0] !== null ? args[0].top : args[1];
    if (typeof top === "number") this._top = top;
  };
  proto.scrollBy = function (...args) {
    const top = typeof args[0] === "object" && args[0] !== null ? args[0].top : args[1];
    if (typeof top === "number") this._top = (this._top || 0) + top;
  };
  proto.scrollIntoView = function () {};
  const [heldTo, heldBy, heldInto] = [proto.scrollTo, proto.scrollBy, proto.scrollIntoView];
  const area = (id) => {
    const listeners = new Map();
    return Object.assign(Object.create(proto), {
      id, scrollHeight: 5000, clientHeight: 600, _listeners: listeners,
      addEventListener: (type, fn) => listeners.set(type, fn),
      removeEventListener: (type) => listeners.delete(type),
      contains: (other) => other._inside === id,
      getBoundingClientRect: () => ({ top: 10, bottom: 610, width: 300, height: 600 }),
    });
  };
  const main = area("scroll-area");
  const side = area("side-scroll");
  const composer = { getBoundingClientRect: () => ({ top: 700, bottom: 760, width: 390, height: 60 }) };
  const page = { "scroll-area": main, "side-scroll": side, "channel-composer": composer };
  let clock = 1000;
  const trace = createScrollTrace({
    lookup: (id) => page[id] ?? null,
    activeElement: () => ({ id: "channel-compose-text" }),
    viewport: () => ({ height: 500, offsetTop: 120 }),
    anchor: () => ({ id: "m-1", offset: -4 }),
    elementPrototype: () => proto,
    now: () => clock,
    stack: () => "Error\n at taker (a:1:1)\n at wrapper (b:2:2)\n at asker (c:3:3)\n at caller (d:4:4)",
  });
  main.scrollTop = 50; // Before the start: nothing is traced.
  trace.start();
  trace.start(); // Twice is once: no double wrap, so one write is one entry.
  clock = 1100; main.scrollTop = 200;
  const stray = Object.assign(Object.create(proto), { id: "elsewhere", scrollHeight: 10, clientHeight: 5 });
  stray.scrollTop = 5; // Not a list: its writes are not traced...
  clock = 1200; stray.scrollIntoView(); // ...but asking to show it is.
  clock = 1300; side.scrollBy({ top: 40 });
  clock = 1400; side._top = 90; side._listeners.get("scroll")();
  let entries = JSON.parse(trace.report()).entries;
  assert.deepStrictEqual(entries.map(({ kind, id, from, to }) => [kind, id, from, to]), [
    ["write", "scroll-area", 50, 200],
    ["into", "elsewhere", null, null],
    ["write", "side-scroll", 0, 40],
    ["scroll", "side-scroll", 40, 90],
  ]);
  assert.equal(entries[0].stack, "at asker (c:3:3) | at caller (d:4:4)");
  assert.equal(entries[0].focus, "channel-compose-text");
  assert.deepStrictEqual([entries[0].composer, entries[0].vv, entries[0].anchor],
    [{ top: 700, bottom: 760, width: 390, height: 60 }, { height: 500, offsetTop: 120 }, { id: "m-1", offset: -4 }]);
  assert.equal(JSON.parse(trace.report()).startedAt, 1000);
  // The ring: a full minute of writes keeps the newest SCROLL_TRACE_MAX_ENTRIES, and a write a
  // minute past the held ones leaves it holding that write alone.
  for (let at = 0; at < SCROLL_TRACE_MAX_ENTRIES + 5; at += 1) { clock = 2000 + at; main.scrollTop = at; }
  entries = JSON.parse(trace.report()).entries;
  assert.equal(entries.length, SCROLL_TRACE_MAX_ENTRIES);
  assert.equal(entries[entries.length - 1].to, SCROLL_TRACE_MAX_ENTRIES + 4);
  clock += 61_000; main.scrollTop = 7;
  assert.deepStrictEqual(JSON.parse(trace.report()).entries.map(({ to }) => to), [7]);
  // Stopping puts every piece back as it was, takes the listeners down, and ends the tracing.
  trace.stop();
  assert.deepStrictEqual(
    [proto.scrollTo, proto.scrollBy, proto.scrollIntoView, Object.getOwnPropertyDescriptor(proto, "scrollTop").get,
      Object.getOwnPropertyDescriptor(proto, "scrollTop").set, side._listeners.size],
    [heldTo, heldBy, heldInto, heldTop.get, heldTop.set, 0]);
  clock += 10; main.scrollTop = 999;
  assert.deepStrictEqual(JSON.parse(trace.report()).entries.map(({ to }) => to), [7],
    "what was traced stays for the report, and nothing joins it");
  assert.equal(main.scrollTop, 999, "the write itself still landed");
});

test("`#1208 scroll-trace-layout-shift`: shifts, replacements and a lost selection are traced and sent by themselves", () => {
  const { createScrollTrace } = CHANNEL_VIEW;
  const proto = {};
  Object.defineProperty(proto, "scrollTop", {
    configurable: true,
    get() { return this._top || 0; },
    set(value) { this._top = value; },
  });
  proto.scrollTo = function () {};
  proto.scrollBy = function () {};
  proto.scrollIntoView = function () {};
  const listenersOf = () => {
    const map = new Map();
    return {
      addEventListener: (type, fn) => map.set(type, [...(map.get(type) || []), fn]),
      removeEventListener: (type, fn) => map.set(type, (map.get(type) || []).filter((each) => each !== fn)),
      fire: (type) => { for (const fn of map.get(type) || []) fn(); },
      count: (type) => (map.get(type) || []).length,
    };
  };
  const main = Object.assign(Object.create(proto), {
    id: "scroll-area", scrollHeight: 5000, clientHeight: 600, ...listenersOf(),
    contains: () => false, getBoundingClientRect: () => ({ top: 0, bottom: 600, width: 300, height: 600 }),
  });
  const log = Object.assign(Object.create(proto), { id: "discord-log", ...listenersOf() });
  const page = { "scroll-area": main, "channel-composer": null };
  const doc = listenersOf();
  let clock = 10000;
  let sel = { collapsed: true, length: 0, desc: "" };
  const sent = [];
  let shiftRun = null;
  let mutationRun = null;
  const heldPO = globalThis.PerformanceObserver;
  const heldMO = globalThis.MutationObserver;
  globalThis.PerformanceObserver = class { constructor(run) { shiftRun = run; } observe() {} disconnect() { shiftRun = null; } };
  globalThis.MutationObserver = class { constructor(run) { mutationRun = run; } observe() {} disconnect() { mutationRun = null; } };
  try {
    const trace = createScrollTrace({
      lookup: (id) => page[id] ?? null,
      activeElement: () => null,
      viewport: () => ({ height: 600, offsetTop: 0 }),
      anchor: () => ({ id: "m-1", offset: 0 }),
      elementPrototype: () => proto,
      now: () => clock,
      stack: () => "Error\n at taker (a:1:1)\n at wrapper (b:2:2)\n at asker (c:3:3)",
      containers: () => [log],
      documentTarget: () => doc,
      selection: () => sel,
      autoSend: (text) => sent.push(text),
    });
    trace.start();
    const row = (id) => ({
      tagName: "LI", id: "", getAttribute: (name) => (name === "data-id" ? id : null), querySelector: () => null,
    });
    // A shift with a hand just in it is traced but never sent by itself.
    doc.fire("pointerdown");
    clock = 10100;
    shiftRun({ getEntries: () => [{ value: 0.2, hadRecentInput: true, sources: [{ node: row("m-9"), previousRect: { x: 0, y: 300, width: 300, height: 40 }, currentRect: { x: 0, y: 320, width: 300, height: 40 } }] }] });
    assert.equal(sent.length, 0, "a shift after the reader's own touch posted itself");
    // A shift nobody made: traced with its sources, and sent at once - once a minute.
    clock = 11000;
    shiftRun({ getEntries: () => [{ value: 0.05, hadRecentInput: false, sources: [{ node: row("m-9"), previousRect: { x: 0, y: 300, width: 300, height: 40 }, currentRect: { x: 0, y: 260, width: 300, height: 40 } }] }] });
    assert.equal(sent.length, 1, "a qualifying shift did not post the trace by itself");
    clock = 12000;
    shiftRun({ getEntries: () => [{ value: 0.05, hadRecentInput: false, sources: [] }] });
    assert.equal(sent.length, 1, "a second qualifying shift inside the minute posted again");
    // A row replaced for the same message, and a selection lost with no hand in it.
    clock = 13000;
    mutationRun([{ removedNodes: [row("m-9")], addedNodes: [row("m-9")] }]);
    sel = { collapsed: false, length: 12, desc: "m-9" };
    doc.fire("selectionchange");
    sel = { collapsed: true, length: 0, desc: "" };
    clock = 71500;
    doc.fire("selectionchange");
    assert.equal(sent.length, 2, "a lost selection did not post the trace by itself");
    // The ring holds the last minute: the touched shift and the first qualifying one are gone.
    const entries = JSON.parse(trace.report()).entries;
    assert.deepStrictEqual(entries.map(({ kind }) => kind), ["shift", "mutate", "selection"]);
    assert.equal(entries[0].value, 0.05);
    assert.equal(entries[0].input, false);
    assert.match(entries[1].stack, /m-9 removed LI#\[m-9\] added LI#\[m-9\]/);
    assert.match(entries[2].stack, /was m-9 len 12/);
    assert.ok(!("text" in entries[2]) && !JSON.stringify(entries[2]).includes("twelve chars!"), "selection words were traced");
    // `#1224 mention-jump-in-view`: a jump target lost is noted, and posts itself a minute on.
    clock = 140000;
    trace.noteJumpLost("m-9");
    const jumped = JSON.parse(trace.report()).entries.at(-1);
    assert.equal(jumped.kind, "jump-lost");
    assert.equal(jumped.stack, "m-9");
    assert.equal(sent.length, 3, "a lost jump target did not post the trace by itself");
    trace.stop();
    assert.equal(doc.count("selectionchange"), 0, "stopping left the selection listener up");
  } finally {
    globalThis.PerformanceObserver = heldPO;
    globalThis.MutationObserver = heldMO;
  }
});

test("`#1186 two-column-splitter`: the separator drags, clamps, keeps each column's place, and resets, from the module alone", () => {
  const { createColumnSplitter } = CHANNEL_VIEW;
  const clampSplit = (value) => {
    const n = Number(value);
    return Number.isFinite(n) ? Math.min(70, Math.max(30, Math.round(n))) : 50;
  };
  const attrs = new Map();
  const separator = {
    setAttribute: (name, value) => attrs.set(name, String(value)),
    setPointerCapture: () => {},
    focus: () => {},
    getBoundingClientRect: () => ({ left: 0, width: 1000 }),
  };
  const vars = new Map();
  const screenMain = {
    style: { setProperty: (name, value) => vars.set(name, value), cursor: "" },
    setPointerCapture: () => {},
    getBoundingClientRect: () => ({ left: 0, width: 1000 }),
  };
  const kept = [];
  const places = { main: { seen: [{ ids: ["m1"], top: 100 }] }, side: { seen: [{ ids: ["s1"], top: 200 }] } };
  const restored = [];
  let working = "main";
  const splitter = createColumnSplitter({
    lookup: (id) => (id === "column-splitter" ? separator : screenMain),
    keep: (value) => kept.push(value),
    clampSplit,
    defaultSplit: 50,
    columnsOn: () => true,
    mainColumn: () => "main",
    sideColumn: () => "side",
    inColumn: (column, act) => { const before = working; working = column; try { return act(); } finally { working = before; } },
    readerPlace: () => places[working],
    keepReaderPlace: (place) => { restored.push([working, place]); return true; },
    scheduleColumnLinks: () => {},
  });
  assert.equal(splitter.apply(140), 70, "apply did not clamp to the widest split");
  assert.equal(vars.get("--two-column-split"), "70%");
  assert.equal(attrs.get("aria-valuenow"), "70");
  splitter.apply(50);
  // A press away from the boundary claims nothing; one on it begins the drag.
  assert.equal(splitter.down({ pointerId: 1, clientX: 100 }), false, "a press off the boundary claimed the drag");
  assert.equal(splitter.down({ pointerId: 1, clientX: 500 }), true, "a press on the boundary did not claim the drag");
  // A drag to 300px of a 1000px screen: the percentage floor (30) and the pixel floor agree.
  restored.length = 0;
  splitter.move({ clientX: 300 });
  assert.equal(splitter.split(), 30);
  assert.deepStrictEqual(restored, [["main", places.main], ["side", places.side]],
    "the drag did not put each column's reader back on the place it read before the width changed");
  // Past the right edge the left column stops at its widest, never wider.
  splitter.move({ clientX: 990 });
  assert.equal(splitter.split(), 70);
  splitter.up();
  assert.deepStrictEqual(kept, [70], "the split was not kept once, where the drag stopped");
  // The keys move it a point at a time and keep as they go; double-click is half and half again.
  splitter.key({ key: "ArrowLeft", preventDefault: () => {} });
  assert.equal(splitter.split(), 69);
  assert.equal(splitter.doubleClick({ clientX: 100 }), false, "a double-click off the boundary reset the split");
  assert.equal(splitter.doubleClick({ clientX: 690 }), true);
  assert.equal(splitter.split(), 50);
  assert.equal(kept[kept.length - 1], 50);
});

test("`#624 voice-page-modularity`: a column's older-pages record and the channel seam, from the module alone", () => {
  const { createOlderPages, freshOlderPages } = CHANNEL_VIEW;
  const a = freshOlderPages(); const b = freshOlderPages();
  a.cursor = "x";
  assert.equal(b.cursor, null, "a fresh column shared its older pages");
  assert.deepStrictEqual(freshOlderPages(), { moreAbove: false, cursor: null, inFlight: false, step: null, deferred: false });
  const seen = [];
  const summary = { replaceChildren: (...kids) => seen.push(kids) };
  const owner = createOlderPages({
    activeColumn: () => ({ older: a }),
    el: () => summary,
    transcript: () => ({ seam: (label, detail) => ({ label, detail }) }),
  });
  assert.equal(owner.olderPages(), a, "olderPages was not the column's own record");
  owner.renderChannelSeam("12 messages");
  assert.equal(seen.length, 1);
  assert.equal(seen[0][0].label, "12 messages");
  assert.match(seen[0][0].detail, /Older messages load as you scroll up/);
  assert.equal(typeof owner.plainReach.gapFrom, "function");
});

test("`#624 voice-page-modularity`: which view a channel opens in and the saved place, from the module alone", () => {
  const { createOpeningView } = CHANNEL_VIEW;
  const store = new Map();
  const recordState = {
    chosen: { channelView: "thread", thread: { id: "t1", origin: "main", summary: null } },
    place: null, forgotten: 0,
  };
  const threadKnown = new Set(["t1"]);
  const canon = { channel: "c1", scope: null, messages: [], dismissals: [], dismissed: new Set(),
    views: new Map(), threads: [], hasThreads: false,
    unplaced: new Set(), unplacedSince: new Map(), unplacedAnswered: new Set(),
    removed: new Map(), complete: null, replyGaps: new Map(), settledByDone: new Map() };
  const owner = createOpeningView({
    uiState: () => ({
      chosenView: () => recordState.chosen, chosenViewsHeld: () => [],
      forgetChosen: () => { recordState.forgotten += 1; recordState.chosen = undefined; },
      setChosen: () => {}, remember: () => {}, heldPlace: () => recordState.place,
      holdPlace: (held) => { recordState.place = held; }, settled: () => true,
      saved: () => null,
    }),
    providerFacts: { threading: () => true },
    reopenedViews: ["main", "flat", "thread"], uiStateKey: "k", uiStateVersion: 1, uiStateChars: 100000,
    channelStore: () => ({ current: () => canon }), threadPresence: () => ({ forget: () => {} }),
    columns: () => ({ main: null, side: null }), readModes: () => ({ hiding: () => false, mode: () => "show" }),
    screens: () => ({ screen: () => "main", view: () => "discord" }), columnWork: () => ({ side: () => false }),
    messageCache: () => ({ storedExactly: (k, v) => store.set(k, v) }),
    captureScroll: () => ({ pinned: false, anchor: null, top: 0 }), channelContextKey: () => "c1",
    deepLinkFromAddress: () => null, deepLinkReaderMoved: () => false,
    el: () => ({ }), idsOf: () => [], inColumn: (_c, fn) => fn(), listPlaceTop: () => 0,
    loadCanon: () => {}, renderColumns: () => {}, restoreChannelComposer: () => {},
    restoreScroll: () => {}, scheduleBackSync: () => {}, selectedChannelId: () => "c1",
    selection: () => ({ view: "flat", thread: null, threadId: null, origin: "main" }),
    token: () => "t", tokenFingerprint: () => "f", upsertThreadSummary: () => {},
    viewKey: (view, threadId) => (view === "thread" ? `thread:${threadId}` : view),
    visibleList: () => ({ children: [] }),
    windowStorage: () => ({ getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v), removeItem: (k) => store.delete(k) }),
    deepLinks: () => ({ applied: () => true }), sideColumnUp: () => false,
  });
  canon.messages.push({ id: "m1", thread: { id: "t1", is_root: false, root_message_id: "r1" } });
  assert.deepStrictEqual(owner.openingChannelView(), { view: "thread", thread: "t1", summary: null, origin: "main" },
    "the chosen thread did not reopen");
  recordState.chosen = { channelView: "thread", thread: { id: "gone", origin: "main", summary: null } };
  assert.deepStrictEqual(owner.openingChannelView(), { view: "flat", thread: null, summary: null, origin: "flat" },
    "a lost thread was reopened instead of the default");
  assert.equal(recordState.forgotten, 1, "the lost thread was not forgotten as it was passed over");
  assert.equal(owner.defaultChannelView(), "flat");
});

test("`#624 voice-page-modularity`: a reopen enters the saved view and pays its one read, from the module alone", () => {
  const { createOpeningView } = CHANNEL_VIEW;
  const record = [];
  const store = new Map();
  const state = { settled: false, owed: null, view: "voice", token: "t", freshAt: 5 };
  const canon = { channel: "c1", scope: null, messages: [], dismissals: [], dismissed: new Set(),
    views: new Map(), threads: [], hasThreads: false,
    unplaced: new Set(), unplacedSince: new Map(), unplacedAnswered: new Set(),
    removed: new Map(), complete: null, replyGaps: new Map(), settledByDone: new Map() };
  const owner = createOpeningView({
    uiState: () => ({
      chosenView: () => undefined, chosenViewsHeld: () => [], forgetChosen: () => {},
      setChosen: () => {}, remember: () => record.push("remember"), heldPlace: () => null,
      holdPlace: () => {}, settled: () => state.settled,
      settle: () => { state.settled = true; record.push("settle"); },
      saved: () => ({ channel: "c1", view: "discord", place: null }),
    }),
    providerFacts: { threading: () => false },
    reopenedViews: ["main", "flat", "thread"], uiStateKey: "k", uiStateVersion: 1, uiStateChars: 100000,
    channelStore: () => ({ current: () => canon }), threadPresence: () => ({ forget: () => {} }),
    columns: () => ({ main: null, side: null }), readModes: () => ({ hiding: () => false, mode: () => "show" }),
    screens: () => ({ screen: () => "main", view: () => state.view }), columnWork: () => ({ side: () => false }),
    messageCache: () => ({ storedExactly: (k, v) => store.set(k, v) }),
    captureScroll: () => ({ pinned: false, anchor: null, top: 0 }), channelContextKey: () => "c1",
    deepLinkFromAddress: () => null, deepLinkReaderMoved: () => false,
    el: () => ({ }), idsOf: () => [], inColumn: (_c, fn) => fn(), listPlaceTop: () => 0,
    loadCanon: () => {}, renderColumns: () => {}, restoreChannelComposer: () => {},
    restoreScroll: () => {}, scheduleBackSync: () => {}, selectedChannelId: () => "c1",
    selection: () => ({ view: "flat", thread: null, threadId: null, origin: "main" }),
    token: () => state.token, tokenFingerprint: () => "f", upsertThreadSummary: () => {},
    viewKey: (view, threadId) => (view === "thread" ? `thread:${threadId}` : view),
    visibleList: () => ({ children: [] }),
    windowStorage: () => ({ getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v), removeItem: (k) => store.delete(k) }),
    deepLinks: () => ({ applied: () => true }), sideColumnUp: () => false,
    documentsQueryValue: () => null, documentsDeepLinkKey: () => "key",
    showView: (name) => { state.view = name; record.push(`show=${name}`); },
    showDocuments: (key) => record.push(`docs=${key}`),
    reopenReadOwed: () => ({
      owe: (after) => { state.owed = { after }; record.push("owe"); },
      take: () => { const owed = state.owed; state.owed = null; return owed; },
      keep: (owed) => { state.owed = owed; record.push("keep"); },
    }),
    replayStarted: () => 7,
    readEnteredChannel: (fresh) => record.push(`read=${fresh}`),
    freshAt: () => state.freshAt,
    whenLiveAttached: (pay) => pay(),
  });
  owner.reopenSavedView();
  assert.deepStrictEqual(record, ["settle", "show=discord", "owe"],
    "the reopen did not enter and owe its read, in order");
  owner.payReopenRead();
  assert.deepStrictEqual(record.slice(3), ["read=true"], "the owed read was not paid once");
  owner.payReopenRead();
  assert.equal(record.length, 4, "the owed read was paid twice");
});

test("`#624 voice-page-modularity`: the card refresh notes a changed page build once, steps collapsed lines off fields, and re-reads for a landed summary", async () => {
  const { createCardRefresh } = CHANNEL_VIEW;
  const log = [];
  let buildSeen = "";
  const refresh = createCardRefresh({
    notePageBuild: (build) => { const changed = build !== buildSeen; buildSeen = build; return changed; },
    pageBuildChanged: () => log.push("pageBuildChanged"),
    view: () => "discord",
    selectedChannelId: () => 42,
    forgetThreadRead: (channel) => log.push(`forget ${channel}`),
    refreshThreadDirectory: async () => { log.push("refreshThreads"); },
    renderThreadHeading: () => log.push("heading"),
    renderSideColumn: () => log.push("side"),
    selectionView: () => "thread",
    renderReplyLinks: () => log.push("replyLinks"),
    isTextField: (target) => target === "field",
    collapsedCurrent: () => 3,
    collapsedChanged: (n) => { log.push(`collapsed ${n}`); return n; },
    setStatus: (text) => log.push(`status ${text}`),
  });
  refresh.noteServerPageBuild({ page_build: "abc" });
  refresh.noteServerPageBuild({ page_build: "abc" });
  refresh.noteServerPageBuild(null);
  assert.deepStrictEqual(log, ["pageBuildChanged", "pageBuildChanged"], "an unchanged or missing build was drawn, or a changed one was not");
  log.length = 0;
  assert.equal(refresh.collapsedLinesFromKeyboard(1, "field"), false);
  assert.equal(refresh.collapsedLinesFromKeyboard(1, "row"), true);
  assert.deepStrictEqual(log, ["collapsed 4", "status Collapsed messages show 4 lines."]);
  log.length = 0;
  await refresh.summaryWatchFired();
  assert.deepStrictEqual(log, ["forget 42", "refreshThreads", "heading", "side", "replyLinks"]);
});

test("`#624 voice-page-modularity`: the read state's acts — Done copies caught up, the dismissal undone, the backlog armed then cleared — from the module alone", async () => {
  const { createReadActs } = CHANNEL_VIEW;
  const log = [];
  const store = new Set(["a"]);
  const held = { dismissed: new Set(["b"]) };
  const doneArchived = { archived: new Set() };
  let offer = null;
  let armed = false;
  const owner = createReadActs({
    byId: () => ({ children: [] }), leftById: () => ({ children: [] }), inThisColumn: (fn) => fn,
    selection: () => ({ view: "flat", threadId: null, thread: null }), threads: () => [],
    readHiding: () => false, readRunsSuspended: () => false, threading: () => true,
    eachColumnRenderCached: () => {}, openReadRun: () => {}, peelReadRun: () => {}, readRunRows: () => [],
    threadIdOf: () => null, selectionThread: () => null,
    makeDismissalOffer: () => ({ current: () => offer, set: (next) => { offer = next; } }),
    makeInboxLedger: () => ({ leave: () => {} }),
    storeDismissed: () => store, storeChannel: () => "c1", selectedChannelId: () => "c1",
    heldDoneCopies: () => [held], sideShowing: () => false, activeColumn: () => "main",
    sideColumn: () => "side", mainColumn: () => "main", sideLeft: () => false,
    inColumn: (_column, act) => act(), doneState: () => doneArchived,
    cardsWhileDressedAfter: (_up, act) => act(), sideColumnUp: () => false,
    renderCachedTimeline: () => log.push("render"), syncUnreadReplies: () => log.push("counts"),
    renderSideColumn: () => log.push("side"),
    el: () => ({ children: [{ getAttribute: () => "false" }] }),
    idsOf: () => ["a"], readFilterKeeps: () => () => true,
    timelineMessages: () => [{ id: "a" }],
    renderTodoControls: () => log.push("controls"),
    restoreMessages: async (messages) => { log.push(["restore", ...messages]); },
    undoBookmarkBump: async () => { log.push("bump"); },
    listedMessages: () => [{ id: 1 }, { id: 2 }],
    backlogIsArmed: () => armed, armBacklog: () => { armed = true; log.push("arm"); },
    disarmBacklog: () => { armed = false; log.push("disarm"); },
    setStatus: (text) => log.push(text),
    dismissMessages: async (body) => { log.push(["dismiss", body.through, body.limit > 0]); },
    throughBump: () => "bump-of-last",
  });
  // caughtUpDone: the same set back where nothing differs, a new one where it does.
  const copy = new Set(["a"]);
  assert.equal(owner.caughtUpDone(copy, ["a"]), copy, "an unchanged copy was replaced");
  const wide = new Set(["b"]);
  const caught = owner.caughtUpDone(wide, ["a", "b"]);
  assert.notEqual(caught, wide);
  assert.deepStrictEqual([...caught], ["a"], "a copy was not brought into line with the store");
  // followDoneEverywhere: held copies are replaced into line with the store.
  owner.followDoneEverywhere(["a", "b"], "main");
  assert.deepStrictEqual([...held.dismissed], ["a"]);
  assert.equal(owner.rowsMissDone(), false, "rows drawn Done as their copy says counted as missing");
  // undoDismissal: nothing standing, nothing happens; standing, it restores exactly that set.
  await owner.undoDismissal();
  assert.deepStrictEqual(log, []);
  owner.lastDismissal.set({ messages: ["m1", "m2"], bump: null });
  await owner.undoDismissal();
  assert.deepStrictEqual(log, ["controls", ["restore", "m1", "m2"]]);
  assert.equal(owner.lastDismissal.current(), null);
  // clearBacklog: the first tap arms and says the count, the second clears through the newest.
  log.length = 0;
  await owner.clearBacklog();
  assert.deepStrictEqual(log, ["arm", "Tap again to clear 2 — undo will be offered afterwards."]);
  await owner.clearBacklog();
  assert.deepStrictEqual(log.slice(2), ["disarm", ["dismiss", "2", true]]);
});

test("`#624 voice-page-modularity`: one card highlighted — per channel, toggled by a press, put out by Esc — from the module alone", () => {
  const { createLitCards } = CHANNEL_VIEW;
  const el = (attrs = {}, children = []) => {
    const made = { attrs: new Map(Object.entries(attrs)), children, className: "", parentNode: null,
      hasAttribute: (name) => made.attrs.has(name), getAttribute: (name) => (made.attrs.has(name) ? made.attrs.get(name) : null),
      setAttribute: (name, value) => made.attrs.set(name, String(value)), removeAttribute: (name) => made.attrs.delete(name),
      getBoundingClientRect: () => ({ top: 10 }) };
    for (const child of children) child.parentNode = made;
    return made;
  };
  const button = el({}, []); button.className = "side-card";
  const cardA = el({ "data-id": "t1", "data-card": "" }, [button]);
  const cardB = el({ "data-id": "t2", "data-card": "" }, []);
  const sideCards = el({}, [cardA, cardB]);
  let channel = "c1";
  let cycle = false;
  const owner = createLitCards({
    cardsCycle: () => cycle, selectedChannelId: () => channel,
    leftById: (id) => (id === "side-cards" ? sideCards : el({}, [])),
    noteLinkedSelection: () => {}, rowShown: () => true,
    sidePane: () => ({ scroller: () => ({ scrollTop: 0 }) }), channelPane: () => ({ scroller: () => ({ scrollTop: 0 }) }),
    fitOverscroll: () => {}, bringIntoView: () => {}, noteOverscrollReader: () => {},
    nudge: (scroller, delta) => { scroller.scrollTop += delta; },
    sayRowShortcut: () => {}, openThread: () => "open", openThreadFromList: () => "list",
    selectSideThread: () => "side", markSideFromCards: () => {}, threadListShown: () => false,
    sideShowing: () => false, focusList: () => {}, columnLinksShown: () => false,
    sideMode: () => "list", channelOnScreen: () => true, sideColumnUp: () => true,
    inMainColumn: (act) => act(), selectionView: () => "flat", pinsOnly: () => false,
    paneFor: () => null, nodeWithin: () => false, keyRow: () => null, rowInView: () => false,
  });
  assert.equal(owner.litCard(), null, "a card started highlighted");
  owner.toggleLitCard("t1", cardA);
  assert.equal(owner.litCard(), "t1");
  assert.equal(cardA.getAttribute("data-highlighted"), "true", "the pressed card was not drawn lit");
  assert.equal(button.getAttribute("aria-expanded"), "true");
  // Another channel keeps its own highlight, or none.
  channel = "c2";
  assert.equal(owner.litCard(), null, "one channel's highlight showed in another");
  channel = "c1";
  // The same card pressed again puts it out.
  owner.toggleLitCard("t1", cardA);
  assert.equal(owner.litCard(), null, "the highlighted card pressed again stayed lit");
  assert.equal(cardA.hasAttribute("data-highlighted"), false);
  // Esc puts out the highlighted card on screen, and says whether there was one.
  owner.toggleLitCard("t1", cardA);
  assert.equal(owner.putOutLitCard(), true, "Esc found no highlighted card to put out");
  assert.equal(owner.litCard(), null, "Esc left the card lit");
  assert.equal(owner.putOutLitCard(), false, "Esc claimed a card where none was lit");
  // In the Threads view's cycle nothing highlights.
  cycle = true;
  owner.toggleLitCard("t2", cardB);
  assert.equal(owner.litCard(), null, "the Threads view highlighted a card");
});

test("`#624 voice-page-modularity`: the card at the top of a thread — drawn once per signature, folded by a press under its own kept key — from the module alone", () => {
  const { createThreadCardHead } = CHANNEL_VIEW;
  const was = globalThis.document;
  const log = [];
  const el = (attrs = {}, children = []) => {
    const made = { attrs: new Map(Object.entries(attrs)), children, className: "", parentNode: null, style: null,
      hidden: false, scrollTop: 0,
      hasAttribute: (name) => made.attrs.has(name), getAttribute: (name) => (made.attrs.has(name) ? made.attrs.get(name) : null),
      setAttribute: (name, value) => made.attrs.set(name, String(value)), removeAttribute: (name) => made.attrs.delete(name),
      append: (...kids) => made.children.push(...kids), getBoundingClientRect: () => ({ top: 0 }) };
    for (const child of children) child.parentNode = made;
    return made;
  };
  const card = el({ id: "thread-card" }); card.id = "thread-card"; card.style = { values: new Map(), getPropertyValue: (k) => card.style.values.get(k) || "", setProperty: (k, v) => card.style.values.set(k, v), removeProperty: (k) => card.style.values.delete(k) };
  const textButton = el({ id: "thread-card-text" }); textButton.id = "thread-card-text";
  const ageNode = el({ id: "thread-card-age" }); ageNode.id = "thread-card-age";
  const countNode = el({ id: "thread-card-count" }); countNode.id = "thread-card-count";
  countNode.replaceChildren = (...kids) => { countNode.children = kids; };
  const byId = { "thread-card": card, "thread-card-text": textButton, "thread-card-age": ageNode, "thread-card-count": countNode,
    "side-thread-card": el({}), "side-thread-card-text": el({}), "scroll-area": el({}), "side-scroll": el({}) };
  let folded = false;
  const choice = { id: "t1", root: { thread: {} }, at: 1000, started: 900 };
  const tally = { threads: new Map([["t1", { held: 2, unread: 1 }]]), records: new Map(), summaries: new Map(), byRoot: new Map() };
  globalThis.document = { createElement: () => el({}) };
  try {
    const owner = createThreadCardHead({
      makeFoldChoice: () => ({ on: () => folded, keep: (on) => log.push(["keep", on]), set: (on) => { folded = on; }, stored: () => folded }),
      foldKey: "vibe-talk.voice.thread-card-folded",
      leftById: (id) => byId[id] || el({}),
      unansweredRoot: () => false, threadReplyCount: () => 2, exactReplyCount: () => 2,
      cardView: () => ({ text: "About this", mode: "plain", cut: null }),
      briefAge: () => "3h", sideCardReplies: () => "2 replies", sideCardUnread: () => "1 unread",
      cardIsSummarized: () => false, watchPendingSummaries: () => {}, threadHue: () => 25,
      markCardLayout: () => {}, fillCardText: (words, _view, text) => { words.textContent = text; },
      setTextIfChanged: (node, text) => { node.textContent = text; }, fullLocalAt: () => "then",
      cardCountParts: () => ["parts"], scheduleColumnLinks: () => log.push("links"),
      drawnOnPage: () => false, atBottom: () => false, overscrollRoom: () => 0,
      fitOverscroll: () => {}, followFoot: () => {}, sideShowing: () => false,
      toPosition: (area, top) => { area.scrollTop = top; },
      sideThreadChoice: () => null, replyTally: () => tally,
    });
    assert.equal(owner.THREAD_CARD_FOLD_KEY, "vibe-talk.voice.thread-card-folded", "the fold's stored key changed");
    owner.paintThreadCard(card, choice, tally);
    assert.equal(card.getAttribute("data-thread"), "t1");
    assert.equal(card.style.values.get("--thread-hue"), "25");
    assert.deepStrictEqual(log, ["links"]);
    // The same signature draws nothing again.
    owner.paintThreadCard(card, choice, tally);
    assert.deepStrictEqual(log, ["links"], "an unchanged card was drawn again");
    // A press on the card folds it under the kept choice, and the bar says so.
    owner.threadCardPressed(card, { target: textButton });
    assert.deepStrictEqual(log.slice(1), [["keep", true], "links"]);
    assert.equal(card.hasAttribute("data-folded"), true, "the folded card was not drawn as a bar");
    assert.equal(textButton.getAttribute("aria-expanded"), "false");
    owner.setThreadCardFolded(false);
    assert.equal(card.hasAttribute("data-folded"), false, "the opened card stayed a bar");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: Main's messages among the cards and their order — kept choices named, rows filtered as Main's are — from the module alone", () => {
  const { createMainCards } = CHANNEL_VIEW;
  const log = [];
  const el = (attrs = {}) => {
    const made = { attrs: new Map(Object.entries(attrs)), children: [], className: "", parentNode: null, hidden: true, title: "",
      hasAttribute: (name) => made.attrs.has(name), getAttribute: (name) => (made.attrs.has(name) ? made.attrs.get(name) : null),
      setAttribute: (name, value) => made.attrs.set(name, String(value)), toggleAttribute: (name, on) => (on ? made.attrs.set(name, "") : made.attrs.delete(name)),
      append: () => {}, insertBefore: () => {},
      getBoundingClientRect: () => ({ top: 0, bottom: 10 }) };
    return made;
  };
  const mainButton = el(); const orderButton = el();
  const nodes = { "side-main": mainButton, "side-order": orderButton, "side-order-whole": el(), "side-order-short": el(), "side-scroll": el(),
    "channel-composer": el(), "side-list": el(), "pane-discord": el(), "outgoing-log": el(), "column-reply": el() };
  let mainOn = false;
  const kept = new Map();
  const storage = { getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, v), removeItem: (k) => kept.delete(k) };
  const messages = [
    { id: "m1", thread: null, timestamp: 1 }, { id: "m2", thread: { id: "t-of-m2", is_root: true }, timestamp: 2 }, { id: "m3", thread: null, timestamp: 3 },
  ];
  const owner = createMainCards({
    intoView: (element, block, inline) => { if (typeof element.scrollIntoView === "function") element.scrollIntoView(inline === undefined ? { block } : { block, inline }); },
    makeMainChoice: () => ({ on: () => mainOn, set: (on) => { mainOn = on; }, keep: (on) => log.push(["keep-main", on]), stored: () => mainOn }),
    threadsMainKey: "vibe-talk.voice.threads-main",
    storage: () => storage,
    leftById: (id) => nodes[id] || el(),
    threadListOnScreen: () => true, sideMode: () => "list", threadListShown: () => true,
    forgetCards: () => log.push("forget"), sideColumnUp: () => false,
    keepingCardPlace: (change) => change(), renderSideColumn: () => log.push("side"),
    setStatus: (text) => log.push(text), selectedCardId: () => null,
    paneRowForId: () => null, rowShown: () => false,
    storeChannel: () => "c1", storeMessages: () => messages, storeRemoved: () => new Set(),
    storeDismissed: () => new Set(["m3"]), selectedChannelId: () => "c1",
    inView: () => true, hiddenEmpty: () => false, hidingRead: () => true,
    stillToDo: (message) => message.id !== "m3", sentFromThisPage: () => false,
    whileMainRows: (act) => act(), glom: (rows) => rows, discordNode: () => el(),
    answeredBy: () => null, dressChannelRow: () => {},
  });
  assert.equal(owner.THREADS_MAIN_KEY, "vibe-talk.voice.threads-main", "the Main choice's stored key changed");
  assert.equal(owner.mainRowsOnScreen(), false, "Main's messages started among the cards");
  // Under Hide read, the read main message (m3, Done) is left out; the thread root carded is left out.
  assert.deepStrictEqual(owner.mainRowMessages(new Set(["t-of-m2"])).map((m) => m.id), ["m1"]);
  // Pressing Main channel keeps the choice and offers the cards again.
  owner.setMainRowsShown(true);
  assert.equal(owner.mainRowsOnScreen(), true);
  assert.equal(mainButton.getAttribute("aria-pressed"), "true");
  assert.deepStrictEqual(log, [["keep-main", true], "forget"]);
  // The order: the next after the one chosen, kept in the device's storage by its own key.
  const first = owner.threadOrders.current();
  owner.setThreadOrder(owner.nextThreadOrder());
  assert.notEqual(owner.threadOrders.current(), first, "the order chip did not move the order on");
  assert.equal(typeof kept.get("vibe-talk.voice.thread-order"), "string", "the order was not kept under its own key");
  assert.equal(owner.cardOrderOnScreen(), owner.threadOrders.current());
});

test("`#624 voice-page-modularity`: leaving the thread list and the card place — the held place put back, the cards opened where the reader was — from the module alone", () => {
  const { createCardPlaceActs } = CHANNEL_VIEW;
  const log = [];
  let anchor = { newest: false, id: "t1", offset: 12 };
  let heldPlace = null;
  const owner = createCardPlaceActs({
    inSideColumn: () => false, inMainColumn: (act) => act(),
    listBefore: () => ({ layout: "one", columns: null }), columnsFit: () => false,
    restoreBeforeColumns: () => {}, setLayout: () => {}, chooseOneColumn: (place) => log.push(["choose", place]),
    oneColumnBeforeList: () => "back-place", settleFocus: () => log.push("focus"),
    threadSummary: () => null, threadListOnScreen: () => false, cardFlightStart: () => null,
    flyThreadCard: () => {}, leftById: () => ({}),
    postCardShown: () => false, mainFollows: () => false, selectionKey: () => "flat|null",
    takeListPlace: () => heldPlace, setThreadList: (on) => log.push(["list", on]),
    renderMainToggle: () => log.push("main-toggle"), renderOrderToggle: () => log.push("order-toggle"),
    placeMainCard: () => log.push("main-card"), endRepliesWalk: () => log.push("end-walk"),
    placePostCard: () => log.push("post-card"), placeHolds: () => true,
    keepReaderPlace: () => { log.push("keep-place"); return true; },
    scrollToNewest: () => log.push("newest"), showOneColumnLeft: () => log.push("one-left"),
    followIfPinned: () => log.push("pinned"), renderSearchInAll: () => log.push("search"),
    refreshSearchCount: () => log.push("count"),
    makeCardPlaces: () => ({
      anchor: () => anchor,
      place: (place, follow) => { log.push(["place", place && place.id, follow]); return true; },
      keeping: (change) => change(),
    }),
    selectedChannelId: () => "c1", takeListCards: () => log.push("take-cards"),
    closeOverextend: () => log.push("close-room"), setSideListMode: () => log.push("mode-list"),
    whileOpeningCards: (act) => act(), renderSideMode: () => log.push("render-side"),
  });
  // Esc on the list in one column: back to the place it was chosen from, focus settled.
  owner.threadListBack();
  assert.deepStrictEqual(log, [["choose", "back-place"], "focus"]);
  // Leaving with no held place: the list goes, toggles redrawn, no place put back.
  log.length = 0;
  owner.leaveThreadList("one");
  assert.deepStrictEqual(log, [["list", false], "main-toggle", "order-toggle", "main-card", "end-walk", "post-card", "one-left", "search", "count"]);
  // A held place that still holds is put back in Main's column.
  heldPlace = { channel: "c1", view: "main", place: { pinned: false } };
  log.length = 0;
  owner.leaveThreadList("two");
  assert.ok(log.includes("keep-place"), "the held place was not put back");
  assert.ok(!log.includes("end-walk"), "the search walk ended while two columns keep the cards");
  // The card place remembered per channel is where the cards open again.
  log.length = 0;
  owner.rememberCardPlace();
  owner.openSideCards();
  assert.deepStrictEqual(log.slice(-3), ["mode-list", "render-side", ["place", "t1", true]]);
  // The anchor and keeping pass straight to the places owner; opening from the list chooses the thread.
  assert.deepStrictEqual(owner.cardAnchor(), { newest: false, id: "t1", offset: 12 });
  let kept = "";
  owner.keepingCardPlace(() => { kept = "changed"; });
  assert.equal(kept, "changed", "keeping did not run the change through the places owner");
  assert.equal(owner.placeCardAnchor(null), true);
  owner.openThreadFromList("t9", { id: "t9" });
  assert.deepStrictEqual(log.at(-1), ["choose", { view: "thread", thread: "t9", summary: { id: "t9" }, origin: "list" }]);
});

test("`#624 voice-page-modularity`: the right column's thread — selected, closed, read behind, and answered for — from the module alone", async () => {
  const { createSideThreadActs } = CHANNEL_VIEW;
  const log = [];
  const held = new Map();
  let mode = "list";
  let view = "flat";
  const el = () => ({ hidden: false, parentNode: null, replaceChildren: () => {}, getBoundingClientRect: () => ({ left: 1, top: 2 }) });
  const owner = createSideThreadActs({
    leftById: () => el(),
    makeFlight: () => ({ flyThreadCard: (...args) => log.push(["fly", args[1]]), endCardFlightPart: () => {}, endCardFlight: () => {} }),
    cardRowOnRight: () => null, rowShown: () => false, rowInView: () => false, sidePane: () => null,
    openSideCards: () => log.push("open-cards"), configApplied: () => false,
    guardQuietly: (run) => run, refreshThreadDirectory: async () => { log.push("directory"); },
    noteDeepLinkReaderMove: () => {}, selectedChannelId: () => "c1", columnsShown: () => true,
    threadSummary: () => null, sideThreads: () => held,
    sideMode: () => mode, setSideMode: (next) => { mode = next; }, setSideFromCards: () => {},
    replyDockedAway: () => false, replyInSide: () => false, putDockedReplyAway: () => log.push("dock-away"),
    renderSideMode: () => log.push("side-mode"), inSideColumn: (act) => act(),
    selectionView: () => view, selectionThreadId: () => "t1",
    rowsMissDone: () => false, renderCachedTimeline: () => {},
    changeChannelView: (...args) => { log.push(["change", ...args]); return "opened"; },
    endSearchWalk: () => log.push("end-search"), dropReplyArrowTap: () => {}, putRepliesBack: () => {},
    rememberChannelDraft: () => {}, rememberChannelContext: () => {}, bumpLoadGeneration: () => log.push("bump"),
    clearGathered: () => {}, forgetUnpackReads: () => {}, stopUnpackWatch: () => {},
    endRepliesWalk: () => {}, forgetRepliesNote: () => {}, clearSideSearchNote: () => {},
    rememberCardPlace: () => log.push("remember-place"), activeIsSide: () => false,
    resetSideRecords: () => log.push("reset-records"), forgetCards: () => {},
    sideColumnUp: () => true, threadListShown: () => false,
    loadSideDiscord: async (options) => { log.push(["load", options]); },
    channelContextKey: () => "ctx", activeColumn: () => "side-col",
    forwardCursor: () => null, sideShowing: () => true,
  });
  // Selecting draws it at once from what is held and remembers it for the channel.
  assert.equal(owner.selectSideThread("t1"), "opened");
  assert.equal(mode, "thread");
  assert.deepStrictEqual(held.get("c1"), { id: "t1", summary: null });
  assert.deepStrictEqual(log.filter((entry) => Array.isArray(entry) && entry[0] === "fly"), [["fly", null]]);
  // A read since answers for nothing yet; one sent after, in full, answers for a later refresh.
  assert.equal(owner.answeredSince(5, "poll"), false);
  owner.columnReadsSent.set("side-col", { context: "ctx", read: 9, full: true });
  assert.equal(owner.answeredSince(5, "poll"), true, "a fuller later read did not answer for this one");
  assert.equal(owner.answeredSince(9, "poll"), false, "the same read answered for itself");
  // Reading behind: the thread's read is sent, not the directory's.
  log.length = 0;
  await owner.readSideColumn("poll", null);
  assert.deepStrictEqual(log, [["load", { keepPosition: true, reason: "poll", readAs: "side" }]]);
  // Closing forgets the thread and shows the cards again.
  view = "thread";
  owner.closeSideThread();
  assert.equal(held.has("c1"), false);
  assert.ok(log.includes("open-cards"), "closing did not show the cards");
});

test("`#624 voice-page-modularity`: the cards filtered, their read runs folded, and the links asked after — from the module alone", () => {
  const { createCardRuns } = CHANNEL_VIEW;
  const log = [];
  const el = (attrs = {}, children = []) => {
    const made = { attrs: new Map(Object.entries(attrs)), children, className: "", parentNode: null, hidden: false, textContent: "",
      hasAttribute: (name) => made.attrs.has(name), getAttribute: (name) => (made.attrs.has(name) ? made.attrs.get(name) : null),
      setAttribute: (name, value) => made.attrs.set(name, String(value)), removeAttribute: (name) => made.attrs.delete(name) };
    for (const child of children) child.parentNode = made;
    return made;
  };
  const readCard = el({ "data-card": "", "data-id": "t1", "data-thread-read": "", "data-search": "read one" });
  const freshCard = el({ "data-card": "", "data-id": "t2", "data-search": "fresh two" });
  const list = el({}, [readCard, freshCard]);
  const stateNode = el({});
  let mode = "fold";
  const owner = createCardRuns({
    leftById: (id) => (id === "side-cards" ? list : stateNode),
    noteSuspendedRuns: () => false, unpackOnScreen: () => false, readMode: () => mode,
    readRunsSuspended: () => false, forgetCards: () => {}, renderSideCards: () => {},
    searchOpen: () => false, searchNode: () => null, searchTerms: () => [],
    threadsWithHits: () => null, openedCardGaps: () => null, hidingRead: () => false,
    rowMatches: () => true, markReplyMatch: () => {}, markSearchHits: () => {},
    olderThreadsFailure: () => "", allLoaded: () => true,
    setCardsCount: (cards, shown) => log.push(["count", cards, shown]),
    threadListOnScreen: () => false, sayCardCount: () => {}, renderRepliesSearch: () => {},
    drawCardGaps: () => {}, inOpenedGap: () => false,
    selectedChannelId: () => "c1", rowIsRead: () => false,
    readRunPlan: (items) => new Map([[items[0], { role: "head" }], [items[1], { role: "member" }]]),
    readRunLine: (row) => { const line = el({}); line.parentNode = row; ownerLine = line; return line; },
    placeInReadRun: (row, role) => log.push(["place", row.getAttribute("data-id"), role]),
    placeReadRunPeel: (row, count) => log.push(["peel", row.getAttribute("data-id"), count]),
    keepingCardPlace: (change) => change(),
    makeColumnLinksDeps: () => ({
      onTwoColumns: () => false, sideMode: () => "list", mainList: () => null,
      openThread: () => null, activePane: () => null, sidePane: () => ({}),
      litCard: () => null, cardRow: () => null, hue: () => 0,
    }),
  });
  let ownerLine = null;
  // No search, no Hide read: both cards stand, and the count says so.
  owner.filterSideCards();
  assert.deepStrictEqual(log, [["count", 2, 2]]);
  assert.equal(readCard.hidden, false);
  // Under Fold read the run's member is placed behind its head's line.
  log.length = 0;
  owner.foldCardRuns(list);
  assert.deepStrictEqual(log, [["place", "t1", "head"], ["place", "t2", "member"], ["peel", "t1", 2]],
    "the cards' run did not size its peel offer (the run under the peel minimum)");
  // Show read again: no places are given.
  mode = "show";
  owner.foldCardRuns(list);
  assert.deepStrictEqual(log.slice(3), [["place", "t1", null], ["place", "t2", null]]);
  // The list's word over no cards at all.
  assert.equal(owner.sideListState(0, 0, 0), "No threads in this channel yet.");
  assert.equal(owner.sideListState(2, 0, 0), "No threads match that search — all of this channel is loaded.");
});

test("`#624 voice-page-modularity`: the scroll trace's anchor, stored choice and sending, from the module alone", async () => {
  const { createScrollTraceControls } = CHANNEL_VIEW;
  const record = [];
  const row = { id: "r1", getBoundingClientRect: () => ({ top: 130 }) };
  const area = { getBoundingClientRect: () => ({ top: 100 }) };
  const box = { checked: false };
  const controls = createScrollTraceControls({
    scrollAnchor: () => row,
    idsOf: () => ["m9"],
    storedTrace: () => true,
    keepTrace: (on) => record.push(`keep=${on}`),
    el: () => box,
    scrollTrace: () => ({
      start: () => record.push("start"),
      stop: () => record.push("stop"),
      report: () => "{\"v\":1}",
    }),
    api: async (path, options) => record.push(`post ${path} ${options.method}`),
    setStatus: (text) => record.push(text),
  });
  assert.deepStrictEqual(controls.scrollTraceAnchor(area), { id: "m9", offset: 30 });
  assert.equal(controls.storedScrollTrace(), true);
  controls.applyScrollTrace(true);
  await controls.sendScrollTrace();
  assert.deepStrictEqual(record, ["keep=true", "start", "post /api/v1/scroll-trace POST", "Scroll trace sent."]);
  assert.equal(box.checked, true);
});

test("`#624 voice-page-modularity`: putting the list in a read mode — the mode set, kept, and drawn as the route says — from the module alone", async () => {
  const { createReadModeAct } = CHANNEL_VIEW;
  const log = [];
  let mode = "show";
  let hiding = false;
  let threading = true;
  const modes = {
    mode: () => mode, hiding: () => hiding,
    set: (next) => { mode = next; hiding = next === "hide"; log.push(`set ${next}`); },
    keep: () => log.push("keep-mode"),
  };
  const owner = createReadModeAct({
    makeReadModes: () => modes,
    keepingFilterPlace: (change) => change(),
    clearReadRuns: () => log.push("clear-runs"),
    settleUiState: () => log.push("settle"), saveUiState: () => log.push("save-ui"),
    disarmBacklog: () => log.push("disarm"),
    clearLastDismissal: () => log.push("clear-dismissal"),
    clearBacklogCount: () => log.push("clear-count"),
    renderTodoControls: () => log.push("controls"),
    threading: () => threading,
    eachColumnRenderCached: () => log.push("each-column"),
    forgetCards: () => log.push("forget-cards"), sideColumnUp: () => true,
    renderSideColumn: () => log.push("side"),
    loadDiscordEnter: async () => { log.push("load-enter"); },
    applyReadRuns: () => log.push("apply-runs"), renderScrollTools: () => log.push("tools"),
    guardQuietly: (run) => run, requestVisibleSummaries: () => log.push("summaries"),
  });
  // Not a mode at all: nothing happens.
  owner.setReadMode("bogus");
  assert.deepStrictEqual(log, [], "an unknown mode changed something");
  // Asking for Hide folds instead (#1168), and with threads both columns are drawn again.
  owner.setReadMode("hide");
  assert.equal(owner.readModes.mode(), "fold");
  assert.deepStrictEqual(log, ["set fold", "clear-runs", "settle", "save-ui", "keep-mode", "disarm",
    "clear-count", "controls", "each-column", "forget-cards", "side", "summaries"]);
  // Show again, the same route: no dismissal is cleared while hiding never changed.
  log.length = 0;
  owner.setReadMode("show");
  assert.deepStrictEqual(log, ["set show", "clear-runs", "settle", "save-ui", "keep-mode", "disarm",
    "clear-count", "controls", "each-column", "forget-cards", "side", "summaries"],
    "a change that never hid cleared the dismissal, or drew otherwise");
  // Without threads, leaving Hide read clears the dismissal and reads the other route, guarded;
  // and a change that hides nothing new draws the runs instead.
  const log2 = [];
  let mode2 = "hide";
  let hiding2 = true;
  const modes2 = {
    mode: () => mode2, hiding: () => hiding2,
    set: (next) => { mode2 = next; hiding2 = next === "hide"; log2.push(`set ${next}`); },
    keep: () => log2.push("keep-mode"),
  };
  const other = createReadModeAct({
    makeReadModes: () => modes2,
    keepingFilterPlace: (change) => change(),
    clearReadRuns: () => log2.push("clear-runs"),
    settleUiState: () => log2.push("settle"), saveUiState: () => log2.push("save-ui"),
    disarmBacklog: () => log2.push("disarm"),
    clearLastDismissal: () => log2.push("clear-dismissal"),
    clearBacklogCount: () => log2.push("clear-count"),
    renderTodoControls: () => log2.push("controls"),
    threading: () => false,
    eachColumnRenderCached: () => log2.push("each-column"),
    forgetCards: () => log2.push("forget-cards"), sideColumnUp: () => false,
    renderSideColumn: () => log2.push("side"),
    loadDiscordEnter: async () => { log2.push("load-enter"); },
    applyReadRuns: () => log2.push("apply-runs"), renderScrollTools: () => log2.push("tools"),
    guardQuietly: (run) => { log2.push("guarded"); return run; },
    requestVisibleSummaries: () => log2.push("summaries"),
  });
  other.setReadMode("show");
  assert.deepStrictEqual(log2, ["set show", "clear-runs", "settle", "save-ui", "keep-mode", "disarm",
    "clear-dismissal", "clear-count", "controls", "load-enter", "guarded", "summaries"],
    "leaving Hide read without threads did not clear the dismissal, read the other route guarded");
  log2.length = 0;
  other.setReadMode("fold");
  assert.deepStrictEqual(log2, ["set fold", "clear-runs", "settle", "save-ui", "keep-mode", "disarm",
    "clear-count", "controls", "apply-runs", "tools", "summaries"],
    "a change hiding nothing new did not draw the runs");
});

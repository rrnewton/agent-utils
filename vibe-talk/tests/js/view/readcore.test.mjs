// Tests for web/src/view/channel/readcore.ts on its own: the timeline view built as the page
// build builds it and held directly, with fake ports and stand-in nodes (`#624
// voice-page-modularity`, mu-readcore). The page's behaviour over the same module, on the
// fixture's DOM, is tested in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The channel's timeline as the page reads and draws it (web/src/view/channel/readcore.ts). */
const READCORE = await moduleForTests("src/view/channel/index.ts");

/** Minimal fake ports: every page hand the view asks for, recorded where it matters. */
function fakeDeps(overrides = {}) {
  const calls = [];
  const canon = { channel: "c", messages: [], threads: [], views: new Map(), dismissed: new Set(), hasThreads: false, scope: null };
  const deps = {
    el: (id) => ({ id, children: [], replaceChildren: () => {}, textContent: "", hidden: false }),
    selection: () => ({ view: "main", threadId: null, thread: null }),
    timelineRows: () => ({ messages: [], threads: [] }),
    olderPages: () => ({ cursor: null, moreAbove: undefined, inFlight: false }),
    loadState: () => ({ generation: 0, freshAt: 0, freshness: "fresh" }),
    listState: () => ({ newestId: null, backlog: 0 }),
    entryState: () => ({ threadOwed: null, threadAwaitsRead: false, readOwed: null }),
    doneState: () => ({ archived: new Set() }),
    selectedChannelId: () => "c",
    channelContextKey: () => "ctx",
    channelStore: { current: () => canon, open: () => {} },
    providerFacts: { threading: () => true },
    messageCache: { readMessageCache: () => null, writeMessageCache: () => {}, forgetChannelScopes: () => {} },
    restoreSavedBookmark: () => {},
    noteServerThreadHuesIn: () => {},
    knownChannel: (channel) => channel,
    viewKey: (view = "main", thread = null) => (view === "thread" ? `thread:${thread}` : view),
    reader: {
      readTimeline: async () => ({ messages: [], threads: [], dismissed: [] }),
      timelinePath: () => "/timeline",
      readReasonOf: () => null,
      olderReadReason: () => "older",
      forwardCursor: () => null,
      joinedDeltas: (pages) => pages[pages.length - 1],
      landedAnswer: () => null,
      staleAsOf: () => null,
    },
    timelineDeltaPages: 4,
    isCursorRefusal: () => false,
    abandonedReads: { held: () => false, holdKey: () => "k", abandon: () => {} },
    channelReads: { claim: () => true, cursorOwed: () => false, oweCursor: () => {}, queued: () => false, payCursor: () => {} },
    queueDiscordLoad: () => {},
    loadDiscord: async () => {},
    finishDiscordLoad: async () => {},
    replayLedger: { start: () => 0, mutationAwaits: () => false },
    columnReadsSent: new Map(),
    landedReadPosition: () => ({ wasAtNewest: true, keepPosition: false }),
    landAbandonedTimelineRead: () => {},
    queuePlaceUnplaced: () => {},
    prepareSpeech: () => {},
    guardQuietly: (fn) => fn,
    settleReplays: () => {},
    setStatus: () => {},
    liveSeen: () => 0,
    liveNews: () => 0,
    readsAsked: () => 0,
    readOrderNow: () => 0,
    sentNumberOf: () => 0,
    refreshFailed: () => false,
    within: async (_ms, request) => request(undefined),
    clock: { setTimeout: () => 0, clearTimeout: () => {} },
    discordNode: () => ({}),
    threadCards: () => [],
    glom: (messages) => messages.map((m) => [m]),
    idsOf: () => [],
    rowMessages: () => [],
    hiddenEmpty: () => false,
    hidingRead: () => false,
    readFilterKeeps: () => () => true,
    gatheredParent: () => null,
    observeOutgoingMessages: () => {},
    noteArchived: () => {},
    noteFreshRead: () => {},
    noteChannelReadFailure: () => {},
    renderChannelLoading: () => {},
    backgroundRead: () => false,
    renderChannelNavigation: () => {},
    renderChannelSeam: () => {},
    renderOlderControl: () => {},
    renderScrollTools: () => {},
    renderTodoControls: () => {},
    renderChannelRows: () => {},
    setChannelFreshness: () => {},
    renderOutgoingMessages: () => {},
    channelSummary: () => "",
    channelName: () => "",
    clearChannelScreen: () => {},
    holdingReplyArrows: (draw) => draw(),
    holdingScrollTools: (draw) => draw(),
    channelReadPosition: () => ({ wasAtNewest: true, anchorId: null, anchorTop: 0, around: null }),
    renderChannelFreshness: () => {},
    preservingScroll: (mutate) => mutate(),
    columnWork: { active: () => ({}) },
    settleAfterRead: () => {},
    catchUpHeldView: () => false,
    catchUpFromAll: () => {},
    syncUnreadReplies: () => {},
    followInOtherColumn: () => {},
    keepEntryLanding: () => true,
    entryLandingMark: () => null,
    payThreadEntry: () => {},
    saveChannelScope: () => {},
    requestVisibleSummaries: () => {},
    seamsAfterMidnight: () => {},
    applySearch: () => {},
    readRunsSuspended: () => false,
    columnSteps: () => ({ column: {}, pause: () => {}, resume: () => {} }),
    inColumn: (_column, fn) => fn(),
    inOtherColumn: (fn) => fn(),
    replyTally: () => ({ threads: new Map() }),
    unseenReplies: () => 0,
    followPlacement: () => {},
    unplacedCount: () => 0,
    unplacedNext: () => 0,
    heldUnansweredRoot: () => null,
    copyLedger: {},
    removalRecord: {},
    removedAfterRead: () => false,
    ownPostStands: () => false,
    foldReadState: () => true,
    noteReplyGaps: () => {},
    noteServerPageBuild: () => {},
    renderColumns: () => {},
    threadPresenceNote: () => {},
    syncCanonDismissed: () => {},
    olderTriggerPx: 80,
    threadPresence: { note: () => {} },
    openThreadName: () => "",
    captureScroll: () => ({ pinned: true, anchor: null, top: 0 }),
    restoreScroll: () => {},
    nudge: (area, delta) => { area.scrollTop += delta; },
    toPosition: (area, top) => { area.scrollTop = top; },
    heldRowAfter: () => null,
    screensView: () => "discord",
    renderJumpNewest: () => {},
    ...overrides,
  };
  return { deps, calls, canon };
}

test("`#624 voice-page-modularity`: the timeline view refuses work after dispose, from the module alone", async () => {
  const { createTimelineView } = READCORE;
  const { deps } = fakeDeps();
  const view = createTimelineView(deps);
  view.dispose();
  view.dispose();
  await view.loadTimeline({ reason: "poll" });
  await view.loadOlderTimeline();
});

test("`#624 voice-page-modularity`: a view no page has covered draws nothing held, from the module alone", () => {
  const { createTimelineView } = READCORE;
  const { deps } = fakeDeps();
  const view = createTimelineView(deps);
  assert.deepStrictEqual(view.heldViewRows(), []);
  assert.equal(view.walkCursorOwed(), false);
  assert.equal(view.drawProjection(), false);
});

test("`#624 voice-page-modularity`: the older walks mount and dispose without a page, from the module alone", () => {
  const { mountOlderWalks } = READCORE;
  const column = { searchWalk: null, searchNote: null, searched: null };
  const walks = mountOlderWalks({
    ...fakeDeps().deps,
    leftEl: (id) => ({ id, children: [], replaceChildren: () => {}, textContent: "", hidden: false, disabled: false }),
    searchField: { open: () => true, query: () => "needle", links: () => false },
    readModes: { hiding: () => false, mode: () => "show" },
    searchIdentity: (q) => q,
    searchTyped: () => true,
    filterKey: () => "filter",
    linkKindsHidden: () => [],
    authorChoosing: () => "everyone",
    pinsOnly: () => false,
    screensView: () => "discord",
    screensScreen: () => "main",
    rowDay: () => 0,
    threadPresenceAny: () => false,
    endFilterWalk: () => {},
    readerActed: () => {},
    pullFingerDown: () => false,
    sideCardsShown: () => false,
    layoutColumnsShown: () => false,
    lastPaneCurrent: () => null,
    sidePaneValue: null,
    serverSearchOwner: { state: () => ({ key: "", ticket: 0, loading: false, failed: "", hits: [], scopes: [], examined: 0, resume: null }), allChannels: () => false, schedule: () => {}, chooseAllChannels: () => {}, run: async () => {}, more: async () => {}, forget: () => {} },
    serverFactsTokenScope: () => "write",
    searchFurtherRunning: () => false,
    serverCopyOpen: () => true,
    gathering: { stop: () => {}, start: async () => {}, state: () => ({ channel: "", failed: "", state: null }) },
    repliesWalks: { current: () => null, search: () => {}, end: () => {}, holds: () => false, noteFor: () => "" },
    historyReadToStart: () => false,
    api: async () => ({}),
    channelName: () => "chan",
    channelDirectoryChannels: () => [],
    jumpToMessageLink: () => {},
    messageBody: () => ({}),
    markSearchHits: () => {},
    searchParsed: () => ({ terms: [] }),
    heldMessagesById: () => new Map(),
    keepingCardPlace: (change) => change(),
    columnWork: { active: () => column, side: () => false },
    columns: { main: column, side: { ...column, mode: "list" } },
    stillToDo: () => true,
    viewKey: (view = "main", thread = null) => (view === "thread" ? `thread:${thread}` : view),
    walkCursorOwed: () => false,
    loadOlder: async () => {},
  });
  assert.equal(typeof walks.searchKey(), "string");
  assert.equal(walks.searchOffered(), false);
  walks.dispose();
  walks.dispose();
});

// `#1985 vibe-talk-windowed-all-view-fixes`: the windowed foot lock and the
// anti-oscillation guard, on a simulated layout (row 100px, estimate 84px, so
// estimated and real heights disagree as they do on the owner's list).
class FakeEl {
  constructor(tag = "li") {
    this.tagName = tag.toUpperCase(); this.children = []; this.parentNode = null;
    this.dataset = {}; this.style = {}; this.attrs = new Map(); this.ownerDocument = null;
    this.__ids = []; this.__messages = []; this.__h = 100; this.hidden = false;
  }
  setAttribute(k, v) { this.attrs.set(k, String(v)); } getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); } removeAttribute(k) { this.attrs.delete(k); }
  contains(node) { return node === this || this.children.includes(node); }
  append(...nodes) { for (const n of nodes) this.insertBefore(n, null); }
  insertBefore(node, before) {
    if (node.parentNode) node.parentNode.removeChild(node);
    const at = before ? this.children.indexOf(before) : -1;
    if (at < 0) this.children.push(node); else this.children.splice(at, 0, node);
    node.parentNode = this; return node;
  }
  removeChild(node) { const at = this.children.indexOf(node); if (at >= 0) this.children.splice(at, 1); node.parentNode = null; return node; }
  replaceChildren(...nodes) { for (const c of [...this.children]) this.removeChild(c); this.append(...nodes); }
  get offsetHeight() { return this.dataset.windowSpacer ? parseFloat(this.style.height || "0") || 0 : this.__h; }
  getBoundingClientRect() {
    const list = this.__list; const area = this.__area;
    if (!list || !area) return { top: 0, bottom: this.offsetHeight, height: this.offsetHeight };
    let top = -area.scrollTop;
    for (const child of list.children) { if (child === this) break; top += child.offsetHeight; }
    return { top, bottom: top + this.offsetHeight, height: this.offsetHeight };
  }
}

test("`#1985`: a windowed render at the foot ends at the last group and lands on the bottom row", () => {
  const { createTimelineView } = READCORE;
  const messages = Array.from({ length: 50 }, (_, i) => ({ id: String(i + 1), content: "x", timestamp: `2026-10-01T00:00:${String(i % 60).padStart(2, "0")}Z` }));
  const area = new FakeEl("div"); area.clientHeight = 500; area.clientWidth = 400;
  let scrollTopHeld = 0;
  Object.defineProperty(area, "scrollHeight", { get() { return list.children.reduce((sum, c) => sum + c.offsetHeight, 0); } });
  // As a browser does: a scroll write is clamped to the foot of what stands.
  Object.defineProperty(area, "scrollTop", {
    get() { return scrollTopHeld; },
    set(value) { scrollTopHeld = Math.max(0, Math.min(Number(value) || 0, Math.max(0, area.scrollHeight - area.clientHeight))); },
  });
  area.getBoundingClientRect = () => ({ top: 0, bottom: 500, height: 500 });
  area.addEventListener = () => {};
  const list = new FakeEl("ul"); list.id = "discord-log";
  const doc = { activeElement: null, createElement: (tag) => { const el = new FakeEl(tag); el.ownerDocument = doc; el.__list = list; el.__area = area; return el; } };
  list.ownerDocument = doc;
  const rows = { messages, threads: [] };
  const { deps, canon } = fakeDeps({
    el: (id) => (id === "scroll-area" ? area : id === "discord-log" ? list : new FakeEl("div")),
    selection: () => ({ view: "flat", threadId: null, thread: null }),
    timelineRows: () => rows,
    windowed: () => ({ prefetchPx: 100, rowCap: 5 }),
    heldFilterRow: () => null,
    jumpAnchorRow: () => null,
    discordNode: (group) => { const el = doc.createElement("li"); el.__ids = group.map((m) => String(m.id)); el.__messages = group; el.__list = list; el.__area = area; return el; },
    idsOf: (row) => row.__ids || [],
    rowMessages: (row) => row.__messages || [],
    captureScroll: () => ({ pinned: true, anchor: null, top: 0 }),
    scrollAnchor: () => null,
    followsNewest: () => true,
    heldRowAfter: () => null,
    guardQuietly: (fn) => fn,
  });
  canon.messages = messages;
  const view = createTimelineView(deps);
  view.renderCachedTimeline();
  const mounted = () => list.children.filter((c) => !c.dataset.windowSpacer);
  assert.ok(mounted().length <= 5, `mounted ${mounted().length} rows over the cap`);
  // Stand at the foot as END/Newest leave the reader, then render again: the
  // window must end at the last group and the reader on the bottom row.
  area.scrollTop = area.scrollHeight;
  view.renderCachedTimeline();
  const last = mounted()[mounted().length - 1];
  assert.equal(last.__ids[0], "50", "the window at the foot does not end at the newest row");
  assert.ok(Math.abs(area.scrollTop + area.clientHeight - area.scrollHeight) <= 1,
    `at the foot the reader stands at ${area.scrollTop}+${area.clientHeight} of ${area.scrollHeight}, not on the bottom row`);
  view.dispose();
});


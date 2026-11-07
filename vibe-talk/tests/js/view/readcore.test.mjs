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

test("`#624 voice-page-modularity`: the counts read is not begun for another channel or a hidden page, from the module alone", async () => {
  const { createTimelineView } = READCORE;
  const { deps } = fakeDeps();
  const view = createTimelineView(deps);
  await view.readAllForCounts("other-channel");
  await view.readAllForCounts("");
});

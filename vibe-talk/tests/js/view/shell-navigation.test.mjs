// Tests for web/src/view/shell/navigation.ts on its own: the module built as the page build builds
// it and held directly, with stand-in page state (`#624 voice-page-modularity`). The page's behaviour
// over the same factory, on the fixture's DOM, is tested in ../page/navigation.mjs and
// ../page/links.mjs. ADD this file beside the other view suites and import nothing: the runner
// collects tests/js/view/*.test.mjs by directory (check the lane's runner before landing).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");
const NAVCORE = await moduleForTests("src/core/navigation/index.ts");

/** A page stand-in: every dep a thunk over `state`, which a test moves under the factory. */
function makeNavigation(state) {
  const calls = [];
  const elements = new Map();
  const el = (id) => {
    if (!elements.has(id)) {
      const element = {
        id, scrollTop: 0, textContent: "", title: "", attrs: {},
        focus() { calls.push(["focus", id]); }, replaceChildren: () => calls.push(["clear", id]),
        setAttribute(name, value) { this.attrs[name] = value; },
        getAttribute(name) { return this.attrs[name] ?? null; },
      };
      Object.defineProperty(element, "hidden", {
        get: () => state.hidden.has(id),
        set: (value) => { if (value) state.hidden.add(id); else state.hidden.delete(id); },
      });
      elements.set(id, element);
    }
    return elements.get(id);
  };
  const nav = SHELL.createPageNavigation({
    createBackStack: NAVCORE.createBackStack, createVisits: NAVCORE.createVisits,
    createDeepLinks: NAVCORE.createDeepLinks, createLinkJumps: NAVCORE.createLinkJumps,
    visitKey: NAVCORE.visitKey, deepLinkQuery: NAVCORE.deepLinkQuery,
    clock: { setTimeout: (run, ms) => { state.timers.push({ run, ms }); return state.timers.length; }, clearTimeout: () => {} },
    session: "test-session",
    screens: () => ({
      screen: () => state.screen, beforeSettings: () => "main", helpReturn: () => "settings",
      setHelpReturn: () => {}, view: () => state.view,
      setView: (name) => { calls.push(["showView", name]); state.view = name; },
    }),
    el, leftEl: el,
    inSide: () => false, mainColumn: () => "main", inColumn: (_c, act) => act(), sideColumn: () => "side",
    sideMode: () => state.sideMode, sideThreadId: () => state.sideThread,
    selectedChannelId: () => state.channel, selection: () => state.selection,
    layoutColumns: () => state.layout === "two", layoutThreadList: () => state.layout === "list",
    layoutFit: () => true, listBefore: () => ({ layout: "one" }), sideThreadsHas: () => false,
    replyDocked: () => state.replyDocked,
    rowMenuOpen: () => null, promptsOpen: () => false, actionMenuOpen: () => false,
    closeSpeedPopover: () => { calls.push("closeSpeed"); state.hidden.add("speed-popover"); },
    setPromptsOpen: () => {}, setActionMenuOpen: () => {},
    closeDismissedPopout: () => false, focusList: () => {}, sidePane: () => "side", focusLost: () => false,
    closeSideThread: () => {}, closeHelp: () => calls.push("closeHelp"), closeSettings: () => calls.push("closeSettings"),
    closeReply: () => calls.push("closeReply"), threadListBack: () => calls.push("listBack"),
    // The Documents view as the page wires it (`#1082 documents-back-chain`): an
    // open chain `state.docs` (keys, current last); opening over one pushes,
    // closing pops one step. `documentDepth` omitted where a test says unwired.
    documentOpen: () => state.docs.length > 0,
    ...(state.noDocDepth ? {} : { documentDepth: () => state.docs.length }),
    closeDocument: () => { calls.push("closeDocument"); state.docs.pop(); },
    documentOutlineOpen: () => state.outline === true,
    closeDocumentOutline: () => { calls.push("closeOutline"); const was = state.outline === true; state.outline = false; return was; },
    openDocument: (key) => { calls.push(["openDocument", key]); state.view = "docs"; state.docs.push(key); },
    leaveThread: () => calls.push("leaveThread"), showScreen: () => {}, showHelp: () => {},
    helpEntries: () => [], threadListOffered: () => true, threadStillKnown: () => true, threading: () => true,
    chooseView: (which) => calls.push(["chooseView", which]), selectSideThread: () => {},
    openThreadFromList: () => {}, chooseOneColumn: (choice) => calls.push(["oneColumn", choice.view]),
    defaultChannelView: () => "main", threadsOfChannel: () => [], threadIdOf: (thread) => thread.id,
    token: () => "tok", channels: () => [{ id: "C1" }], chooseChannel: () => {},
    viewKey: () => "main", viewsHas: () => true, readsBusy: () => false, readsStarted: () => 1,
    seekMessage: async () => {}, rowHolding: () => null, answeredElsewhere: () => false,
    providerNameOf: () => "discord", providerName: () => "discord", threadOf: () => null,
    setStatus: (text) => calls.push(["status", text]), showError: (text) => calls.push(["error", text]),
    guardQuietly: (run) => (...args) => run(...args), settleFocus: () => {}, replyOnScreen: () => false,
    setLinkedMark: () => {}, cardsCycle: () => false, rowShown: () => true, visibleList: () => ({ children: [] }),
    rowOnScreen: () => true, eachColumn: (act) => act(), openReadRun: () => {}, setSearchOpen: () => {},
    finishPackSlide: () => {}, headOfList: () => 0, renderScrollTools: () => {}, revealRow: () => {},
    toPosition: (area, top) => { area.scrollTop = top; },
    idsOf: () => [], inAppLinksOn: () => true, channelLabel: (channel) => channel, renderedText: () => "x",
    inThisColumn: (act) => act, captureScroll: () => ({ pinned: true, anchor: null, top: 0 }),
    listPlaceTop: () => 0, sideShowing: () => false,
    restoreScroll: (place) => calls.push(["restore", place]), scrollToNewest: () => {},
    bumpReplyJump: () => {}, settleUiState: () => {},
    viewRestored: () => false, stopDiscordPolling: () => {}, storedOpenInView: () => "flat",
    cycleView: () => calls.push("cycleView"),
    loadDiscord: async (options) => calls.push(["loadDiscord", options]),
    readSideColumn: async (reason, since) => calls.push(["readSide", reason, since]),
    scheduleDiscordPoll: () => calls.push("schedulePoll"),
    putDockedReplyAway: () => {}, gatherState: () => state.gather, listState: () => state.list,
    dropReplyArrowTap: () => {}, applyChannelProvider: () => {}, rememberActiveChannel: () => {},
    rememberChannelDraft: () => {}, openChannelView: () => {}, forgetThreadPresence: () => {},
    timelineRows: () => state.rows, clearChannelContexts: () => {}, restoreThreadHues: () => {},
    seedServerThreadHues: async () => {}, restoreChannelComposer: () => {}, olderPages: () => state.older,
    payReadCursor: () => {}, renderOlderControl: () => {}, loadState: () => state.load,
    hydrateChannelScope: () => false, renderChannelFreshness: () => {}, saveUiState: () => {},
    pinsOnly: () => false, ensurePins: () => {}, renderChannelRows: () => {},
    resetSideColumn: () => {}, renderColumns: () => {}, startChannelStream: () => {},
    viewScroll: () => ({ leave: () => {}, enter: () => state.viewTop }), atBottom: () => false,
    viewMark: () => state.viewMark, setViewMark: () => {}, holdChannelPlace: () => {},
    placePostCard: () => {}, syncDocumentsSelector: () => {}, syncDiscussView: () => {},
    forgetJumpNewest: () => {}, dismissStatus: () => {},
    readAloudOwner: () => ({ nowPlaying: null, pendingRead: null, readingMode: false, stopReading: () => calls.push("stopReading") }),
    renderCallControls: () => {}, renderControlBar: () => {}, renderChannelNavigation: () => {},
    placeChipsOverComposer: () => {},
    writeClipboardText: async () => true, copiedRefusedText: () => "refused", pullWaitMs: () => 0,
    delay: async () => {}, userActivation: () => null, locationSearch: () => state.search,
    locationOrigin: () => "https://example.test", locationPathname: () => "/",
    navigationIndex: () => 0, historyPort: () => state.history, layoutRestoring: () => false,
  });
  return { nav, calls, elements };
}

const freshState = () => ({
  screen: "main", view: "discord", channel: "C1", layout: "one", sideMode: "cards", sideThread: null,
  docs: [], noDocDepth: false,
  selection: { view: "main", threadId: null, origin: null }, replyDocked: false,
  gather: { gathered: { parent: "m1" } }, list: { rowsGathered: true, newestId: "m9" },
  rows: { messages: [{ id: "m1" }], threads: [{ id: "t1" }] }, older: { moreAbove: true, cursor: "c" },
  load: { freshAt: 9, freshness: "stale" }, viewTop: null, viewMark: undefined,
  hidden: new Set(["speed-popover", "dismissed-send-popout"]), timers: [], search: "",
  history: { state: null, replaced: [], replaceState(state, _title, url) { this.replaced.push(url); } },
});

test("`#624` shell navigation: backLevels puts the thread, then a docked reply, first-opened first", () => {
  const state = freshState();
  state.selection = { view: "thread", threadId: "T1", origin: "main" };
  state.replyDocked = true;
  const { nav } = makeNavigation(state);
  assert.deepStrictEqual(nav.backLevels(), ["thread", "reply"]);
  assert.deepStrictEqual(nav.channelPlace(), {
    screen: "main", channel: "C1", layout: "one", view: "thread", thread: "T1", origin: "main", side: null,
  });
});

test("`#624` shell navigation: with nothing noted, Ctrl/Cmd+[ closes the top level as Esc does", () => {
  const state = freshState();
  state.selection = { view: "thread", threadId: "T1", origin: "main" };
  const { nav, calls } = makeNavigation(state);
  assert.equal(nav.placeFromKeyboard(-1), true);
  assert.deepStrictEqual(calls.filter((call) => call === "leaveThread"), ["leaveThread"]);
});

test("`#1082 documents-back-chain`: the chain's depth is its levels, menus on top; unwired, one document is one level", () => {
  const state = freshState();
  state.view = "docs";
  state.docs = ["doc-a", "doc-b"];
  state.hidden.delete("speed-popover");
  const { nav, calls } = makeNavigation(state);
  assert.deepStrictEqual(nav.backLevels(), ["document", "document", "menu:pace"],
    "two open documents are not two levels, or a menu is not on top of them");
  nav.documentBack();
  assert.deepStrictEqual(calls.filter((call) => call === "closeSpeed"), ["closeSpeed"],
    "Back did not close the menu first");
  assert.deepStrictEqual(state.docs, ["doc-a", "doc-b"], "closing the menu popped a document too");
  const unwired = freshState();
  unwired.view = "docs";
  unwired.docs = ["doc-a", "doc-b"];
  unwired.noDocDepth = true;
  assert.deepStrictEqual(makeNavigation(unwired).nav.backLevels(), ["document"],
    "without the depth wired, an open document is not its one old level");
});

test("`#1038 documents-outline-popup`: the open outline is a menu level on top, and Back closes it before the document", () => {
  const state = freshState();
  state.view = "docs";
  state.docs = ["doc-a"];
  state.outline = true;
  const { nav, calls } = makeNavigation(state);
  assert.deepStrictEqual(nav.backLevels(), ["document", "menu:outline"],
    "the open outline is not a level on top of its document");
  nav.documentBack();
  assert.deepStrictEqual(calls.filter((call) => call === "closeOutline"), ["closeOutline"],
    "Back did not close the outline first");
  assert.deepStrictEqual(state.docs, ["doc-a"], "closing the outline popped a document too");
  assert.equal(state.outline, false, "the outline stayed open after Back");
});

test("`#1082 documents-back-chain`: channel -> document is a jump, a second document stacks over it, and Back walks both steps home", async () => {
  const state = freshState();
  const { nav, calls } = makeNavigation(state);
  nav.jumpToDocumentLink("doc-a");
  assert.equal(state.view, "docs", "the jump did not open the document");
  assert.equal(nav.linkJumps.top()?.doc, "doc-a", "the jump does not carry its document");
  assert.deepStrictEqual(nav.backLevels(), ["jump"], "the document is a level beside its jump, not the jump's own");
  // A second document opened through the view's open path: one more level.
  state.docs.push("doc-b");
  assert.deepStrictEqual(nav.backLevels(), ["jump", "document"]);
  // Back (the button takes documentBack, the gesture closes the same levels):
  // the document step first, still reading A, still in Documents.
  nav.documentBack();
  assert.deepStrictEqual(state.docs, ["doc-a"], "the first Back did not return to the first document");
  assert.equal(state.view, "docs");
  assert.deepStrictEqual(nav.backLevels(), ["jump"]);
  // Back again: the jump, to the channel the reader came from.
  nav.documentBack();
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
  assert.equal(state.view, "discord", "the second Back did not return to the channel's view");
  assert.deepStrictEqual(state.docs, [], "returning to the channel left a document open");
  assert.equal(nav.linkJumps.top(), undefined, "the returned jump is still taken");
  assert.deepStrictEqual(nav.backLevels(), []);
  assert.ok(calls.some((call) => Array.isArray(call) && call[0] === "status" && call[1] === "Back where you were."),
    "the return did not put the reader back on their spot");
});

test("`#1082 documents-back-chain`: list -> document -> Back is the list's own step, no jump taken", () => {
  const state = freshState();
  state.view = "docs";
  state.docs = ["doc-a"];
  const { nav, calls } = makeNavigation(state);
  assert.deepStrictEqual(nav.backLevels(), ["document"]);
  nav.documentBack();
  assert.deepStrictEqual(calls.filter((call) => call === "closeDocument"), ["closeDocument"]);
  assert.deepStrictEqual(state.docs, [], "Back from a list-opened document did not reach the list");
  assert.equal(nav.linkJumps.top(), undefined, "a list open was taken for a jump");
});

test("`#624` shell navigation: syncDeepLinkAddress writes the standing place's query once", () => {
  const state = freshState();
  const { nav } = makeNavigation(state);
  nav.syncDeepLinkAddress();
  assert.deepStrictEqual(state.history.replaced, ["/?c=C1&v=main"]);
  state.search = "?c=C1&v=main";
  nav.syncDeepLinkAddress();
  assert.equal(state.history.replaced.length, 1, "an address already saying this is not written again");
});

test("`#624` shell navigation: changing channel clears the old channel's records and reads the new one", async () => {
  const state = freshState();
  const { nav, calls } = makeNavigation(state);
  await nav.changeSelectedChannel();
  assert.deepStrictEqual(state.rows, { messages: [], threads: [] });
  assert.deepStrictEqual(state.older, { moreAbove: false, cursor: null });
  assert.deepStrictEqual(state.load, { freshAt: 0, freshness: "fresh" });
  assert.equal(state.gather.gathered, null);
  assert.deepStrictEqual(calls.filter((call) => Array.isArray(call) && call[0] === "loadDiscord"),
    [["loadDiscord", { reason: "enter" }]]);
  calls.length = 0;
  await nav.readEnteredChannel(true);
  assert.deepStrictEqual(calls, [
    ["loadDiscord", { keepPosition: true, reason: "visible" }],
    ["readSide", "visible", 1],
    "schedulePoll",
  ]);
  nav.pressViewSwitch();
  assert.equal(calls.at(-1), "cycleView");
});

test("`#624` shell navigation: showView swaps the panes and names the view now up", () => {
  const state = freshState();
  state.view = "voice";
  const { nav, calls, elements } = makeNavigation(state);
  nav.showView("discord");
  assert.equal(state.view, "discord");
  assert.deepStrictEqual(calls.filter((call) => Array.isArray(call) && call[0] === "showView"),
    [["showView", "discord"]]);
  assert.equal(elements.get("pane-discord").hidden, false, "the channel pane did not come up");
  assert.equal(elements.get("pane-voice").hidden, true, "the call pane stayed up");
  assert.equal(elements.get("view-switch-label").textContent, "Channel");
  assert.equal(elements.get("view-switch").attrs["aria-pressed"], "false");
  // A return restores the captured mark whole, anchorId included, so a redrawn
  // row can be re-found by its message id.
  const anchor = { id: "row" };
  state.viewTop = 120;
  state.viewMark = { pinned: false, anchor, anchorId: "m1", top: 44 };
  calls.length = 0;
  nav.showView("discord");
  assert.deepStrictEqual(calls.filter((call) => Array.isArray(call) && call[0] === "restore"),
    [["restore", { pinned: false, anchor, anchorId: "m1", top: 44 }]],
    "the returning restore did not pass the captured mark through whole");
});

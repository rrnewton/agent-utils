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
  const el = (id) => ({ id, hidden: state.hidden.has(id), focus() { calls.push(["focus", id]); } });
  const nav = SHELL.createPageNavigation({
    createBackStack: NAVCORE.createBackStack, createVisits: NAVCORE.createVisits,
    createDeepLinks: NAVCORE.createDeepLinks, createLinkJumps: NAVCORE.createLinkJumps,
    visitKey: NAVCORE.visitKey, deepLinkQuery: NAVCORE.deepLinkQuery,
    clock: { setTimeout: (run, ms) => { state.timers.push({ run, ms }); return state.timers.length; }, clearTimeout: () => {} },
    session: "test-session",
    screens: () => ({
      screen: () => state.screen, beforeSettings: () => "main", helpReturn: () => "settings",
      setHelpReturn: () => {}, view: () => state.view,
    }),
    el, leftEl: el,
    inSide: () => false, mainColumn: () => "main", inColumn: (_c, act) => act(), sideColumn: () => "side",
    sideMode: () => state.sideMode, sideThreadId: () => state.sideThread,
    selectedChannelId: () => state.channel, selection: () => state.selection,
    layoutColumns: () => state.layout === "two", layoutThreadList: () => state.layout === "list",
    layoutFit: () => true, listBefore: () => ({ layout: "one" }), sideThreadsHas: () => false,
    replyDocked: () => state.replyDocked,
    rowMenuOpen: () => null, promptsOpen: () => false, actionMenuOpen: () => false,
    closeSpeedPopover: () => {}, setPromptsOpen: () => {}, setActionMenuOpen: () => {},
    closeDismissedPopout: () => false, focusList: () => {}, sidePane: () => "side", focusLost: () => false,
    closeSideThread: () => {}, closeHelp: () => calls.push("closeHelp"), closeSettings: () => calls.push("closeSettings"),
    closeReply: () => calls.push("closeReply"), threadListBack: () => calls.push("listBack"),
    leaveThread: () => calls.push("leaveThread"), showScreen: () => {}, showHelp: () => {}, showView: () => {},
    helpEntries: () => [], threadListOffered: () => true, threadStillKnown: () => true, threading: () => true,
    chooseView: (which) => calls.push(["chooseView", which]), selectSideThread: () => {},
    openThreadFromList: () => {}, chooseOneColumn: (choice) => calls.push(["oneColumn", choice.view]),
    defaultChannelView: () => "main", threadsOfChannel: () => [], threadIdOf: (thread) => thread.id,
    token: () => "tok", channels: () => [{ id: "C1" }], chooseChannel: () => {}, changeSelectedChannel: () => {},
    viewKey: () => "main", viewsHas: () => true, readsBusy: () => false, readsStarted: () => 1,
    seekMessage: async () => {}, rowHolding: () => null, answeredElsewhere: () => false,
    providerNameOf: () => "discord", providerName: () => "discord", threadOf: () => null,
    setStatus: (text) => calls.push(["status", text]), showError: (text) => calls.push(["error", text]),
    guardQuietly: (run) => () => run(), settleFocus: () => {}, replyOnScreen: () => false,
    setLinkedMark: () => {}, cardsCycle: () => false, rowShown: () => true, visibleList: () => ({ children: [] }),
    rowOnScreen: () => true, eachColumn: (act) => act(), openReadRun: () => {}, setSearchOpen: () => {},
    finishPackSlide: () => {}, headOfList: () => 0, renderScrollTools: () => {}, revealRow: () => {},
    idsOf: () => [], inAppLinksOn: () => true, channelLabel: (channel) => channel, renderedText: () => "x",
    inThisColumn: (act) => act, captureScroll: () => ({ pinned: true, anchor: null, top: 0 }),
    listPlaceTop: () => 0, sideShowing: () => false, restoreScroll: () => {}, scrollToNewest: () => {},
    bumpReplyJump: () => {}, settleUiState: () => {}, readEnteredChannel: async () => {},
    viewRestored: () => false, stopDiscordPolling: () => {}, storedOpenInView: () => "flat",
    writeClipboardText: async () => true, copiedRefusedText: () => "refused", pullWaitMs: () => 0,
    delay: async () => {}, userActivation: () => null, locationSearch: () => state.search,
    locationOrigin: () => "https://example.test", locationPathname: () => "/",
    navigationIndex: () => 0, historyPort: () => state.history, layoutRestoring: () => false,
  });
  return { nav, calls };
}

const freshState = () => ({
  screen: "main", view: "discord", channel: "C1", layout: "one", sideMode: "cards", sideThread: null,
  selection: { view: "main", threadId: null, origin: null }, replyDocked: false,
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

test("`#624` shell navigation: syncDeepLinkAddress writes the standing place's query once", () => {
  const state = freshState();
  const { nav } = makeNavigation(state);
  nav.syncDeepLinkAddress();
  assert.deepStrictEqual(state.history.replaced, ["/?c=C1&v=main"]);
  state.search = "?c=C1&v=main";
  nav.syncDeepLinkAddress();
  assert.equal(state.history.replaced.length, 1, "an address already saying this is not written again");
});

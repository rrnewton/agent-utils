// Tests for the keyboard composition (web/src/app/keyboard.ts) on its own: the composition
// built as the page build builds it and held directly, with fake page acts and no page
// around it (`#624 voice-page-modularity`, step 17 move 2). Lands as
// vibe-talk/tests/js/app/keyboard.test.mjs. Run with the other module suites under
// cargo test (tests/page_modules.rs).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The composition root (web/src/app). */
const APP = await moduleForTests("src/app/index.ts");

/** A fake element: only what the panes and owners read at construction or on a plain ask. */
function fakeEl(id = "") {
  return {
    id, hidden: false, clientHeight: 100, children: [],
    getBoundingClientRect: () => ({ top: 0, bottom: 10, height: 10 }),
    hasAttribute: () => false, getAttribute: () => null,
    setAttribute: () => {}, removeAttribute: () => {}, focus: () => {},
  };
}

/** Fake page acts: every dep the composition asks for, recording into `log`, none touching a page. */
function fakeDeps(log, overrides = {}) {
  const el = fakeEl("el");
  const main = { name: "main" };
  const side = { name: "side", mode: "list", fromCards: false };
  const deps = {
    screens: { screen: () => "main", view: () => "discord", beforeSettings: () => "main", helpReturn: () => "settings", setHelpReturn: () => {}, arrive: () => {}, setView: () => {} },
    el: () => el, leftEl: () => el, documentRoot: () => el,
    mainColumn: () => main, sideColumn: () => side,
    inColumn: (_column, act) => act(), inSide: () => false, activeColumn: () => main,
    shownChannelList: () => el, floatingClearance: () => 0, channelContextKey: () => "ctx",
    selectionView: () => "main", pinsOnly: () => false, threadListShown: () => false,
    sideColumnUp: () => false, threadListOnScreen: () => false,
    rowShown: () => true, idsOf: () => ["m1"], shownRowFor: (row) => row, rowTimeTabStop: () => {},
    readerActed: () => log.push("readerActed"), putOutLanding: () => {}, noteLinkedSelection: () => {},
    focusLost: () => true, replyLandingGapPx: () => 8, replyDocked: () => false, replyPane: () => null,
    rowMenuOpen: () => false, openRowMenu: () => null,
    toFoot: () => {}, nudge: () => {}, toPosition: () => {}, endFilterWalk: () => {}, closeOverscroll: () => {}, openOverscroll: () => {},
    scrollToNewest: () => {}, isTextField: () => false, scrollsItself: () => false,
    keepsOwnKeys: () => false, pressedBySpace: () => false,
    entryLandingHolds: () => false, entryLandingFirst: () => null,
    setStatus: (text) => log.push(`status:${text}`), rowIsRead: () => false, isDone: () => false,
    toggleArchived: () => {}, guardQuietly: (run) => () => run(),
    selectedChannelId: () => "c1", markRowDone: async () => {}, doneSaved: async () => true,
    markReadInService: async () => ({ service: "s", asked: false, marked: false, already: false, why: "" }),
    doneReadWords: () => "done", serviceReadWords: () => "service", localReadWords: () => "local",
    statusSays: () => false, dismissStatus: () => {},
    showingLinks: () => false, isFolded: () => false, setFolded: () => {}, atBottom: () => false,
    preservingScroll: (act) => act(), preservingCollapse: (_area, _row, _head, _foot, act) => act(), renderScrollTools: () => {}, rowMessages: () => [{ thread: null }],
    isOpenThreadRoot: () => false, viewKey: (...args) => args.join(":"), foldedOnes: () => new Map(),
    closeReadRun: () => {}, renderSideColumn: () => {}, applyReadRuns: () => {},
    openReadRun: () => {}, liveUnpackedRow: () => null, liveCardRow: () => null, actOnRow: () => {},
    cardForArrow: () => null, columnsShown: () => false, isComposer: () => false, closeThread: () => {},
    sideThreadId: () => "", cardRowOnRight: () => null, closeSideThread: () => {},
    toggleMessageDetails: () => {}, upstreamUrlOf: () => null, upstreamServiceName: () => "service",
    copyUpstreamLink: () => {}, copyDeepLink: () => {}, readCardAloud: () => {}, readRowAloud: () => {},
    combinedContent: () => "text", renderedHtmlOf: () => "<p>text</p>", rawTextOf: () => "text",
    writeClipboardText: async () => true,
    closeSettings: () => {}, pressViewSwitch: () => {}, chooseView: () => {}, cycleReadMode: () => {},
    readModeSaid: () => "said", searchOpen: () => false, setSearchOpen: () => {}, setPinnedOnly: () => {},
    linksOnly: () => false, setLinksOnly: () => {}, cycleAuthorFilter: () => {},
    doneOffer: () => null, undoDismissal: () => {}, sendOffer: () => null, sendHeld: () => false,
    putBackDismissedSend: () => {}, bookmarkOffer: () => null, currentChannelId: () => "c1",
    undoBookmarkRemoval: () => {}, threadListBack: () => {}, mainRowsOnScreen: () => true,
    channelsInPickerOrder: () => [], chooseChannel: () => {}, changeSelectedChannel: () => {},
    showScreen: () => {}, showHelp: () => {}, openMenus: () => [], closeOpenMenu: () => false,
    closeRowMenuOnEscape: () => false, replyKeysHere: () => false, closeReply: () => {},
    putOutLitCard: () => false, clearAllThreadFocus: () => false, clearLinkedMark: () => false,
    modKeyName: () => "Ctrl", manyChatServices: () => false, chatServiceName: () => "chat",
    threadOrderWords: () => "newest", minCollapsedLines: () => 3, maxCollapsedLines: () => 12,
    placeFromKeyboard: () => {}, threadStripFromKeyboard: () => {}, foldStepFromKeyboard: () => {},
    collapsedLinesFromKeyboard: () => {}, packFromKeyboard: () => {}, unpackFromKeyboard: () => {},
    bookmarkFromKeyboard: () => {}, bookmarkJumpFromKeyboard: () => {}, orderFromKeyboard: () => {},
    mainRowsFromKeyboard: () => {}, columnsFromKeyboard: () => {},
    childByClass: () => null,
    afterDone: () => "stay",
    deviceSettings: () => ({ get: () => true, keep: () => {} }),
    keyTimingSetting: () => false, locationSearch: () => "?key-timing=1",
    ...overrides,
  };
  // replyPane must be a pane once the composition exists; the fake pages it as the channel pane
  // through an override in the tests that need it. Default null is never called at construction.
  return { deps, el, main, side };
}

test("`#624 voice-page-modularity`: the composition builds the panes and owners in order, and nothing is called early", () => {
  const { createKeyboardComposition } = APP;
  const log = [];
  const { deps } = fakeDeps(log);
  let screenCalls = 0;
  deps.screens = { ...deps.screens, screen: () => { screenCalls += 1; return "main"; } };
  const kb = createKeyboardComposition(deps);
  assert.equal(kb.channelPane.name, "channel");
  assert.equal(kb.sidePane.name, "side");
  assert.deepStrictEqual(kb.KEY_PANES, [kb.channelPane, kb.sidePane], "the panes are not in the page's order");
  assert.equal(kb.lastPane.current(), kb.channelPane, "the last pane does not start on the channel's");
  assert.equal(kb.paneColumn(kb.sidePane).name, "side");
  assert.equal(kb.paneColumn(kb.channelPane).name, "main");
  assert.equal(kb.channelOnScreen(), true);
  assert.equal(kb.keyTimingInAddress, true, "?key-timing in the address was not read");
  assert.equal(kb.COPY_REFUSED, "This browser would not copy to the clipboard.");
  assert.equal(kb.singleKeys.current(), true, "single keys are not on until restored, as before");
  assert.ok(Array.isArray(kb.KEYMAP) && kb.KEYMAP.length >= 40, "the keymap was not built");
  assert.equal(typeof kb.keyDispatch.onPageKey, "function");
  assert.equal(typeof kb.keyHints.render, "function");
  assert.equal(typeof kb.sayRowShortcut, "function");
  // The destructured names are the owners' own functions, not wrappers that could drift.
  assert.equal(kb.paneFor, kb.keySelection.paneFor);
  assert.equal(kb.settleFocus, kb.pointerSelection.settleFocus);
  assert.equal(kb.foldForRead, kb.foldKeys.foldForRead);
  assert.equal(kb.stepChannel, kb.channelKeys.stepChannel);
  assert.equal(kb.onEscape, kb.screenKeys.onEscape);
  assert.ok(screenCalls > 0, "channelOnScreen did not ask the screens");
});

test("`#624 voice-page-modularity`: a later owner is read late — singleKeys saying the keys goes through keyHints built after it", () => {
  const { createKeyboardComposition } = APP;
  const log = [];
  const { deps } = fakeDeps(log);
  let elCalls = 0;
  deps.el = () => { elCalls += 1; return fakeEl("el"); };
  const kb = createKeyboardComposition(deps);
  // Construction called no page act that only a later owner needs: keyHints.render (which
  // singleKeys' render act calls) reads elements, and it was not run to build the graph.
  assert.equal(elCalls, 0, "building the composition read elements for a later owner");
  assert.equal(kb.singleKeys.current(), true);
  // sayRowShortcut delegates to the one RowShortcuts the hints were given.
  const said = [];
  const button = { setAttribute: (n, v) => said.push([n, v]), removeAttribute: (n) => said.push([n, null]) };
  kb.rowShortcuts.set(new Map([["fold", ["Shift+O"]]]));
  kb.sayRowShortcut(button, "fold");
  assert.deepStrictEqual(said, [["aria-keyshortcuts", "Shift+O"]]);
  kb.sayRowShortcut(button, "unknown-class");
  assert.deepStrictEqual(said[1], ["aria-keyshortcuts", null]);
});

test("`#624 voice-page-modularity`: two compositions in one process share no pane, mark or shortcut state", () => {
  const { createKeyboardComposition } = APP;
  const one = createKeyboardComposition(fakeDeps([]).deps);
  const two = createKeyboardComposition(fakeDeps([]).deps);
  assert.notEqual(one.channelPane, two.channelPane);
  assert.notEqual(one.KEY_PANES, two.KEY_PANES);
  one.channelPane.marks.set("ctx", { id: "m1", drawn: true, index: 0 });
  assert.equal(two.channelPane.marks.size, 0, "the second composition saw the first's marks");
  one.lastPane.take(one.sidePane);
  assert.equal(two.lastPane.current(), two.channelPane, "the second composition's last pane moved");
  one.rowShortcuts.set(new Map([["fold", ["x"]]]));
  assert.deepStrictEqual([...two.rowShortcuts.classes()], [], "the second composition saw the first's shortcuts");
});

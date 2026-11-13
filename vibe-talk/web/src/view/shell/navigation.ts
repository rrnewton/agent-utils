// Navigation orchestration over the channel: the phone's back gesture and the browser's Back
// (`#304 back-gesture-in-app`, `#423 back-key-consistency`), Back and Forward through the places
// visited, deep links (`#462 webapp-deep-links`) and in-app message links (`#543
// in-app-message-links`, `#580 open-in-chat-jump`); `#624 voice-page-modularity`, step 17.
//
// WHAT THIS OWNS. Only the legacy orchestration: reading where the reader stands from the page's
// screens, columns and selection, deciding what a Back closes and in what order, showing a visited
// place, keeping the address in step with the place, applying the link the page opened by, and
// jumping to (and back from) a message a link names. The state machines are NOT here: the history
// entries are `core/navigation`'s `BackStack`, the visited places its `Visits`, the opened-by link
// its `DeepLinks`, and the jumps its `LinkJumps`. Those owners are created here from the factories
// the page injects, in the order the legacy script created them, so this file needs no import of
// `core/navigation` (the architecture row for `view/shell` admits only the foundations, the ports
// and `view/dom`): every core shape it speaks of is the structural shape below, satisfied by the
// core owners the page passes in.
//
// EVERY DEPENDENCY IS A THUNK. The legacy script defined these functions over state declared
// after them (`screens` at the screens section, `layoutChoice` near the end of the file) and
// relied on function declarations being hoisted. A factory's destructured functions are `const`s,
// so nothing below may read a dependency's value while `createPageNavigation` itself runs: each
// dep is called only inside a returned function, never at creation. See NOTES.md (TDZ/ordering).

import type { DeepLinkRef, MessageLinkRef } from "../../foundation/index.ts";
import { messageLinkRef } from "../../foundation/index.ts";
import type { ClockPort } from "../../ports/index.ts";
import { nodeWithin } from "../dom/index.ts";
import type { Screens } from "./screens.ts";

/** A place on a channel's main screen (structural twin of `core/navigation`'s `ChannelPlace`). */
export interface NavChannelPlace {
  screen: "main";
  channel: string;
  layout: "one" | "list" | "two";
  view: string | null;
  thread: string | null;
  origin: string | null;
  side: string | null;
}

/** Any place: a channel's, or Settings or Help over the channel place they were opened from. */
export type NavPlace = NavChannelPlace | {
  screen: "settings" | "help";
  channel: string;
  under: NavChannelPlace | null;
  from: string | null;
  entry: string | null;
};

/** The window's history, as far as the entries and the address use it. */
export interface NavHistoryPort {
  readonly state: unknown;
  pushState(state: unknown, title: string, url?: string): void;
  replaceState(state: unknown, title: string, url?: string): void;
  back(): void;
  go(delta: number): void;
  scrollRestoration?: string;
}

/** What a link asks for: its channel, view, thread and message, each null where it says none. */
export interface NavDeepLinkWanted {
  readonly channel: string | null;
  readonly view: string | null;
  readonly thread: string | null;
  readonly message: DeepLinkRef | null;
}

/** The history entries' owner, as created by `core/navigation`'s `createBackStack`. */
export interface NavBackStack {
  started(): boolean;
  deferred(): boolean;
  start(): boolean;
  schedule(): void;
  pop(state: unknown): void;
}

/** What creating the history entries asks of this orchestration (its `BackStackDependencies`). */
export interface NavBackStackDependencies {
  readonly clock: ClockPort;
  readonly session: string;
  history(): NavHistoryPort | null;
  index(): number | null;
  mayPush(): boolean;
  levels(): string[];
  close(tag: string): void;
  failed(message: string): void;
  notePlace(): void;
  syncAddress(): void;
  applyLink(): void;
}

/** The visited places' owner, as created by `core/navigation`'s `createVisits`. */
export interface NavVisits {
  note(place: NavPlace | null): void;
  walk(step: number, here: string, showable: (place: NavPlace) => boolean, go: (place: NavPlace) => void): boolean;
  arriving(): boolean;
  position(): { readonly at: number; readonly length: number };
}

/** The opened-by link's owner, as created by `core/navigation`'s `createDeepLinks`. */
export interface NavDeepLinks {
  wanted(): NavDeepLinkWanted | null;
  focus(): DeepLinkRef | null;
  setFocus(ref: DeepLinkRef | null): void;
  written(): string | null;
  noteWritten(address: string): void;
  applied(): boolean;
  markApplied(): void;
  applying(): boolean;
  setApplying(applying: boolean): void;
  epoch(): number;
  noteReaderMove(): void;
  readerMoves(): number;
  readerMoved(): boolean;
  showing(): boolean;
  arrive<T>(run: () => T): T;
  signOut(): void;
}

/** Where the reader was when a jump was taken (structural twin of `core/navigation`'s `LinkSpot`). */
export interface NavLinkSpot {
  readonly place: NavChannelPlace | null;
  readonly column: "main" | "side";
  readonly pinned: boolean;
  readonly anchor: string | null;
  readonly offset: number;
  readonly focus: DeepLinkRef | null;
  readonly call?: boolean;
  readonly docs?: boolean;
}

/** A jump taken (structural twin of `core/navigation`'s `LinkJump`). */
export interface NavLinkJump {
  readonly spot: NavLinkSpot;
  readonly below: readonly string[];
  landed: string[] | null;
  /** The document this jump opened (`#1082 documents-back-chain`), where it opened one. */
  readonly doc?: string;
}

/** The jumps' owner, as created by `core/navigation`'s `createLinkJumps`. */
export interface NavLinkJumps {
  top(): NavLinkJump | undefined;
  take(jump: NavLinkJump): number;
  undo(): NavLinkJump | undefined;
  holds(jump: NavLinkJump): boolean;
  current(ticket: number): boolean;
  nextTicket(): number;
  returnStarted(): void;
  returnEnded(): void;
  levels(levels: string[]): string[];
  forget(): void;
}

/** A message, as far as a link made from it reads it. */
export interface NavMessage {
  readonly id: unknown;
  readonly channel_id?: unknown;
  readonly thread?: { readonly is_root?: unknown; readonly root_message_id?: unknown } | null;
}

/** Why a timeline read was asked for, as the timeline view accepts it. */
export interface NavReadOptions {
  keepPosition?: boolean;
  quiet?: boolean;
  ownAct?: boolean;
  reason?: string | readonly (string | undefined)[];
  readAs?: string;
}

/** The gathered replies' record, as far as changing channel clears it. */
export interface NavGatherState {
  gathered: unknown;
}

/** The list record, as far as changing channel resets it. */
export interface NavListState {
  rowsGathered: boolean;
  newestId: unknown;
}

/** The older-pages record, as far as changing channel resets it. */
export interface NavOlderPages {
  moreAbove: boolean;
  cursor: unknown;
}

/** The load record, as far as changing channel resets it. */
export interface NavLoadState {
  freshAt: number;
  freshness: string;
}

/** Where each view was left, as `showView` reads and writes it. */
export interface NavViewScroll {
  leave(view: string, place: { top: number; atNewest: boolean }): void;
  enter(view: string): number | null;
}

/** A captured scroll place, as the scroll owner captures and restores it. */
export interface NavScrollMark {
  pinned: boolean;
  anchor: Element | null;
  top: number;
  anchorId?: string | null;
}

/** The read-aloud owner, as far as leaving the channel stops or keeps a read. */
export interface NavReadOwner {
  readonly nowPlaying: { readonly kind?: unknown } | null;
  readonly pendingRead: unknown;
  readingMode: boolean;
  stopReading(): void;
}

/** What the page injects: the core factories, created owners' ports, and every page act, all thunked. */
export interface PageNavigationDependencies<Column, ThreadLike, Pane> {
  // The core owners' factories (the page's prelude names), injected so this package imports no core.
  createBackStack(deps: NavBackStackDependencies): NavBackStack;
  createVisits(): NavVisits;
  createDeepLinks(deps: { search(): string; arriving(): boolean }): NavDeepLinks;
  createLinkJumps(): NavLinkJumps;
  /** What tells one place from another (`core/navigation`'s `visitKey`), injected for the same reason. */
  visitKey(place: NavPlace | null | undefined): string;
  /** The query for a channel place (`core/navigation`'s `deepLinkQuery`), injected for the same reason. */
  deepLinkQuery(place: NavChannelPlace, ref?: DeepLinkRef | null): string;
  readonly clock: ClockPort;
  /** This page load's own name for the entries it makes (`BACK_SESSION`, kept in the legacy script). */
  readonly session: string;
  /** Whether something over the channel is open (`rowMenus.open` etc. are read through `openMenus`). */
  // Page state, read only inside the returned functions.
  screens(): Pick<Screens, "screen" | "beforeSettings" | "helpReturn" | "setHelpReturn" | "view" | "setView">;
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  inSide(): boolean;
  mainColumn(): Column;
  inColumn<T>(column: Column, act: () => T): T;
  sideColumn(): Column;
  sideMode(): string;
  sideThreadId(channel: string): string | null;
  selectedChannelId(): unknown;
  selection(): { view: string; threadId: unknown; origin: string | null };
  layoutColumns(): boolean;
  layoutThreadList(): boolean;
  layoutFit(): boolean;
  listBefore(): { layout: string; channel?: string; view?: string; thread?: string | null };
  sideThreadsHas(channel: string): boolean;
  replyDocked(): boolean;
  // Menus and screens.
  rowMenuOpen(): { root: Node; close(): void; button: { focus(options?: { preventScroll?: boolean }): void } | null } | null;
  promptsOpen(): boolean;
  actionMenuOpen(): boolean;
  closeSpeedPopover(): void;
  setPromptsOpen(open: boolean): void;
  setActionMenuOpen(open: boolean): void;
  closeDismissedPopout(): boolean;
  focusList(pane: Pane): void;
  sidePane(): Pane;
  focusLost(): boolean;
  closeSideThread(): void;
  closeHelp(): void;
  closeSettings(): void;
  closeReply(): void;
  /** Whether a document is open in the Documents view (its back level). */
  documentOpen(): boolean;
  /**
   * How many documents are open in the Documents view's chain
   * (`#1082 documents-back-chain`), where the page wires it: one back level
   * each. Unwired, an open document counts as the one level it always was.
   */
  documentDepth?(): number;
  /**
   * Whether the open document's outline pop-up is open, and its close
   * (`#1038 documents-outline-popup`): its `menu:outline` level. Unwired, the
   * pop-up element itself is read and its button pressed.
   */
  documentOutlineOpen?(): boolean;
  closeDocumentOutline?(): boolean;
  /** Close the open document one step: the previous one, or its list. */
  closeDocument(): void;
  threadListBack(): void;
  leaveThread(): void;
  showScreen(name: string): void;
  showHelp(entry: string | null): void;
  // Places and views.
  helpEntries(): readonly { id: unknown; open: boolean }[];
  threadListOffered(): boolean;
  threadStillKnown(id: string): boolean;
  threading(): boolean;
  chooseView(which: "main" | "flat" | "threads" | "columns"): void;
  selectSideThread(id: string): void;
  openThreadFromList(id: string): void;
  chooseOneColumn(choice: { view: string; thread: string; summary: ThreadLike | null; origin: string }): void;
  defaultChannelView(): string;
  threadsOfChannel(): readonly ThreadLike[];
  threadIdOf(thread: ThreadLike): unknown;
  // Deep links and jumps.
  token(): string;
  channels(): readonly { id: unknown }[];
  chooseChannel(id: string): void;
  cycleView(): void;
  loadDiscord(options: NavReadOptions): Promise<void>;
  readSideColumn(reason: string, since: number): Promise<void>;
  scheduleDiscordPoll(): void;
  putDockedReplyAway(): void;
  gatherState(): NavGatherState;
  listState(): NavListState;
  dropReplyArrowTap(): void;
  applyChannelProvider(): void;
  rememberActiveChannel(chosen?: boolean): void;
  rememberChannelDraft(): void;
  openChannelView(): void;
  forgetThreadPresence(): void;
  timelineRows(): { messages: unknown[]; threads: unknown[] };
  clearChannelContexts(): void;
  restoreThreadHues(channel: unknown): void;
  seedServerThreadHues(channel: unknown): unknown;
  restoreChannelComposer(): void;
  olderPages(): NavOlderPages;
  payReadCursor(): void;
  renderOlderControl(): void;
  loadState(): NavLoadState;
  hydrateChannelScope(): boolean;
  renderChannelFreshness(): void;
  saveUiState(): void;
  pinsOnly(): boolean;
  ensurePins(): void;
  renderChannelRows(): void;
  resetSideColumn(reopen?: boolean): void;
  renderColumns(): void;
  startChannelStream(channel: unknown): void;
  viewScroll(): NavViewScroll;
  atBottom(area: HTMLElement): boolean;
  viewMark(name: string): NavScrollMark | undefined;
  setViewMark(name: string, mark: NavScrollMark): void;
  holdChannelPlace(): void;
  placePostCard(): void;
  syncDocumentsSelector(): void;
  syncDiscussView(): void;
  forgetJumpNewest(name: string): void;
  dismissStatus(): void;
  readAloudOwner(): NavReadOwner;
  renderCallControls(): void;
  renderControlBar(): void;
  renderChannelNavigation(): void;
  placeChipsOverComposer(): void;
  viewKey(): string;
  viewsHas(key: string): boolean;
  readsBusy(): boolean;
  readsStarted(): number;
  seekMessage(target: string, land: () => unknown, elsewhere: () => unknown): Promise<void>;
  rowHolding(id: string): Element | null;
  answeredElsewhere(target: string): boolean;
  providerNameOf(channel: string): string;
  providerName(): string;
  threadOf(message: NavMessage): string | null;
  setStatus(text: string): void;
  showError(message: string): void;
  guardQuietly<A extends unknown[], R>(run: (...args: A) => R): (...args: A) => Promise<Awaited<R>>;
  settleFocus(): void;
  replyOnScreen(): boolean;
  // Linked mark (owned by `view/channel`'s `createLinkedMark`, still created in the legacy script).
  setLinkedMark(channel: string, id: string): void;
  cardsCycle(): boolean;
  rowShown(row: HTMLElement): boolean;
  visibleList(): HTMLElement;
  rowOnScreen(row: HTMLElement): boolean;
  eachColumn(act: () => void): void;
  openReadRun(row: HTMLElement): void;
  setSearchOpen(open: boolean): void;
  finishPackSlide(): void;
  headOfList(area: HTMLElement, row: HTMLElement): number;
  /** Put a scroller at a measured position, through the scroll owner (`#1251 scroll-single-owner`). */
  toPosition(area: HTMLElement, top: number): void;
  renderScrollTools(): void;
  revealRow(row: HTMLElement, center: boolean): void;
  /**
   * Note the row a jump just landed on as the scroll anchor (`#1224 mention-jump-in-view`):
   * what arrives above it afterwards holds it, not the first row on screen. Optional where
   * a host does not wire the anchor; the landing itself is unchanged there.
   */
  noteJumpAnchor?(row: HTMLElement, id: string): void;
  idsOf(row: HTMLElement): string[];
  // In-app links.
  inAppLinksOn(): boolean;
  /** Open the document `key` in the in-app reader (a marked document link). */
  openDocument(key: string): void;
  channelLabel(channel: string): string;
  renderedText(node: Element): string;
  inThisColumn<T>(act: (event: T) => void): (event: T) => void;
  captureScroll(): NavScrollMark;
  listPlaceTop(area: HTMLElement): number;
  sideShowing(): boolean;
  restoreScroll(place: NavScrollMark): void;
  scrollToNewest(): void;
  bumpReplyJump(): void;
  settleUiState(): void;
  viewRestored(): boolean;
  stopDiscordPolling(): void;
  storedOpenInView(): "flat" | "main";
  writeClipboardText(text: string): Promise<boolean>;
  copiedRefusedText(): string;
  pullWaitMs(): number;
  delay(ms: number): Promise<void>;
  userActivation(): { hasBeenActive: boolean } | null;
  layoutRestoring(): boolean;
  locationSearch(): string;
  locationOrigin(): string;
  locationPathname(): string;
  navigationIndex(): number | null;
  historyPort(): NavHistoryPort | null;
}

/** The orchestration's commands and questions, under the legacy script's own names. */
export interface PageNavigation {
  readonly backStack: NavBackStack;
  readonly visits: NavVisits;
  readonly deepLinks: NavDeepLinks;
  readonly linkJumps: NavLinkJumps;
  backHistory(): NavHistoryPort | null;
  historyIndex(): number | null;
  mayPushHistory(): boolean;
  openMenus(): string[];
  closeOpenMenu(name: string): boolean;
  backLevels(): string[];
  screenLevelsNow(): string[];
  closeSideByX(): void;
  closeBackLevel(tag: string): void;
  scheduleBackSync(): void;
  onBackPop(event: { state: unknown } | null): void;
  startBackStack(): void;
  channelPlace(): NavChannelPlace | null;
  currentPlace(): NavPlace | null;
  notePlace(): void;
  channelPlaceShowable(place: NavChannelPlace | null): boolean;
  placeShowable(place: NavPlace): boolean;
  showChannelPlace(place: NavChannelPlace): void;
  showPlace(place: NavPlace): void;
  placeFromKeyboard(step: number): boolean;
  noteDeepLinkReaderMove(): void;
  deepLinkReaderMoved(): boolean;
  deepLinkRefOf(message: NavMessage): DeepLinkRef;
  deepLinkFromAddress(): NavDeepLinkWanted | null;
  deepLinkFor(ref: DeepLinkRef): string;
  syncDeepLinkAddress(): void;
  linkedRowShown(row: HTMLElement): boolean;
  landOnDeepLink(id: string, center?: boolean): boolean;
  waitForDeepLink(ready: () => boolean): Promise<boolean>;
  applyDeepLink(): Promise<void>;
  copyDeepLink(message: NavMessage): Promise<boolean>;
  keepsInApp(ref: MessageLinkRef | null): boolean;
  messageLinkLabel(ref: MessageLinkRef): string;
  wireMessageLinks(box: Element): void;
  onMessageLinkTap(ref: MessageLinkRef, event: {
    ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean; button?: number;
    preventDefault?: () => void; stopPropagation?: () => void;
  } | null): void;
  linkJumpSpot(): NavLinkSpot;
  withLinkJumps(levels: string[]): string[];
  arriveOnChannel(channel: string): void;
  arriveAtPlace(place: NavChannelPlace): void;
  jumpToDocumentLink(key: string, from?: Element | null): void;
  documentBack(): void;
  jumpToMessageLink(ref: MessageLinkRef): Promise<void>;
  jumpToSentMessage(ref: MessageLinkRef): Promise<void>;
  returnFromLinkJump(): void;
  putBackOnSpot(spot: NavLinkSpot): boolean;
  pressViewSwitch(): void;
  readEnteredChannel(keepPosition: boolean): Promise<void>;
  changeSelectedChannel(): Promise<void>;
  showView(name: string): void;
}

/** How long opening from a link waits for sign-in and a first read, in `pullWaitMs` waits. */
const DEEP_LINK_WAITS = 600;

/** The orchestration, with nothing created yet: the owners below are made in the legacy order. */
export function createPageNavigation<Column, ThreadLike, Pane>(deps: PageNavigationDependencies<Column, ThreadLike, Pane>): PageNavigation {
  const el = deps.el;

  // --- the phone's back gesture, and the browser's Back (`#304`) --------------------------------
  // The long rationale (what opens over the channel, THE HISTORY MIRRORS WHAT IS OPEN, THE GESTURE
  // CLOSES WHAT THE BUTTON CLOSES, a reload standing on the earlier load's entries, a menu being an
  // entry too) stays with the legacy script's section head, which this factory replaces; it is the
  // design record for the deps below and for `core/navigation`'s `BackStack`, whose header carries
  // the same account from the owner's side.

  // The page's own history entries: one owner in `core/navigation` (`createBackStack`). What is open
  // (`backLevels`) and how each level closes (`closeBackLevel`) are this factory's, below.
  const backStack: NavBackStack = deps.createBackStack({
    clock: deps.clock,
    session: deps.session,
    history: () => backHistory(),
    index: () => historyIndex(),
    mayPush: () => mayPushHistory(),
    levels: () => backLevels(),
    close: (tag) => closeBackLevel(tag),
    failed: (message) => deps.showError(message),
    notePlace: () => notePlace(),
    syncAddress: () => syncDeepLinkAddress(),
    applyLink: () => deps.guardQuietly(applyDeepLink)(),
  });

  /** The window's history, where it can be written; null in a host without one. */
  function backHistory(): NavHistoryPort | null {
    return deps.historyPort();
  }

  /**
   * Where the browser is in the tab's history, by the Navigation API where it has one; null elsewhere.
   * Counted from the first of the run of this origin's entries the browser is in, so in an installed app,
   * whose window starts at the app, it is how many entries of the app's own there are under this one.
   */
  function historyIndex(): number | null {
    return deps.navigationIndex();
  }

  /** Whether the page may push an entry now: not before the reader has ever touched it, where the browser says. */
  function mayPushHistory(): boolean {
    const activation = deps.userActivation();
    return !activation || activation.hasBeenActive !== false;
  }

  /** The menus open now, as levels, in the order Escape closes them. */
  function openMenus(): string[] {
    const open: string[] = [];
    if (deps.rowMenuOpen()) open.push("menu:row");
    if (!el("speed-popover").hidden) open.push("menu:pace");
    if (deps.promptsOpen()) open.push("menu:prompts");
    if (deps.actionMenuOpen()) open.push("menu:actions");
    // `#348 settings-dynamic-lists`. The pop-out over Recently dismissed, on Settings.
    if (!el("dismissed-send-popout").hidden) open.push("menu:dismissed");
    // `#1038 documents-outline-popup`. The outline; unwired, its toggle's expanded word tells it.
    const outlineToggle = el("doc-outline-open");
    if (deps.documentOutlineOpen
      ? deps.documentOutlineOpen()
      : typeof outlineToggle.getAttribute === "function" && outlineToggle.getAttribute("aria-expanded") === "true") {
      open.push("menu:outline");
    }
    return open;
  }

  /**
   * Close menu `name` as Escape closes it, and say whether it was open. The ⋯ menu gives the focus back
   * to the ⋯ that opened it when the focus was inside it — an item just hidden is a focus on nothing —
   * and the pace popover and the prompts tray give it to their buttons.
   */
  function closeOpenMenu(name: string): boolean {
    const rowMenu = deps.rowMenuOpen();
    if (name === "menu:row" && rowMenu) {
      const { root, close, button } = rowMenu;
      const inside = nodeWithin(root, document.activeElement);
      close();
      if (inside && button && typeof button.focus === "function") button.focus({ preventScroll: true });
      return true;
    }
    if (name === "menu:pace" && !el("speed-popover").hidden) {
      deps.closeSpeedPopover();
      el("read-speed").focus();
      return true;
    }
    if (name === "menu:prompts" && deps.promptsOpen()) {
      deps.setPromptsOpen(false);
      el("prompts-open").focus();
      return true;
    }
    if (name === "menu:actions" && deps.actionMenuOpen()) {
      deps.setActionMenuOpen(false);
      el("action-menu-open").focus();
      return true;
    }
    if (name === "menu:dismissed" && !el("dismissed-send-popout").hidden) return deps.closeDismissedPopout();
    if (name === "menu:outline") {
      if (deps.closeDocumentOutline) return deps.closeDocumentOutline();
      const toggle = el("doc-outline-open");
      if (typeof toggle.getAttribute === "function" && toggle.getAttribute("aria-expanded") === "true" &&
        typeof toggle.click === "function") {
        toggle.click();
        if (typeof toggle.focus === "function") toggle.focus({ preventScroll: true });
        return true;
      }
    }
    return false;
  }

  /**
   * What a Back would close now, the first-opened first: the levels the page keeps an entry for. Read
   * in the left column, the one worked on outside `inColumn`.
   */
  function backLevels(): string[] {
    // `#543 in-app-message-links`. A jump by a link is a level over what was open when it was taken.
    return withLinkJumps(screenLevelsNow());
  }

  /**
   * What is open now, as levels, the first-opened first, without the jumps taken by links. Read in the
   * left column, the one worked on outside `inColumn`.
   */
  function screenLevelsNow(): string[] {
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), screenLevelsNow);
    const levels = screenLevels(deps.screens().screen());
    // On top of everything, in every browser: see "A MENU IS AN ENTRY TOO" in the section head.
    levels.push(...openMenus());
    return levels;
  }

  /**
   * The levels of screen `name`, with those of the screen it covers under it: Settings over the screen
   * it was opened from, Help over Settings or over where "?" was pressed (`helpReturn`). The sign-in
   * screen is the bottom of everything, with none.
   */
  function screenLevels(name: string, depth = 0): string[] {
    if (depth > 2) return [];
    if (name === "main") return channelLevels();
    if (name === "settings") return [...screenLevels(deps.screens().beforeSettings(), depth + 1), "settings"];
    if (name === "help") return [...screenLevels(deps.screens().helpReturn(), depth + 1), "help"];
    return [];
  }

  /**
   * What is open over the channel's own view: the right column's thread in two columns; the thread list,
   * over what Esc on it goes back to; a thread in one column, over the thread list it was opened from;
   * and a reply open in a list. Nothing on the call's view, which waits in front of them.
   */
  function channelLevels(): string[] {
    // Each document open in the Documents view's chain is a level of its own
    // (`#1082 documents-back-chain`): the view owns the chain, this only asks
    // its depth, and each level's close is the view's one step back. Menus
    // still go on top of them all, in `screenLevelsNow`.
    if (deps.screens().view() === "docs") {
      const wired = deps.documentDepth ? deps.documentDepth() : -1;
      const depth = Number.isFinite(wired) && wired >= 0 ? Math.floor(wired) : deps.documentOpen() ? 1 : 0;
      const count = depth > 0 ? depth : deps.documentOpen() ? 1 : 0;
      return Array.from({ length: count }, () => "document");
    }
    if (deps.screens().view() !== "discord") return [];
    const levels: string[] = [];
    if (deps.layoutColumns()) {
      if (deps.sideMode() === "thread") levels.push("side");
    } else if (deps.layoutThreadList()) {
      levels.push(...underThreadList(), "list");
    } else if (deps.selection().view === "thread") {
      if (deps.selection().origin === "list" && deps.threadListOffered()) levels.push(...underThreadList(), "list");
      levels.push("thread");
    }
    if (deps.replyDocked()) levels.push("reply");
    return levels;
  }

  /**
   * What Esc on the thread list goes back to, where that is itself something a Back closes: a thread in
   * one column, or two columns with a thread on the right (`threadListBack`). So the list chosen over a
   * thread is one entry more, and its Back finds the thread's entry still there.
   */
  function underThreadList(): string[] {
    const channel = String(deps.selectedChannelId() || "");
    const before = deps.listBefore();
    if (before.layout === "two") return deps.layoutFit() && deps.sideThreadsHas(channel) ? ["side"] : [];
    const thread = before.layout === "one" && before.channel === channel && before.view === "thread";
    return thread && before.thread ? ["thread"] : [];
  }

  /** The X of the right column: its thread goes and the cards come back, the keys going to them if they had none. */
  function closeSideByX(): void {
    deps.closeSideThread();
    // The X has gone with the thread: the keys go to the cards.
    if (document.activeElement === el("side-close") || deps.focusLost()) deps.focusList(deps.sidePane());
  }

  /** Close level `tag` as its own Back, X or Escape closes it. */
  function closeBackLevel(tag: string): void {
    if (tag.startsWith("menu:")) closeOpenMenu(tag);
    else if (tag === "help") deps.closeHelp();
    else if (tag === "settings") deps.closeSettings();
    else if (tag === "reply") deps.closeReply();
    else if (tag === "side") closeSideByX();
    else if (tag === "list") deps.threadListBack();
    else if (tag === "thread") deps.leaveThread();
    else if (tag === "document") deps.closeDocument();
    else if (tag === "jump") returnFromLinkJump();
  }

  /**
   * The reader's own Back button (`#1082 documents-back-chain`): the same pop
   * the gesture takes — the top level, whatever it is (a menu first, then a
   * document step, then the jump that opened the chain from a channel). With
   * nothing open over the document, its close is the fallback.
   */
  function documentBack(): void {
    const levels = backLevels();
    if (levels.length === 0) {
      deps.closeDocument();
      return;
    }
    closeBackLevel(levels[levels.length - 1] as string);
    scheduleBackSync();
  }

  /** Put the history in step with what is open, at the end of the task that changed it (`BackStack.schedule`). */
  function scheduleBackSync(): void {
    backStack.schedule();
  }

  /**
   * The browser went back (or forward) to another entry: close one thing for each entry it went back
   * through, as the Back of each thing closed would (`BackStack.pop`).
   */
  function onBackPop(event: { state: unknown } | null): void {
    backStack.pop(event ? event.state : null);
  }

  /**
   * Keep the history in step from here on. The page takes the entry it opened on for its own: at the
   * depth an earlier load gave it, standing on that load's entries under it, never more than the browser
   * says there are entries of the page's own under it; or, holding anything else, as its base. The browser
   * is told not to move the document on a Back, the lists keeping their own places (`#280
   * scroll-stability`); and an entry put off until the reader's first touch is pushed then.
   *
   * ORDERING: the legacy script calls this LAST, with every part of the page defined. Keep that call
   * last in `voice.js`; do not fold it into this factory's creation. The `popstate` listener
   * registered here is the destructured `onBackPop` itself, the same function object the legacy
   * script's callers name (see NOTES.md for the test control that matched its registration text).
   */
  function startBackStack(): void {
    if (backStack.start()) {
      window.addEventListener("popstate", onBackPop);
    }
    const touched = (): void => {
      if (backStack.deferred()) scheduleBackSync();
    };
    for (const type of ["pointerup", "keydown", "click"]) document.addEventListener(type, touched, true);
    scheduleBackSync();
  }

  // --- Back and Forward through the places visited (`#423`) --------------------------------------
  // THE RULE, THE SPECIAL CASES and WHAT STAYS are told in the legacy script's section head and in
  // `core/navigation`'s `Visits` header; only the page-reading half is here.

  // The places visited on this channel: one owner in `core/navigation` (`createVisits`).
  const visits: NavVisits = deps.createVisits();

  /** The place on the channel now, read in the left column, whose state is the page's; null on the call's view. */
  function channelPlace(): NavChannelPlace | null {
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), channelPlace);
    if (deps.screens().view() !== "discord") return null;
    const channel = String(deps.selectedChannelId() || "");
    if (!channel) return null;
    const place: NavChannelPlace = { screen: "main", channel, layout: "one", view: null, thread: null, origin: null, side: null };
    if (deps.layoutColumns()) {
      place.layout = "two";
      const side = deps.sideMode() === "thread" ? deps.sideThreadId(channel) : null;
      place.side = side ? String(side) : null;
    } else if (deps.layoutThreadList()) {
      place.layout = "list";
    } else {
      place.view = deps.selection().view;
      if (deps.selection().view === "thread") {
        place.thread = deps.selection().threadId ? String(deps.selection().threadId) : null;
        place.origin = deps.selection().origin || null;
      }
    }
    return place;
  }

  /** The place the reader is at now: the channel's, or Settings or Help over the screen they were opened from. */
  function currentPlace(): NavPlace | null {
    if (deps.screens().screen() === "main") return channelPlace();
    if (deps.screens().screen() !== "settings" && deps.screens().screen() !== "help") return null;
    const under = deps.screens().beforeSettings() === "main" ? channelPlace() : null;
    const open = deps.screens().screen() === "help" ? deps.helpEntries().find((entry) => entry.open) : null;
    return {
      screen: deps.screens().screen() as "settings" | "help", channel: String(deps.selectedChannelId() || ""), under,
      from: deps.screens().screen() === "help" ? deps.screens().helpReturn() : null,
      entry: open ? String(open.id).replace(/^help-/, "") : null,
    };
  }

  /** Note the place the reader is at, as the one after the place they were at, unless it is that place. */
  function notePlace(): void {
    visits.note(currentPlace());
  }

  /** Whether channel place `place` can be shown now. */
  function channelPlaceShowable(place: NavChannelPlace | null): boolean {
    if (!place) return true;
    if (place.channel !== String(deps.selectedChannelId() || "")) return false;
    if (place.layout === "two") return deps.layoutFit() && deps.threadListOffered() && (!place.side || deps.threadStillKnown(place.side));
    if (place.layout === "list") return deps.threadListOffered();
    if (place.view === "thread") return place.thread !== null && deps.threadStillKnown(place.thread);
    return place.view === "main" || deps.threading();
  }

  /** Whether `place` can be shown now: its channel the one chosen, and what it shows still there. */
  function placeShowable(place: NavPlace): boolean {
    if (place.screen === "main") return channelPlaceShowable(place);
    return place.channel === String(deps.selectedChannelId() || "") && channelPlaceShowable(place.under);
  }

  /**
   * Put the channel on place `place`, by the close Esc uses where `place` is what is under what is open
   * (see THE SPECIAL CASES), and by the views' own choosing for whatever is left to change.
   */
  function showChannelPlace(place: NavChannelPlace): void {
    if (deps.inSide()) {
      deps.inColumn(deps.mainColumn(), () => showChannelPlace(place));
      return;
    }
    // Arriving, not choosing: Back and Forward, and a link being applied. `#462 webapp-deep-links`.
    deepLinks.arrive(() => showChannelPlaceWithin(place));
  }

  /** `showChannelPlace`, with `#462 webapp-deep-links`'s arriving-not-choosing already noted. */
  function showChannelPlaceWithin(place: NavChannelPlace): void {
    if (deps.screens().screen() !== "main" || deps.screens().view() !== "discord") return;
    const now = channelPlace();
    if (now && now.layout === "one" && now.view === "thread" && now.origin === "list" && place.layout === "list") deps.leaveThread();
    else if (now && now.layout === "list" && place.layout !== "list") deps.threadListBack();
    else if (now && now.layout === "two" && now.side && place.layout === "two" && !place.side) closeSideByX();
    if (deps.visitKey(channelPlace()) === deps.visitKey(place)) return;
    if (place.layout === "list") {
      deps.chooseView("threads");
    } else if (place.layout === "two") {
      if (!deps.layoutColumns()) deps.chooseView("columns");
      const shown = deps.sideMode() === "thread" ? deps.sideThreadId(place.channel) : null;
      if (place.side && (!shown || String(shown) !== place.side)) deps.selectSideThread(place.side);
      else if (!place.side && deps.sideMode() === "thread") closeSideByX();
    } else if (place.view === "thread") {
      const id = String(place.thread);
      if (place.origin === "list" && deps.threadListOffered()) deps.openThreadFromList(id);
      else {
        const summary = deps.threadsOfChannel().find((held) => String(deps.threadIdOf(held)) === id) || null;
        deps.chooseOneColumn({ view: "thread", thread: id, summary, origin: place.origin || deps.defaultChannelView() });
      }
    } else {
      deps.chooseView(place.view as "main" | "flat");
    }
  }

  /** Go to place `place`: off the screens over the channel, the channel on the place under, and on to its screen. */
  function showPlace(place: NavPlace): void {
    if (deps.screens().screen() === "help" && deps.visitKey(place) !== deps.visitKey(currentPlace())) deps.closeHelp();
    const under = place.screen === "main" ? place : place.under;
    if (deps.screens().screen() === "settings" && (place.screen === "main" || (place.screen === "help" && place.from !== "settings"))) {
      deps.closeSettings();
    }
    if (deps.screens().screen() === "main" && under) showChannelPlace(under);
    if (place.screen === "settings" && deps.screens().screen() !== "settings") deps.showScreen("settings");
    if (place.screen === "help" && deps.screens().screen() !== "help") {
      if (place.from === "settings" && deps.screens().screen() !== "settings") deps.showScreen("settings");
      deps.screens().setHelpReturn(deps.screens().screen());
      deps.showHelp(place.entry);
    }
  }

  /**
   * Ctrl/Cmd+[ (`step` -1) and Ctrl/Cmd+] (+1): back to the place before, or forward to the place after. See
   * the rule in the section head. Always takes the chord, so it is never the browser's Back or Forward.
   */
  function placeFromKeyboard(step: number): boolean {
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), () => placeFromKeyboard(step));
    const menus = openMenus();
    if (menus.length > 0) {
      closeOpenMenu(menus[menus.length - 1] as string);
      return true;
    }
    if (deps.screens().screen() === "main" && deps.screens().view() !== "discord") {
      deps.setStatus("Back and Forward go through the channel's views; the call has none.");
      return true;
    }
    if (step < 0 && deps.replyOnScreen()) {
      deps.closeReply();
      deps.settleFocus();
      return true;
    }
    notePlace();
    const here = deps.visitKey(currentPlace());
    // Passed over: what cannot be shown now, and the place the reader is at (`Visits.walk`).
    if (visits.walk(step, here, placeShowable, (place) => {
      showPlace(place);
      notePlace();
    })) {
      if (deps.screens().screen() === "main") deps.settleFocus();
      return true;
    }
    // Nothing noted behind, as after a reload or a change of channel that opened on something over a view
    // (the thread list, a thread opened from it): Back closes it as Esc does, onto what is under it.
    if (step < 0) {
      const levels = backLevels().filter((tag) => tag !== "reply" && !tag.startsWith("menu:"));
      if (levels.length > 0) {
        closeBackLevel(levels[levels.length - 1] as string);
        if (deps.screens().screen() === "main") deps.settleFocus();
        return true;
      }
    }
    deps.setStatus(step < 0 ? "Nothing to go back to." : "Nothing to go forward to.");
    return true;
  }


  // --- deep links: the address names the view, and can name a message (`#462`) -------------------
  // The address FOLLOWS, it never leads navigation, except at the open: told in the legacy section
  // head and in `core/navigation`'s `DeepLinks` header. The link's reading, focus, written address,
  // applied/applying, reader choices and sign-out epoch are that owner's; showing its place,
  // landing on its message and writing the address are here.

  // The link the page opened by: one owner in `core/navigation` (`createDeepLinks`).
  const deepLinks: NavDeepLinks = deps.createDeepLinks({
    search: () => deps.locationSearch(),
    // A layout the page itself restores (a turned or resized window) is no choice.
    arriving: () => visits.arriving() || deps.layoutRestoring(),
  });

  /**
   * Whether any of `row` shows in a list on screen: the cards where they cycle, else the list a column
   * shows, between what floats over its head and its foot. A mark scrolled out of sight is not what
   * Esc is asked about. (The linked mark itself stays in the legacy script; see NOTES.md.)
   */
  function linkedRowShown(row: HTMLElement): boolean {
    if (deps.screens().screen() !== "main" || deps.screens().view() !== "discord" || !deps.rowShown(row)) return false;
    if (row.parentNode === deps.leftEl("side-cards")) {
      if (!deps.cardsCycle()) return false;
      const box = row.getBoundingClientRect();
      const area = deps.leftEl("side-scroll").getBoundingClientRect();
      return box.bottom > area.top && box.top < area.bottom;
    }
    let shown = false;
    deps.eachColumn(() => {
      if (row.parentNode === deps.visibleList() && deps.rowOnScreen(row)) shown = true;
    });
    return shown;
  }

  /**
   * Note a choice of view or channel the reader made, for `applyDeepLink` to lose to. A layout the
   * page itself restores is no choice (the owner's `arriving` dep already says so).
   */
  function noteDeepLinkReaderMove(): void {
    deepLinks.noteReaderMove();
  }

  /** Whether the reader has chosen anywhere of their own since the link was first read. */
  function deepLinkReaderMoved(): boolean {
    return deepLinks.readerMoved();
  }

  /** A message's durable identity as a link names it: its service, channel, id, and thread. */
  function deepLinkRefOf(message: NavMessage): DeepLinkRef {
    const channel = String((message && message.channel_id) || deps.selectedChannelId() || "");
    const described = deps.providerNameOf(channel);
    const name = described ? described : deps.providerName();
    const thread = deps.threadOf(message);
    const record = message && message.thread;
    const root = !record || record.is_root === true || String(record.root_message_id || "") === String(message.id);
    return {
      provider: String(name || "").toLowerCase(), channel, message: String(message.id),
      thread: thread && !root ? thread : null,
    };
  }

  /**
   * The link the page opened by, read from the query once; null for a plain open. A reload standing
   * on an address this tab wrote is read the same way.
   */
  function deepLinkFromAddress(): NavDeepLinkWanted | null {
    return deepLinks.wanted();
  }

  /** A whole link to `ref`'s message, opening where it stands: its thread, or Main for a root. */
  function deepLinkFor(ref: DeepLinkRef): string {
    const place: NavChannelPlace = {
      screen: "main", channel: ref.channel, layout: "one",
      view: ref.thread ? "thread" : "main", thread: ref.thread, origin: null, side: null,
    };
    return `${deps.locationOrigin()}/?${deps.deepLinkQuery(place, ref)}`;
  }

  /**
   * Keep the address saying where the reader is: the channel place's query, replaced into the current
   * entry with its state untouched. Only a channel place is written — Settings, Help and the call
   * keep the channel's address under them, as a link copied there should still name it.
   */
  function syncDeepLinkAddress(): void {
    const history = backHistory();
    if (!history) return;
    const place = deps.screens().screen() === "main" ? channelPlace() : null;
    if (!place) return;
    const focused = deepLinks.focus();
    const focus = focused && focused.channel === place.channel &&
      (!focused.thread || place.thread === focused.thread || place.side === focused.thread ||
        place.view === "flat") ? focused : null;
    const query = deps.deepLinkQuery(place, focus);
    const address = `${deps.locationPathname() || "/"}?${query}`;
    const standing = `${deps.locationPathname() || "/"}${deps.locationSearch() || ""}`;
    // Only the standing entry's own address is compared: what another entry (behind Back, ahead of
    // Forward) was left saying is corrected when the reader stands on it again.
    if (address === standing) return;
    try {
      history.replaceState(history.state, "", address);
      deepLinks.noteWritten(address);
    } catch (_error) {
      // A host that refuses the address keeps the page; only its link is stale.
    }
  }

  /**
   * Land on the linked message where a row holds it, as the reply arrow lands on its answer. Over the
   * Threads view's cards, a thread's first message is its card: a link to it lands on the card, at the
   * head of the list, never on the hidden column under the cards nor on the root's row unpacked
   * beneath it.
   */
  function landOnDeepLink(id: string, center = false): boolean {
    const cardOf = (): HTMLElement | null => (deps.cardsCycle() ? [...deps.leftEl("side-cards").children]
      .find((one) => one.hasAttribute("data-card") && (one as HTMLElement & { cardRoot?: { id: unknown } }).cardRoot &&
        String((one as HTMLElement & { cardRoot?: { id: unknown } }).cardRoot?.id) === String(id)) as HTMLElement || null : null);
    let card = cardOf();
    if (card) {
      if (String(card.className).split(/\s+/).includes("search-hidden")) deps.setSearchOpen(false);
      // A card folded into a run of read threads: the run opened, which draws the cards anew.
      deps.openReadRun(card);
      card = cardOf() || card;
      // `#890 voice-page-linked-highlight`. Marked before it is brought on screen.
      deps.setLinkedMark(String(deps.selectedChannelId()), String(id));
      if (!deps.rowShown(card)) {
        deps.setStatus("Hide read is hiding that message.");
        return true;
      }
      const area = deps.leftEl("side-scroll");
      deps.finishPackSlide();
      deps.toPosition(area, deps.headOfList(area, card));
      deps.renderScrollTools();
      deps.setStatus("Here is the linked message's card.");
      return true;
    }
    const row = deps.rowHolding(id);
    if (!row) return false;
    if (String(row.className).split(/\s+/).includes("search-hidden")) deps.setSearchOpen(false);
    deps.openReadRun(row as HTMLElement);
    deps.setLinkedMark(String(deps.selectedChannelId()), String(id));
    deps.revealRow((deps.rowHolding(id) || row) as HTMLElement, center);
    // `#1224 mention-jump-in-view`. The landed row is the reader's place now: what arrives
    // above it after this (live turns, summaries, a page of older history) holds this row.
    if (deps.noteJumpAnchor) {
      const landed = (deps.rowHolding(id) || row) as HTMLElement;
      deps.noteJumpAnchor(landed, String(id));
    }
    deps.renderScrollTools();
    deps.setStatus("Here is the linked message.");
    return true;
  }

  /** Wait `waits` short waits for `ready`, as the seek walk waits for a read in flight. */
  async function waitForDeepLink(ready: () => boolean): Promise<boolean> {
    for (let waited = 0; waited < DEEP_LINK_WAITS && !ready(); waited += 1) {
      await deps.delay(deps.pullWaitMs());
    }
    return ready();
  }

  /**
   * Open where the link says, once: its channel over the restored one, its view over the channel's
   * chosen one, its message landed on by the reply arrow's walk. Best-effort in every part, said in
   * the status line where it cannot be done: an unreadable part never breaks the open.
   */
  async function applyDeepLink(): Promise<void> {
    const wanted = deepLinkFromAddress();
    if (!wanted || deepLinks.applied() || deepLinks.applying()) return;
    const epoch = deepLinks.epoch();
    deepLinks.setApplying(true);
    try {
      // Until the channel itself is on screen: applied over sign-in or the call view, the page's own
      // open would simply overwrite it. Giving up here is not final — the next place noted kicks this
      // again (`scheduleBackSync`); applied is only ever set past this wait.
      if (!await waitForDeepLink(() => Boolean(deps.token()) && deps.channels().length > 0 &&
          Boolean(deps.selectedChannelId()) && deps.screens().screen() === "main" && deps.screens().view() === "discord")) return;
      deepLinks.markApplied();
      // A reader who chose a channel or a view while that wait ran is not moved at all.
      if (epoch !== deepLinks.epoch() || deepLinkReaderMoved()) return;
      const wantedChannel = (wanted.message && wanted.message.channel) || wanted.channel;
      if (wantedChannel) {
        if (!deps.channels().some((channel) => String(channel.id) === wantedChannel)) {
          deps.setStatus("That link names a channel this server does not read.");
          syncDeepLinkAddress();
          return;
        }
        if (String(deps.selectedChannelId()) !== wantedChannel) {
          deps.chooseChannel(wantedChannel);
          // The link's own arrival on its channel is no choice of the reader's.
          deepLinks.arrive(() => changeSelectedChannel());
        }
      }
      if (deps.screens().view() !== "discord") showView("discord");
      const channel = String(deps.selectedChannelId());
      const startPlace = channelPlace();
      const startVisit = visits.position();
      // The channel's own first read decides its opening view as it lands; a place put on before it
      // would be decided over. Wait for that read, then put the link's place on — unless the reader
      // has navigated for themselves meanwhile.
      await waitForDeepLink(() => String(deps.selectedChannelId()) !== channel ||
        (deps.readsStarted() > 0 && deps.viewsHas(deps.viewKey())));
      if (epoch !== deepLinks.epoch() || String(deps.selectedChannelId()) !== channel) return;
      const thread = wanted.thread || (wanted.message && wanted.message.thread) || null;
      let place: NavChannelPlace | null = null;
      if (wanted.view === "threads") {
        place = { screen: "main", channel, layout: "list", view: null, thread: null, origin: null, side: null };
      } else if (wanted.view === "columns") {
        place = { screen: "main", channel, layout: "two", view: null, thread: null, origin: null, side: thread };
      } else if (wanted.view === "thread" || (!wanted.view && thread)) {
        if (thread) place = { screen: "main", channel, layout: "one", view: "thread", thread, origin: "main", side: null };
      } else if (wanted.view === "all") {
        place = { screen: "main", channel, layout: "one", view: "flat", thread: null, origin: null, side: null };
      } else if (wanted.view === "main") {
        place = { screen: "main", channel, layout: "one", view: "main", thread: null, origin: null, side: null };
      }
      // The reader moved while the read was coming: the link loses, quietly.
      const stood = channelPlace();
      const moved = deepLinkReaderMoved() ||
        visits.position().at !== startVisit.at || visits.position().length !== startVisit.length ||
        (startPlace && stood && deps.visitKey(stood) !== deps.visitKey(startPlace));
      if (moved && !(place && stood && deps.visitKey(stood) === deps.visitKey(place))) return;
      // Already there: showing it again would only redraw and re-read under a settled reader.
      const standing = Boolean(place && stood && deps.visitKey(stood) === deps.visitKey(place));
      if (!standing && place && channelPlaceShowable(place)) {
        showChannelPlace(place);
      } else if (!standing && place && place.view === "thread" && place.thread) {
        // Not yet known is not gone: a thread named before its channel was read opens by id.
        deps.chooseOneColumn({ view: "thread", thread: place.thread, summary: null, origin: "main" });
      } else if (!standing && place) {
        deps.setStatus("That link's view cannot be shown here; showing the channel instead.");
      }
      syncDeepLinkAddress();
      if (wanted.view === "voice") {
        deps.setStatus("Links never start a call.");
      }
      if (!wanted.message) return;
      deepLinks.setFocus(wanted.message);
      syncDeepLinkAddress();
      const movesBeforeSeek = deepLinks.readerMoves();
      // The first read of the view now open, waited for as a seek waits for All to land.
      await waitForDeepLink(() => String(deps.selectedChannelId()) !== channel ||
        (deps.viewsHas(deps.viewKey()) && !deps.readsBusy()));
      if (epoch !== deepLinks.epoch() || String(deps.selectedChannelId()) !== channel) return;
      // Chosen while the read landed: the seek would walk views the reader just left.
      if (deepLinks.readerMoves() !== movesBeforeSeek) {
        deepLinks.setFocus(null);
        syncDeepLinkAddress();
        return;
      }
      const target = wanted.message.message;
      await deps.seekMessage(target, () => landOnDeepLink(target),
        () => deps.threading() && deps.selection().view !== "flat" && deps.answeredElsewhere(target));
      if (!deps.rowHolding(target)) {
        deepLinks.setFocus(null);
        syncDeepLinkAddress();
      }
    } finally {
      deepLinks.setApplying(false);
    }
  }

  /** Copy this message's vibe-talk link, and say whether it went. The ⋯ menu's third copy. */
  async function copyDeepLink(message: NavMessage): Promise<boolean> {
    const copied = await deps.writeClipboardText(deepLinkFor(deepLinkRefOf(message)));
    deps.setStatus(copied ? "Copied the vibe-talk link." : deps.copiedRefusedText());
    return copied;
  }

  // --- in-app message links (`#543`, `#580`) -------------------------------------------------------
  // THE SERVER RECOGNIZES, THE PAGE DECIDES; THE JUMP walks as a `#462` link's open does; BACK
  // COMES BACK as one `#304` level: told in the legacy section head. The switch (`inAppLinks`,
  // `core/settings`) and Settings' presentation of it stay in the legacy script (NOTES.md); this
  // factory reads the switch through `deps.inAppLinksOn()`.

  /** Whether a link to `ref` is this app's now: the switch on, and its channel one this server reads. */
  function keepsInApp(ref: MessageLinkRef | null): boolean {
    return deps.inAppLinksOn() && Boolean(ref) && deps.channels().some((channel) => String(channel.id) === (ref as MessageLinkRef).channel);
  }

  /** The words for a link to `ref` whose text was only its address: where it goes, in this app's names. */
  function messageLinkLabel(ref: MessageLinkRef): string {
    const what = ref.message ? (ref.thread ? "reply" : "message") : ref.thread ? "thread" : null;
    const channel = `#${deps.channelLabel(ref.channel)}`;
    return what ? `${channel} › ${what}` : channel;
  }

  /**
   * Make the links to messages in a body just drawn this app's, where they are: each one marked by
   * the server is given a tap that jumps, decided again when it is tapped, and one this app takes is
   * marked for the stylesheet and, if its text was its address, named.
   */
  function wireMessageLinks(box: Element): void {
    const walk = (node: Element): void => {
      for (const kid of node.children) {
        if (String(kid.tagName).toLowerCase() !== "a") {
          walk(kid);
          continue;
        }
        const docKey = kid.getAttribute("data-doc");
        if (docKey) {
          kid.addEventListener("click", deps.inThisColumn((event: Event) => {
            const tap = event as { ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean; button?: number; preventDefault?: () => void };
            if (tap.ctrlKey || tap.metaKey || tap.shiftKey || tap.altKey || (tap.button || 0) > 0) return;
            if (typeof tap.preventDefault === "function") tap.preventDefault();
            jumpToDocumentLink(docKey, kid);
          }));
          kid.setAttribute("data-in-app", "");
          kid.setAttribute("title", `Opens here — ${String(kid.getAttribute("href") || "")}`);
        }
        const ref = messageLinkRef(kid.getAttribute("data-msg"));
        if (!ref) continue;
        kid.addEventListener("click", deps.inThisColumn((event: Event) => onMessageLinkTap(ref, event as {
          ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean; button?: number;
          preventDefault?: () => void; stopPropagation?: () => void;
        })));
        if (!keepsInApp(ref)) continue;
        const href = String(kid.getAttribute("href") || "");
        kid.setAttribute("data-in-app", "");
        if (deps.renderedText(kid) === href) {
          const label = messageLinkLabel(ref);
          kid.replaceChildren(document.createTextNode(label));
        }
        kid.setAttribute("title", `Opens here — ${href}`);
      }
    };
    walk(box);
  }

  /**
   * A tap on a link to a message: a jump, when the link is this app's and nothing asks for another
   * tab. Otherwise the browser follows the address, as for any link.
   */
  function onMessageLinkTap(ref: MessageLinkRef, event: {
    ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean; button?: number;
    preventDefault?: () => void; stopPropagation?: () => void;
  } | null): void {
    if (!keepsInApp(ref)) return;
    if (event && (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || (event.button || 0) > 0)) return;
    if (event && typeof event.preventDefault === "function") event.preventDefault();
    if (event && typeof event.stopPropagation === "function") event.stopPropagation();
    void deps.guardQuietly(jumpToMessageLink)(ref);
  }

  // The jumps taken by links: one owner in `core/navigation` (`createLinkJumps`).
  const linkJumps: NavLinkJumps = deps.createLinkJumps();

  /** Where the reader is, to come back to: the place, the column, and the message their eye is on. */
  function linkJumpSpot(): NavLinkSpot {
    const column = deps.inSide() ? "side" : "main";
    const place = channelPlace();
    const mark = deps.captureScroll();
    const anchor = mark.anchor;
    return {
      place, column, pinned: mark.pinned,
      anchor: anchor ? anchor.getAttribute("data-context-id") || deps.idsOf(anchor as HTMLElement)[0] || null : null,
      offset: anchor ? mark.top - deps.listPlaceTop(el("scroll-area")) : 0,
      focus: deepLinks.focus(),
      call: false,
    };
  }

  /** The levels a Back would close, with the jumps taken by links among them (`LinkJumps.levels`). */
  function withLinkJumps(levels: string[]): string[] {
    return linkJumps.levels(levels);
  }

  /**
   * Where to come back to from a document link tapped at `from`
   * (`#1082 documents-back-chain`): the jump spot, with its anchor the linking
   * message itself where the row holding the link can be named, at its height
   * now — so Back puts that message back on screen, not merely near it.
   */
  function documentLinkSpot(from: Element | null): NavLinkSpot {
    const base = linkJumpSpot();
    let node: Element | null = from;
    for (let depth = 0; node && depth < 12; depth += 1) {
      let id: string | null = null;
      try {
        id = node.getAttribute("data-context-id") || deps.idsOf(node as HTMLElement)[0] || null;
      } catch (_error) {
        id = null;
      }
      if (id) {
        let offset = base.offset;
        try {
          offset = node.getBoundingClientRect().top - deps.listPlaceTop(el("scroll-area"));
        } catch (_error) {
          // Unmeasured, the eye's own anchor height is still the spot to keep.
        }
        return { ...base, pinned: false, anchor: id, offset };
      }
      node = node.parentElement || (node.parentNode as unknown as Element | null);
    }
    return base;
  }

  /**
   * Open document `key` from a message (`#1082 documents-back-chain`): a link
   * jump like a message link's — the spot taken first, the jump one level
   * from the tap, the document the level it landed on — so Back returns to
   * the channel and the linking message instead of the documents list. Off
   * the channel's view there is no spot to take: the document just opens, as
   * it always did.
   */
  function jumpToDocumentLink(key: string, from?: Element | null): void {
    // Opening is the left column's act, wherever the link was tapped
    // (`#1336 doc-back-side-thread-hidden`), as `jumpToMessageLink`'s jump
    // is: in the right column `showView`'s `el("pane-discord")` is
    // `#side-pane`, so opening from a side thread there hid that thread
    // instead of the channel's pane, and nothing put it back. The spot is
    // taken first, in the tapped column; only the open moves.
    const openIt = (): void => deps.inColumn(deps.mainColumn(), () => deps.openDocument(key));
    if (deps.screens().screen() !== "main" || deps.screens().view() !== "discord") {
      openIt();
      return;
    }
    const spot = documentLinkSpot(from ?? null);
    const under = linkJumps.top();
    // A jump still under way when this one is taken: what it had opened is its own.
    if (under && under.landed === null) under.landed = screenLevelsNow();
    const jump: NavLinkJump = { spot, below: backLevels(), landed: null, doc: key };
    linkJumps.take(jump);
    scheduleBackSync();
    try {
      openIt();
    } finally {
      // Landed, in the reader or on its failure: what is open now — the
      // document's levels — is the jump's, and a further document opened
      // from it is a level over the jump.
      if (jump.landed === null && linkJumps.holds(jump)) {
        jump.landed = screenLevelsNow();
        scheduleBackSync();
      }
    }
  }

  /** Wait for the channel on screen to have read the view it shows, or to be left; false when it was left. */
  async function linkJumpRead(channel: string, ticket: number): Promise<boolean> {
    await waitForDeepLink(() => !linkJumps.current(ticket) || String(deps.selectedChannelId()) !== channel ||
      (deps.readsStarted() > 0 && deps.viewsHas(deps.viewKey()) && !deps.readsBusy()));
    return linkJumps.current(ticket) && String(deps.selectedChannelId()) === channel;
  }

  /** Choose channel `channel` as a link's arrival, which is no choice of the reader's (`#462`). */
  function arriveOnChannel(channel: string): void {
    deps.chooseChannel(channel);
    deepLinks.arrive(() => changeSelectedChannel());
  }

  /** Put channel place `place` on as an arrival: by the views' own choosing, or a thread by its id. */
  function arriveAtPlace(place: NavChannelPlace): void {
    if (deps.visitKey(channelPlace()) === deps.visitKey(place)) return;
    if (channelPlaceShowable(place)) {
      showChannelPlace(place);
    } else if (place.view === "thread" && place.thread) {
      // Not yet known is not gone: opened by id, as a link's thread is, and named by the read after.
      deepLinks.arrive(() => deps.chooseOneColumn({ view: "thread", thread: place.thread as string, summary: null, origin: place.origin || "main" }));
    }
  }

  /**
   * Jump to what a link names: see THE JUMP in the section head. The reader's spot is taken first,
   * and the jump is a level from the start, so its entry is pushed in the tap's own task and a Back
   * at any point undoes it.
   */
  async function jumpToMessageLink(ref: MessageLinkRef): Promise<void> {
    if (deps.screens().screen() !== "main" || deps.screens().view() !== "discord") return undefined;
    // Where the reader is, in the column the link was tapped in; the jump itself is the left column's.
    const spot = linkJumpSpot();
    return deps.inColumn(deps.mainColumn(), () => jumpFromSpot(ref, spot));
  }

  /**
   * Jump to the message a sent card names (`#580 open-in-chat-jump`), from the call's view or the
   * channel's: a link's jump (`jumpFromSpot`), landing in the view Settings names (All unless the
   * reader chose Main) with the message centred and lit. From the call's view the channel's view
   * comes up as part of the jump, and its Back is the call's view again.
   */
  async function jumpToSentMessage(ref: MessageLinkRef): Promise<void> {
    if (deps.screens().screen() !== "main") return undefined;
    const fromDocs = deps.screens().view() === "docs";
    const fromCall = deps.screens().view() !== "discord" && !fromDocs;
    const spot: NavLinkSpot = fromDocs
      ? { place: null, column: "main", pinned: false, anchor: null, offset: 0, focus: deepLinks.focus(), docs: true }
      : fromCall
        ? { place: null, column: "main", pinned: false, anchor: null, offset: 0, focus: deepLinks.focus(), call: true }
        : linkJumpSpot();
    const enter = fromCall || fromDocs ? async (): Promise<void> => {
      deps.settleUiState();
      showView("discord");
      // A channel the jump changes to is read as it is chosen; the one already chosen is read here.
      if (String(deps.selectedChannelId()) === ref.channel) await readEnteredChannel(deps.viewRestored());
    } : null;
    return deps.inColumn(deps.mainColumn(), () => jumpFromSpot(ref, spot, { view: deps.storedOpenInView(), center: true, enter }));
  }

  /**
   * `jumpToMessageLink` from `spot`, in the left column. `options.view` (`#580`) is the view to land
   * a message in, "flat" for All or "main", where the channel's layout leaves the choice to the jump:
   * never over two columns, which keep their own. `options.center` centres the message rather than
   * putting it under the top. `options.enter` runs once the jump is a level, before anything else.
   */
  async function jumpFromSpot(ref: MessageLinkRef, spot: NavLinkSpot, options: {
    view?: "flat" | "main"; center?: boolean; enter?: (() => Promise<void>) | null;
  } = {}): Promise<void> {
    const here = String(deps.selectedChannelId() || "");
    if (ref.channel === here && !ref.message && !ref.thread) {
      deps.setStatus("That link is to this channel.");
      return;
    }
    const under = linkJumps.top();
    // A jump still under way when another is taken: what it had opened by now is its own.
    if (under && under.landed === null) under.landed = screenLevelsNow();
    // From the call's view nothing is open under the jump: what the channel's view shows is the jump's.
    const jump: NavLinkJump = { spot, below: spot.call ? [] : backLevels(), landed: null };
    const ticket = linkJumps.take(jump);
    scheduleBackSync();
    try {
      if (options.enter) await options.enter();
      if (!linkJumps.current(ticket)) return;
      if (ref.channel !== here) {
        arriveOnChannel(ref.channel);
        if (!await linkJumpRead(ref.channel, ticket)) return;
      }
      const channel = ref.channel;
      const now = channelPlace();
      let place: NavChannelPlace | null = null;
      const want = deps.threading() && ref.message && !(now && now.layout === "two") ? options.view || null : null;
      if (want === "flat") {
        // All: every message in the order sent, a reply among them, so no thread is opened.
        place = { screen: "main", channel, layout: "one", view: "flat", thread: null, origin: null, side: null };
      } else if (ref.thread && deps.threading()) {
        const origin = want === "main" ? "main"
          : now && now.layout === "one" && (now.view === "main" || now.view === "flat") ? now.view
            : now && now.view === "thread" && (now.origin === "main" || now.origin === "flat") ? now.origin
              : deps.defaultChannelView();
        place = { screen: "main", channel, layout: "one", view: "thread", thread: ref.thread, origin, side: null };
      } else if (want === "main") {
        place = { screen: "main", channel, layout: "one", view: "main", thread: null, origin: null, side: null };
      } else if (ref.message && now && (now.layout === "list" || (now.layout === "one" && now.view === "thread"))) {
        // A root message is on the channel's own timeline: the view a thread was opened from, or the
        // channel's own, where the thread list or a thread cannot show it.
        const view = now.layout === "one" && (now.origin === "main" || now.origin === "flat") ? now.origin : deps.defaultChannelView();
        place = { screen: "main", channel, layout: "one", view: view === "flat" ? "flat" : "main", thread: null, origin: null, side: null };
      }
      if (place) {
        arriveAtPlace(place);
        if (!await linkJumpRead(channel, ticket)) return;
      }
      if (!ref.message) {
        deps.setStatus(ref.thread ? "Here is the linked thread." : `Here is #${deps.channelLabel(channel)}.`);
        return;
      }
      const name = deps.providerNameOf(channel);
      deepLinks.setFocus({ provider: String(name || ref.service).toLowerCase(), channel, message: ref.message, thread: ref.thread });
      const target = ref.message;
      // A Back while the walk looks further back ends it: nothing lands over the place put back.
      await deps.seekMessage(target, () => linkJumps.current(ticket) && landOnDeepLink(target, Boolean(options.center)),
        () => linkJumps.current(ticket) && deps.threading() && deps.selection().view !== "flat" && deps.answeredElsewhere(target));
      if (linkJumps.current(ticket) && !deps.rowHolding(target)) deepLinks.setFocus(null);
    } finally {
      // Landed, or given up: what is open now is the jump's, and what opens after is over it.
      if (jump.landed === null && linkJumps.holds(jump)) {
        jump.landed = screenLevelsNow();
        scheduleBackSync();
      }
    }
  }

  /** Undo the last jump: its level goes at once, and the reader is put back where they were. */
  function returnFromLinkJump(): void {
    const jump = linkJumps.undo();
    if (!jump) return;
    // A walk the jump's seek is still making is over: its next step finds another ticket.
    deps.bumpReplyJump();
    linkJumps.returnStarted();
    void deps.guardQuietly(returnToLinkSpot)(jump.spot).finally(() => {
      linkJumps.returnEnded();
      scheduleBackSync();
    });
  }

  /**
   * Put the reader back on `spot`: its channel, its place, and the message their eye was on, at the
   * same height — walked back to if it is no longer in the list, as a link's message is.
   */
  async function returnToLinkSpot(spot: NavLinkSpot): Promise<void> {
    const ticket = linkJumps.nextTicket();
    // A jump from the Documents list goes back to it, at its scroll place (kept per view).
    if (spot.docs) {
      if (deps.screens().screen() !== "main" || deps.screens().view() === "docs") return;
      deps.settleUiState();
      showView("docs");
      deps.stopDiscordPolling();
      deps.setStatus("Back on the Documents list.");
      return;
    }
    // `#580 open-in-chat-jump`. A jump from the call's view goes back to it, where the reader was.
    if (spot.call) {
      if (deps.screens().screen() !== "main" || deps.screens().view() === "voice") return;
      deps.settleUiState();
      showView("voice");
      deps.stopDiscordPolling();
      deps.setStatus("Back on the call's view.");
      return;
    }
    const place = spot.place;
    if (!place || deps.screens().screen() !== "main") return;
    // `#1082 documents-back-chain`. A jump into a document comes back from the
    // Documents view: close its chain (each close is one step, so close until
    // none is open), put the channel's view back up, and read the channel it
    // names where it is already the chosen one, as a jump entering it does.
    if (deps.screens().view() === "docs") {
      for (let step = 0; step < 16 && deps.documentOpen(); step += 1) deps.closeDocument();
      deps.settleUiState();
      showView("discord");
      if (String(deps.selectedChannelId() || "") === place.channel) {
        await readEnteredChannel(deps.viewRestored());
      }
    } else if (deps.screens().view() !== "discord") return;
    if (!deps.channels().some((channel) => String(channel.id) === place.channel)) return;
    if (String(deps.selectedChannelId()) !== place.channel) {
      arriveOnChannel(place.channel);
      if (!await linkJumpRead(place.channel, ticket)) return;
    }
    arriveAtPlace(place);
    if (!await linkJumpRead(place.channel, ticket)) return;
    deepLinks.setFocus(spot.focus);
    syncDeepLinkAddress();
    const column = spot.column === "side" && deps.sideShowing() ? deps.sideColumn() : deps.mainColumn();
    const put = (): boolean => deps.inColumn(column, () => putBackOnSpot(spot));
    if (spot.pinned || !spot.anchor) {
      if (spot.pinned) deps.inColumn(column, deps.scrollToNewest);
      deps.setStatus("Back where you were.");
      return;
    }
    if (put()) return;
    const anchor = spot.anchor;
    await deps.inColumn(column, () => deps.seekMessage(anchor, () => linkJumps.current(ticket) && putBackOnSpot(spot),
      () => deps.threading() && deps.selection().view !== "flat" && deps.answeredElsewhere(anchor)));
  }

  /** Put the row `spot` names at the height it was, in the column being worked on; false when it is not drawn. */
  function putBackOnSpot(spot: NavLinkSpot): boolean {
    const row = [...deps.visibleList().children].find((candidate) =>
      candidate.getAttribute("data-context-id") === spot.anchor || deps.idsOf(candidate as HTMLElement).includes(String(spot.anchor)));
    if (!row) return false;
    deps.restoreScroll({ pinned: false, anchor: row as HTMLElement, top: deps.listPlaceTop(el("scroll-area")) + spot.offset });
    deps.setStatus("Back where you were.");
    return true;
  }

  // --- entering a channel, and changing it -----------------------------------------------------

  /** The view switch, pressed: rotate Voice -> Channel -> Documents -> Voice (`documents-view-client`). */
  function pressViewSwitch(): void {
    // The selector owns the rotation and, through `chooseMajorView`, each view's
    // entry work: entering the channel re-reads it (keeping the position on a
    // return, as the two-way switch did), and any other view stops its polling.
    deps.cycleView();
  }

  /** Read the channel just entered, and keep reading it while it stays on screen. */
  function readEnteredChannel(keepPosition: boolean): Promise<void> {
    return deps.guardQuietly(async () => {
      try {
        // A return finds the channel's store as it was left, so it asks only for what changed.
        const since = deps.readsStarted();
        await deps.loadDiscord(keepPosition ? { keepPosition: true, reason: "visible" } : { reason: "enter" });
        // ...and the right column's thread, or its list of threads. `#229 desktop-two-column`.
        await deps.readSideColumn(keepPosition ? "visible" : "enter", since);
      } finally {
        // Armed after a failure too: offline is exactly when the reader needs the page to try again.
        if (deps.screens().view() === "discord") {
          deps.scheduleDiscordPoll();
        }
      }
    })();
  }
  // Both wrapped in a lambda rather than passed straight through: a DOM listener is handed the
  // EVENT as its first argument, and `loadDiscord`'s first argument is its options object. Reading
  // `keepPosition` off a MouseEvent happens to answer false, which is the right answer by accident
  // and stops being right the moment another option is added.
  //
  // Refresh keeps your place. It is a re-read of the channel you are already looking at, and being
  // thrown to the bottom of it is the same defect as a background poll doing so — the fact that you
  // asked for fresh messages is not a request to stop reading the one in front of you. A reader who
  // was at the bottom still follows, which is the case where "keep my place" and "show me the
  // newest" are the same instruction.
  // Changing channel is not a re-read; it is a different history, and its bottom is where to start.
  // The walk back resets with it: a cursor from one channel means nothing in another, and carrying
  // one across would ask the server to step back from a message that is not there.
  function changeSelectedChannel(): Promise<void> {
    // From the left column, whichever column the key that asked for it was in. `#229 desktop-two-column`.
    if (deps.inSide()) return deps.inColumn(deps.mainColumn(), changeSelectedChannel);
    noteDeepLinkReaderMove();
    // `#277 reply-in-column`. A reply open in a column answers a message of the channel being left.
    deps.putDockedReplyAway();
    // `#215 reply-coalesce`. Gathered replies belong to the channel they were gathered in, whose rows
    // are about to go — and so does a tap on an arrow still waiting to jump, which would otherwise walk
    // this channel's history for a message of the last one.
    deps.gatherState().gathered = null;
    deps.listState().rowsGathered = false;
    deps.dropReplyArrowTap();
    deps.applyChannelProvider();
    // This window's, and the one a new window opens on. `#265 window-channel-memory`.
    deps.rememberActiveChannel(true);
    deps.rememberChannelDraft();
    // `#205 channel-view-memory`. Another channel opens in the view the reader chose there, and in ITS
    // default — All where it has threads — when they chose none: a choice made in the last channel is
    // that channel's, and it waits there for the reader to come back. The first read is the view's.
    deps.settleUiState();
    deps.openChannelView();
    deps.forgetThreadPresence();
    deps.timelineRows().messages = [];
    deps.timelineRows().threads = [];
    deps.clearChannelContexts();
    // `#396 thread-color-memory`. The channel's own colours, as this device last drew them.
    deps.restoreThreadHues(deps.selectedChannelId());
    // `#427 server-thread-hues`: and offered to the server, as at sign-in — issued before this
    // channel's first read below, not awaited by it.
    void deps.seedServerThreadHues(deps.selectedChannelId());
    el("thread-list").replaceChildren();
    deps.restoreChannelComposer();
    deps.olderPages().moreAbove = false;
    deps.olderPages().cursor = null;
    deps.payReadCursor();
    deps.listState().newestId = null;
    el("discord-log").replaceChildren();
    // The summary goes with the rows it is about. It is written only inside `loadDiscord`, which
    // THROWS when the read fails — so leaving it standing means a failed change of channel shows
    // the previous channel's name and a count of messages that are no longer on the screen, which
    // is the most confident possible way of being wrong.
    el("channel-summary").replaceChildren();
    deps.renderOlderControl();
    deps.loadState().freshAt = 0;
    deps.loadState().freshness = "fresh";
    // `#18 offline-message-cache`. The new channel as this device last saw it, until the read below
    // answers — which then merges into those rows rather than replacing them.
    const saved = deps.hydrateChannelScope();
    deps.renderChannelFreshness();
    if (saved) deps.scrollToNewest();
    deps.saveUiState();
    // `#206 pin-message`. Under the Pinned filter the new channel's pins are what the reader is about
    // to look at, so they are read now rather than after its first read reports their revision.
    if (deps.pinsOnly()) {
      deps.ensurePins();
      deps.renderChannelRows();
    }
    // `#229 desktop-two-column`. The right column shows this channel's thread selected earlier in
    // the session, or its cards; and a channel without threads is one column.
    deps.resetSideColumn(true);
    deps.renderColumns();
    // A stream follows ONE channel, and a cursor from the old one means nothing in the new one —
    // the same reason the walk-back cursor is dropped two lines above.
    deps.startChannelStream(deps.selectedChannelId());
    return deps.loadDiscord(saved && deps.screens().view() === "discord" ? { keepPosition: true, reason: "enter" } : { reason: "enter" });
  }

  function showView(name: string): void {
    // `#277 reply-in-column`. The call's view is one list, with no column for a reply to be open in.
    if (name !== "discord") deps.putDockedReplyAway();
    // Written down BEFORE the panes swap, because after that the scroll position belongs to
    // whichever pane is now up and says nothing about where the reader was.
    const leaving = deps.screens().view();
    if (leaving && leaving !== name) {
      const area = el("scroll-area");
      deps.viewScroll().leave(leaving, { top: area.scrollTop, atNewest: deps.atBottom(area) });
      if (leaving === "discord") deps.setViewMark(leaving, deps.captureScroll());
      // ...and, leaving the channel, the message the reader was on, for a reopen. `#189 restore-ui-state`.
      if (leaving === "discord") deps.holdChannelPlace();
    }
    deps.screens().setView(name);
    el("pane-voice").hidden = name !== "voice";
    el("pane-discord").hidden = name !== "discord";
    el("pane-docs").hidden = name !== "docs";
    el("docs-fetch").hidden = name !== "docs";
    // `#269 approval-card-inline`. A waiting draft is the next message of the list now on screen, put
    // there BEFORE the place in that list is restored, which counts it.
    deps.placePostCard();
    const discord = name === "discord";
    // `#845 voice-toggle-colours`. Pressed, lit in the accent, is the call's view; the channel's is the darker one.
    el("view-switch").setAttribute("aria-pressed", name === "voice" ? "true" : "false");
    el("view-switch-label").textContent = discord ? "Channel" : name === "docs" ? "Documents" : "Voice";
    el("view-switch").title = discord
      ? "Showing the channel. Press to show the documents."
      : name === "docs"
        ? "Showing the documents. Press to show the call."
        : "Showing the call. Press to show the channel.";
    deps.syncDocumentsSelector();
    deps.syncDiscussView();
    // Both panes share ONE scroll container, so a switch swaps the content out from under a
    // scrollTop that belonged to the other pane. On a FIRST entry, landing on the newest message is
    // right and landing at the top is badly wrong: arriving at the top of a long channel means
    // scrolling past everything already read to reach the thing you opened it for.
    //
    // On every entry AFTER that, it is the reader's place, and throwing it away is the same defect
    // one level up. A glance at the transcript and back should not cost forty messages of scrolling.
    //
    // ...unless the place they left was THE NEWEST LINE, which is an instruction rather than an
    // offset: turns can land in the transcript while the reader is over on the channel, and putting
    // them back at the number the bottom used to be is the same "your place moved" defect wearing the
    // fix's clothes. See `viewScroll`.
    const top = deps.viewScroll().enter(name);
    const returning = top !== null;
    if (returning) {
      const mark = name === "discord" ? deps.viewMark(name) : undefined;
      if (mark && mark.anchor) deps.restoreScroll(mark);
      else deps.toPosition(el("scroll-area"), top);
    } else {
      deps.scrollToNewest();
    }
    // Only when the reader really was taken to the newest message. Clearing it on a RETURN would
    // hide a chip that is pointing at something they have not seen.
    if (!returning) {
      deps.forgetJumpNewest(name);
    }
    // Told to the caller (`viewScroll.restored()`, kept by `enter` above), because restoring the position
    // here is only half of it: entering the channel also RE-READS it, and a read that does not know it is a
    // return settles to the newest message and undoes this immediately. See the view-switch handler.
    // The chips belong to the list you are looking at, and you are now looking at a different one.
    deps.renderScrollTools();
    deps.renderChannelFreshness();
    // A channel word ("fetching the channel…") is not the Documents reader's news.
    if (name === "docs") deps.dismissStatus();
    // Leaving the channel takes the reading with it: audio that goes on playing over the transcript
    // is audio the reader cannot see the source of, on a list where the next thing it does is
    // archive a message.
    // `#1039 documents-read-parity`: kind-aware. Entering Documents stops a message read (and
    // clears its mode) but keeps a document read — that view is its source; entering the channel
    // or the call stops a document read, whose strip and source are the reader's.
    const readOwner = deps.readAloudOwner();
    const nowReading = readOwner.nowPlaying;
    const documentReading = Boolean(
      (nowReading && nowReading.kind === "document") ||
      (!nowReading && typeof readOwner.pendingRead === "string" && readOwner.pendingRead.startsWith("doc:")));
    if (!discord) {
      readOwner.readingMode = false;
      if (name !== "docs" || !documentReading) readOwner.stopReading();
    } else if (documentReading) {
      readOwner.stopReading();
    }
    // Read replaces Talk in this view, so the control pane has to be re-decided on every switch.
    deps.renderCallControls();
    // ...and so does the channel picker, which is a member of the bar now rather than a row at the
    // top of the scrollback. LAST, and after `currentView` is set, for the same reason `showScreen`
    // ends this way: the bar decides what it shows from the view that is now up. `#83
    // channel-selector-in-bar`.
    deps.renderControlBar();
    deps.renderChannelNavigation();
    // `#229 desktop-two-column`. One column or two is the channel's layout.
    deps.renderColumns();
    // `#245 done-feedback`. The chips stand on the channel's message box, which only the channel has.
    deps.placeChipsOverComposer();
    // The call's controls (Sound, Clear, Talk) are not the Documents view's.
    // `#1505 documents-read-controls-highlight-tap`: while a document read
    // stands (playing or pending) the pane stays, so its Read is the read's
    // Pause and its pace the read's — the strip steers but cannot pause.
    if (deps.screens().screen() === "main") el("control-pane").hidden = name === "docs" && !documentReading;
    deps.saveUiState();
    // `#224 keyboard-shortcuts`. The list on screen takes the focus, so that the scroll keys move it.
    // Above all off the view switch: left holding the focus from the click that pressed it, it took
    // the next Space as another press and switched straight back. Only on the main screen: a view
    // changed behind Settings must not take the focus from the control the reader is on there.
    if (leaving !== name && deps.screens().screen() === "main") deps.settleFocus();
    // `#304 back-gesture-in-app`. The call's view waits in front of whatever the channel had open.
    scheduleBackSync();
  }

  return {
    backStack, visits, deepLinks, linkJumps,
    backHistory, historyIndex, mayPushHistory, openMenus, closeOpenMenu, backLevels, screenLevelsNow,
    closeSideByX, closeBackLevel, scheduleBackSync, onBackPop, startBackStack,
    channelPlace, currentPlace, notePlace, channelPlaceShowable, placeShowable, showChannelPlace,
    showPlace, placeFromKeyboard,
    noteDeepLinkReaderMove, deepLinkReaderMoved, deepLinkRefOf, deepLinkFromAddress, deepLinkFor,
    syncDeepLinkAddress, linkedRowShown, landOnDeepLink, waitForDeepLink, applyDeepLink, copyDeepLink,
    keepsInApp, messageLinkLabel, wireMessageLinks, onMessageLinkTap, linkJumpSpot, withLinkJumps,
    arriveOnChannel, arriveAtPlace, jumpToDocumentLink, documentBack, jumpToMessageLink, jumpToSentMessage, returnFromLinkJump,
    putBackOnSpot, pressViewSwitch, readEnteredChannel, changeSelectedChannel, showView,
  };
}

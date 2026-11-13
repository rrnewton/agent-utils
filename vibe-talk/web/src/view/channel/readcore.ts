// The channel's timeline as the page reads and draws it (`#203 incremental-refresh`, `#220
// view-switch-instant`, `#226 page-energy-profile`, `#308 live-reply-in-main`, `#310
// stale-live-read-flicker`, `#316 late-counts-resurrect`; `#624 voice-page-modularity`, moved from
// the legacy script into view/channel; mu-readcore): the refresh (a full read or a chain of
// deltas), the step back, the store drawn with no request, the rows a view switch draws from
// what the store already holds, and All read behind Main for Main's counts. The wire itself —
// one read, when it was sent, in what order — is the reader's (core/sync `createTimelineReader`);
// what a landed page does to the reader's place, the chips and the other column is the page's,
// passed in. One instance for the whole page: the column is resolved on every call, and captured
// explicitly across a wait (`columnSteps`), never read ambiently after one.

import type { Message, ThreadSummary, TimelineResponse } from "../../contract/index.ts";
import {
  answerUnplaced,
  DISCORD_PAGE_LIMIT,
  exactReplyCount,
  foldDeltaIn,
  foldLiveIn,
  foldPageIn,
  heldNow,
  inViewOf,
  newerPage,
  newerPageSpeaksFor,
  pageFoldIn,
  placeFromPageIn as placeFromPage,
  projectionOf,
  restoreCanon,
  sameMessage,
  threadOf,
  timeOf,
} from "../../core/channels/index.ts";
import type { ChannelCanon, ChannelStore, CopyOrder, PageFold, Removals, ReplyTally, SavedTimeline } from "../../core/channels/index.ts";
/** The reader (core/sync `createTimelineReader`), as this view uses it. */
export interface TimelineReaderLike {
  readTimeline(path: string, options?: { signal?: CancelSignal }): Promise<TimelineResponse>;
  timelinePath(before?: string | null, after?: string | null, reason?: string | null): string;
  readReasonOf(options: { readAs?: string; reason?: string | readonly (string | undefined)[] } | null | undefined): string | null;
  withReadReason(path: string, options: { readAs?: string; reason?: string | readonly (string | undefined)[] } | null | undefined): string;
  olderReadReason(): string;
  forwardCursor(reason: string | readonly (string | undefined)[] | undefined): string | null;
  joinedDeltas(pages: readonly TimelineResponse[]): TimelineResponse;
  landedAnswer(pages: readonly TimelineResponse[]): { messages: Message[]; threads: ThreadSummary[]; delta: { complete: boolean } | null } | null;
  staleAsOf(payload: TimelineResponse): number | null;
}
import { READ_MODE_LABELS, READ_MODE_TITLES, READ_MODE_VERBS, allNoticeWords, nextReadMode } from "../../present/rows/index.ts";
import { createOlderPager } from "../../core/channels/index.ts";
import { isMessageRow, patchRows, rowShown, shownRowFor } from "./rows.ts";
import { cardRunIds, threadCardId } from "./cards.ts";
import { childByClass } from "../dom/index.ts";
import { neighbourStayed, rowNeighbours } from "./cardplace.ts";
import { answeredThread } from "../../core/channels/index.ts";
import { READ_RUN_PEEL_SIZE, isNoise, joinsGroup, readRunText } from "../../present/rows/index.ts";
import type { CancelSignal, ClockPort } from "../../ports/index.ts";

/** The view on screen in the column being worked on (`selection()`). */
export interface TimelineSelection {
  view: string;
  threadId: string | null;
  thread: ThreadSummary | null;
}

/** The column being worked on's rows (`timelineRows()`). */
export interface TimelineRows {
  messages: Message[];
  threads: ThreadSummary[];
}

/** The column being worked on's walk back (`olderPages()`). */
export interface TimelineOlderPages {
  cursor: string | null;
  moreAbove: boolean | undefined;
  inFlight: boolean;
  step: Promise<void> | null;
  deferred: boolean;
}

/** The column being worked on's read state (`loadState()`), as far as this view touches it. */
export interface TimelineLoadState {
  generation: number;
  freshAt: number;
  freshness: string;
}

/** The column being worked on's entry state (`entryState()`), as far as this view touches it. */
export interface TimelineEntryState {
  threadOwed: string | null;
  threadAwaitsRead: boolean;
  readOwed: string | null;
}

/** One column of the two, as working on it reads it. */
export interface TimelineColumn {
  readonly selection: TimelineSelection;
  readonly rows: TimelineRows;
  readonly older: TimelineOlderPages;
  readonly load: TimelineLoadState;
  readonly entry: TimelineEntryState;
}

/** Working on a captured column across a wait (`columnSteps()`). */
export interface TimelineColumnSteps {
  readonly column: TimelineColumn;
  pause(): void;
  resume(): void;
}

/** Why a refresh was asked for (`queueDiscordLoad` merges them). */
export interface TimelineReadOptions {
  keepPosition?: boolean;
  quiet?: boolean;
  ownAct?: boolean;
  reason?: string | readonly (string | undefined)[];
  readAs?: string;
}

/** A read given up on, kept so its late answer can still land. */
export interface TimelineAbandonedRead {
  key: string;
  context: string;
  channel: string;
  view: string;
  thread: string | null;
  canon: ChannelCanon;
  column: TimelineColumn;
  sent: string | null;
  held: Map<string, Message> | null;
  sentAt: number;
  live: number;
  news: number;
  number: number;
}

/** What the timeline view asks of the page: the legacy names, each asked as it is needed. */
export interface TimelineViewDependencies {
  el(id: string): HTMLElement;
  selection(): TimelineSelection & { view: string; threadId: string | null };
  timelineRows(): TimelineRows;
  olderPages(): TimelineOlderPages;
  loadState(): TimelineLoadState;
  listState(): { newestId: string | null; backlog: number; runRows: number };
  entryState(): TimelineEntryState;
  doneState(): { archived: Set<string> };
  selectedChannelId(): string;
  channelContextKey(): string;
  channelStore: ChannelStore;
  providerFacts: { threading(): boolean };
  messageCache: {
    readMessageCache(): { scopes: Record<string, { mode: string; bookmark?: unknown; usedAt?: number } | undefined> } | null;
    writeMessageCache(cache: unknown, channel: string): void;
    forgetChannelScopes(matches: (id: string) => boolean): void;
  };
  restoreSavedBookmark(channel: string, bookmark: unknown): void;
  noteServerThreadHuesIn(messages: readonly Message[], threads: readonly ThreadSummary[], channel?: string, saved?: boolean): void;
  knownChannel(channel: string): TimelineResponse["channel"];
  viewKey(view?: string, thread?: string | null): string;
  reader: TimelineReaderLike;
  timelineDeltaPages: number;
  isCursorRefusal(error: unknown): boolean;
  abandonedReads: {
    held(key: string, options?: TimelineReadOptions | null): boolean;
    holdKey(): string;
    abandon<T>(answer: Promise<T>, read: { key: string; channel: string; sentAt: number; live: number; news: number; canon: unknown; generation?: number; number?: number }, land: (payload: T) => void, settled?: ((landed: boolean, unused: T | null) => void) | null): void;
  };
  channelReads: {
    claim(context: string | null): boolean;
    cursorOwed(): boolean;
    oweCursor(owed: boolean): void;
    queued(column: TimelineColumn): boolean;
    payCursor(): void;
  };
  queueDiscordLoad(options?: TimelineReadOptions): void;
  finishDiscordLoad(column?: TimelineColumn): Promise<void>;
  replayLedger: { start(): unknown; mutationAwaits(): boolean };
  columnReadsSent: Map<TimelineColumn, { context: string; read: unknown; full: boolean }>;
  landAbandonedTimelineRead(payload: TimelineResponse, read: TimelineFoldRead): void;
  queuePlaceUnplaced(): void;
  heldMessage(id: string): Message | null;
  removedUpstream(id: string): boolean;
  setStatus(text: string): void;
  prepareSpeech(): void;
  guardQuietly(fn: (...args: never[]) => unknown): (...args: never[]) => unknown;
  settleReplays(read: unknown, answer: { messages: Message[]; threads: ThreadSummary[]; delta: { complete: boolean } | null } | null): void;
  liveSeen(channel: string): number;
  liveNews(channel: string): number;
  readsAsked(): number;
  readOrderNow(): number;
  sentNumberOf(payload: TimelineResponse): number;
  refreshFailed(payload: TimelineResponse): boolean;
  within<T>(ms: number, request: (signal: CancelSignal | undefined) => Promise<T>, abandoned?: ((answer: Promise<T>) => void) | null): Promise<T>;
  clock: ClockPort;
  discordNode(messages: Message[]): HTMLElement;
  threadCards(threads: readonly ThreadSummary[]): HTMLElement[];
  glom(messages: readonly Message[]): Message[][];
  idsOf(row: Element | null | undefined): string[];
  rowMessages(row: Element | null | undefined): Message[];
  hiddenEmpty(message: Message): boolean;
  hidingRead(): boolean;
  readFilterKeeps(thread?: unknown): (message: Message) => boolean;
  gatheredParent(): string | null;
  observeOutgoingMessages(messages: readonly Message[]): void;
  noteArchived(payload: TimelineResponse, replace: boolean): void;
  noteFreshRead(payload: TimelineResponse): void;
  noteChannelReadFailure(error: unknown): void;
  renderChannelLoading(on: boolean, quiet?: boolean, background?: boolean): void;
  backgroundRead(options?: TimelineReadOptions): boolean;
  renderChannelNavigation(): void;
  renderChannelSeam(label: string): void;
  renderOlderControl(): void;
  renderScrollTools(): void;
  renderTodoControls(): void;
  renderChannelRows(): void;
  setChannelFreshness(state: string): void;
  renderOutgoingMessages(): void;
  channelSummary(count: number, whole: boolean | undefined, name: string): string;
  channelName(channel: TimelineResponse["channel"]): string;
  clearChannelScreen(): void;
  holdingReplyArrows(draw: () => void): void;
  holdingScrollTools<T>(draw: () => T): T;
  renderChannelFreshness(): void;
  preservingScroll(mutate: () => void): void;
  columnWork: { active(): TimelineColumn; side(): boolean };
  catchUpHeldView(words?: boolean): boolean;
  catchUpFromAll(): void;
  syncUnreadReplies(): void;
  followInOtherColumn(): void;
  keepEntryLanding(mark: unknown): boolean;
  entryLandingMark(): unknown;
  payThreadEntry(): void;
  saveChannelScope(): void;
  requestVisibleSummaries(): void;
  seamsAfterMidnight(): void;
  applySearch(): void;
  columnSteps(): TimelineColumnSteps;
  inColumn<T>(column: TimelineColumn, fn: () => T): T;
  inOtherColumn(fn: () => void): void;
  replyTally(): ReplyTally;
  unseenReplies(id: string, message: Message, tally: unknown): number;
  followPlacement(): void;
  unplacedCount(): number;
  heldUnansweredRoot(id: string | null): Message | null;
  copyLedger: CopyOrder;
  removalRecord: Removals;
  removedAfterRead(canon: ChannelCanon, id: string, versions: unknown): boolean;
  ownPostStands(id: string): boolean;
  foldReadState(payload: TimelineResponse, view: string): boolean;
  noteReplyGaps(payload: TimelineResponse, view: string, older: boolean, delta: boolean, before: unknown): void;
  noteServerPageBuild(payload: TimelineResponse): void;
  renderColumns(): void;
  threadPresenceNote(hasThreads: boolean | undefined, inThread: boolean): void;
  syncCanonDismissed(): void;
  olderTriggerPx: number;
  inThisColumn(fn: (...args: never[]) => unknown): (...args: never[]) => unknown;
  pinsView(): { only: boolean };
  searchOffered(): boolean;
  pullToRefresh: { fingerDown(): boolean };
  heldFilterRow(): Element | null;
  /** The row a jump just landed on, held as the place until the reader moves (`#1224`). Absent: unkept. */
  jumpAnchorRow?(): Element | null;
  scrollAnchor(area: HTMLElement): Element | null;
  /** The reader's text selection in `area` and putting it back (`#1209`), as the scroll keeper's. */
  selectionIn?(area: HTMLElement): { id: string; start: number; end: number } | null;
  restoreSelectionIn?(area: HTMLElement, selection: { id: string; start: number; end: number }): void;
  followsNewest(area: HTMLElement): boolean;
  visibleList(): HTMLElement;
  noiseSettings: { renderCount(): void };
  combineOn(): boolean;
  columns: { main: TimelineColumn; side: TimelineColumn & { mode: string } };
  sideShowing(): boolean;
  openThreadRootId(): string | null;
  isOpenThreadRoot(row: Element, rootId: string | null): boolean;
  readRunWords(row: Element): string;
  placeInReadRun(row: Element, role: string, count: number): void;
  runDaysOf(row: Element): string;
  renderReplyLinks(): void;
  leftEl(id: string): HTMLElement;
  cardRunKey(): string;
  cardRunRows(head: Element): Element[];
  screensScreen(): string;
  cardsDrawnForget(): void;
  renderSideColumn(): void;
  keepSelection(): void;
  searchField: { open(): boolean; links(): boolean; query(): string };
  authorChosen(): string;
  searchTyped(query: string): boolean;
  stillToDo(message: Message, done?: ReadonlySet<string>): boolean;
  screenBelongsToToken(): boolean;
  readModes: { hiding(): boolean; mode(): string };
  readLedger: { epoch(): number; noteSent(payload: unknown, sent: number): void };
  api(path: string, options?: { signal?: CancelSignal }): Promise<TimelineResponse>;
  currentDiscordLoad(generation: number, channel: string, readingTodo: boolean): boolean;
  noChannelWords: string;
  notePinsRevision(revision: unknown): void;
  noteBookmark(payload: unknown): void;
  plainReach: { noteNewest(saved: boolean, kept: number, before: number, messages: number, at: string | undefined): void; gapFrom(): number | null; restore(gap: number | null): void };
  noiseRenderCount(): void;
  foldedOneIds(key: string): Set<string>;
  foldedOnes: Map<string, Set<string>>;
  readRuns: { plan(rows: unknown, key: string, joins: unknown, idsFor?: unknown, forcedIds?: Set<string>): Map<Element, { role: string; count: number }>; open(key: string, ids: string[]): void; peel(key: string, ids: string[]): void };
  /** Fold every revealed row back to its head line: a message out of a fold starts collapsed. */
  collapseRevealed(rows: readonly Element[]): void;
  setReadMode(mode: string): void;
  setJumpNewest(wanted: boolean, view?: string, arrived?: number | null): void;
  scrollToNewest(): void;
  keepFilterHold(): boolean;
  unplacedNext(): number;
  threadPresence: { note(hasThreads: boolean | undefined, inThread: boolean): void };
  openThreadName(): string;
  captureScroll(): { pinned: boolean; anchor: Element | null; top: number };
  restoreScroll(mark: { pinned: boolean; anchor: Element | null; top: number }): void;
  /** Move the list by a measured delta, or put it at a measured position, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(area: HTMLElement, delta: number): void;
  toPosition(area: HTMLElement, top: number): void;
  screensView(): string;
  renderJumpNewest(): void;
}

/** What folding a late page asks: the view it read, its store and what it held. */
export interface TimelineFoldRead {
  context: string;
  channel: string;
  view: string;
  thread: string | null;
  canon: ChannelCanon;
  column?: unknown;
  sent?: string | null;
  held?: Map<string, Message> | null;
}

/** The timeline view's commands and questions: the legacy names, unchanged. */
export interface TimelineView {
  loadCanon(channel: string): void;
  drawProjection(): boolean;
  walkCursorOwed(): boolean;
  heldViewRows(): Message[];
  drawHeldRows(rows: readonly Message[], freshAt: number, freshness: string): boolean;
  queueReadAllForCounts(): void;
  readAllForCounts(channel: string, soon?: boolean): Promise<void>;
  rowsAsDrawn(pages: readonly TimelineResponse[]): boolean;
  applyTimelinePage(payload: TimelineResponse, older?: boolean, saved?: boolean, delta?: boolean, rows?: readonly Message[] | null, held?: Map<string, Message> | null): Message[];
  renderCachedTimeline(): void;
  mutationAwaitsRead(): boolean;
  loadTimeline(options?: TimelineReadOptions): Promise<void>;
  foldLateTimelinePage(payload: TimelineResponse, read: TimelineFoldRead): void;
  loadOlderTimeline(signal?: CancelSignal): Promise<void>;
  foldLiveMessage(message: Message, live?: boolean, replayed?: boolean, read?: unknown): Message;
  loadDiscord(options?: TimelineReadOptions): Promise<void>;
  placeReplyTarget(entry: { replyTo?: unknown; channel?: string }): Promise<string | null>;
  loadOlder(signal?: CancelSignal): Promise<void> | null;
  maybeLoadOlder(): void;
  takeDeferredOlder(): void;
  appendChannelRow(arriving: Message, live?: boolean, folded?: boolean): void;
  appendToColumns(arriving: Message, live?: boolean, replayed?: boolean): Message;
  applyReadRuns(): void;
  readRunPlan(rows: unknown, key: string, joins: unknown, idsFor?: unknown, forcedIds?: Set<string>): Map<Element, { role: string; count: number }>;
  readRunRows(row: Element): Element[];
  openReadRun(row: Element): void;
  peelReadRun(row: Element, edge: "first" | "last"): void;
  renderReadModeControl(): void;
  readRunsSuspended(): boolean;
  applyNewestPage(payload: TimelineResponse, saved?: boolean): number;
  channelReadPosition(area: HTMLElement): { wasAtNewest: boolean; previousTop: number; anchorId: string | null; anchorTop: number; around: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null };
  settleAfterRead(messages: readonly Message[], how: { area: HTMLElement; keepPosition: boolean; ownAct?: boolean; wasAtNewest: boolean; previousTop: number; anchorId?: string | null; anchorTop?: number; around?: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null; heldPlaceAt?: number }): void;
  landedReadPosition(area: HTMLElement, keepPosition: boolean, sentAtNewest: boolean): { wasAtNewest: boolean; keepPosition: boolean; previousTop: number; anchorId: string | null; anchorTop: number; around: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null };
  rowIsRead(row: Element): boolean;
  cycleReadMode(): void;
  inView(message: Message, view: string, thread: string | null, scope?: string | null): boolean;
  foldTimelinePage(payload: TimelineResponse, older: boolean, view?: string, thread?: string | null, held?: Map<string, Message> | null): void;
  foldTimelineDelta(payload: TimelineResponse, sent: string | null, view?: string, thread?: string | null): boolean;
  projectView(view?: string, thread?: string | null): Message[] | null;
  holdingMessage(mutate: () => void): void;
  dispose(): void;
}

/** How long after a read of All a read of Main asks for none for its counts: five minutes. */
const COUNTS_ALL_READ_MS = 300000;
/** How soon after a counts read the server answered from its own copy the page asks once more. */
const COUNTS_SOON_MS = 10000;
/** How long one timeline read may run before the page gives up on it. */
const CHANNEL_READ_TIMEOUT_MS = 20000;
/** How many older pages `pagesBackTo` follows at most. */
const REPLY_JUMP_PAGES = 5;

/** The timeline view over `deps`: reads drawn, pages folded, the walk back walked. */
export function createTimelineView(deps: TimelineViewDependencies): TimelineView {
  const el = deps.el;
  const selection = deps.selection;
  const timelineRows = deps.timelineRows;
  const olderPages = deps.olderPages;
  const loadState = deps.loadState;
  const listState = deps.listState;
  const entryState = deps.entryState;
  const doneState = deps.doneState;
  const selectedChannelId = deps.selectedChannelId;
  const channelContextKey = deps.channelContextKey;
  const channelStore = deps.channelStore;
  const providerFacts = deps.providerFacts;
  const messageCache = deps.messageCache;
  const timelineReader = deps.reader;
  const TIMELINE_DELTA_PAGES = deps.timelineDeltaPages;
  const isCursorRefusal = deps.isCursorRefusal;
  const abandonedReads = deps.abandonedReads;
  const channelReads = deps.channelReads;
  const queueDiscordLoad = deps.queueDiscordLoad;
  const finishDiscordLoad = deps.finishDiscordLoad;
  const sentNumberOf = deps.sentNumberOf;
  const refreshFailed = deps.refreshFailed;
  const within = deps.within;
  const copyLedger = deps.copyLedger;
  const removalRecord = deps.removalRecord;
  const removedAfterRead = deps.removedAfterRead;
  const ownPostStands = deps.ownPostStands;
  const readOrder = { asked: () => deps.readsAsked() };
  const orderNow = () => deps.readOrderNow();
  const liveEvents = { seen: (channel: string) => deps.liveSeen(channel), news: (channel: string) => deps.liveNews(channel) };
  const replayLedger = deps.replayLedger;
  const columnReadsSent = deps.columnReadsSent;
  const landAbandonedTimelineRead = deps.landAbandonedTimelineRead;
  const queuePlaceUnplaced = deps.queuePlaceUnplaced;
  const setStatus = deps.setStatus;
  const guardQuietly = deps.guardQuietly;
  const readAloudOwner = () => ({ prepareSpeech: deps.prepareSpeech });
  const renderChannelFreshness = deps.renderChannelFreshness;
  const searchField = deps.searchField;
  const authorFilters = { chosen: () => deps.authorChosen() };
  const searchTyped = deps.searchTyped;
  const cardsDrawn = { forget: () => deps.cardsDrawnForget() };
  const renderSideColumn = deps.renderSideColumn;
  const keepSelection = deps.keepSelection;
  const stillToDo = deps.stillToDo;
  const inThisColumn = deps.inThisColumn;
  const pinsView = deps.pinsView;
  const searchOffered = deps.searchOffered;
  const pullToRefresh = deps.pullToRefresh;
  const heldFilterRow = deps.heldFilterRow;
  const jumpAnchorRow = deps.jumpAnchorRow;
  const scrollAnchor = deps.scrollAnchor;
  const followsNewest = deps.followsNewest;
  const visibleList = deps.visibleList;
  const noiseSettings = deps.noiseSettings;
  const combineSwitch = { on: () => deps.combineOn() };
  const columns = deps.columns;
  const sideShowing = deps.sideShowing;
  const openThreadRootId = deps.openThreadRootId;
  const isOpenThreadRoot = deps.isOpenThreadRoot;
  const readRunWords = deps.readRunWords;
  const placeInReadRun = deps.placeInReadRun;
  const runDaysOf = deps.runDaysOf;
  const renderReplyLinks = deps.renderReplyLinks;
  const leftEl = deps.leftEl;
  const cardRunKey = deps.cardRunKey;
  const cardRunRows = deps.cardRunRows;
  const OLDER_TRIGGER_PX = deps.olderTriggerPx;
  const preservingScroll = deps.preservingScroll;
  const columnWork = deps.columnWork;
  const settleReplays = deps.settleReplays;
  const unplacedArrivals = { count: () => deps.unplacedCount(), next: () => deps.unplacedNext() };
  const countsAllReadInFlight = (() => {
    let inFlight: string | null = null;
    return { busy: () => inFlight !== null, begin: (channel: string) => { inFlight = channel; }, end: () => { inFlight = null; } };
  })();
  const noteServerPageBuild = deps.noteServerPageBuild;
  const noteServerThreadHuesIn = deps.noteServerThreadHuesIn;
  const renderColumns = deps.renderColumns;
  const foldReadState = deps.foldReadState;
  const noteReplyGaps = deps.noteReplyGaps;
  const captureScroll = deps.captureScroll;
  const restoreScroll = deps.restoreScroll;
  const renderJumpNewest = deps.renderJumpNewest;
  const threadPresence = deps.threadPresence;
  const openThreadName = deps.openThreadName;
  const discordNode = deps.discordNode;
  const threadCards = deps.threadCards;
  const glom = deps.glom;
  const idsOf = deps.idsOf;
  const rowMessages = deps.rowMessages;
  const hiddenEmpty = deps.hiddenEmpty;
  const hidingRead = deps.hidingRead;
  const readFilterKeeps = deps.readFilterKeeps;
  const gatheredParent = deps.gatheredParent;
  const observeOutgoingMessages = deps.observeOutgoingMessages;
  const noteArchived = deps.noteArchived;
  const noteFreshRead = deps.noteFreshRead;
  const noteChannelReadFailure = deps.noteChannelReadFailure;
  const renderChannelLoading = deps.renderChannelLoading;
  const backgroundRead = deps.backgroundRead;
  const renderChannelNavigation = deps.renderChannelNavigation;
  const renderChannelSeam = deps.renderChannelSeam;
  const renderOlderControl = deps.renderOlderControl;
  const renderScrollTools = deps.renderScrollTools;
  const renderTodoControls = deps.renderTodoControls;
  const renderChannelRows = deps.renderChannelRows;
  const setChannelFreshness = deps.setChannelFreshness;
  const renderOutgoingMessages = deps.renderOutgoingMessages;
  const channelSummary = deps.channelSummary;
  const channelName = deps.channelName;
  const clearChannelScreen = deps.clearChannelScreen;
  const holdingReplyArrows = deps.holdingReplyArrows;
  const holdingScrollTools = deps.holdingScrollTools;
  const catchUpHeldView = deps.catchUpHeldView;
  const catchUpFromAll = deps.catchUpFromAll;
  const syncUnreadReplies = deps.syncUnreadReplies;
  const followInOtherColumn = deps.followInOtherColumn;
  const keepEntryLanding = deps.keepEntryLanding;
  const entryLandingMark = deps.entryLandingMark;
  const payThreadEntry = deps.payThreadEntry;
  const saveChannelScope = deps.saveChannelScope;
  const requestVisibleSummaries = deps.requestVisibleSummaries;
  const seamsAfterMidnight = deps.seamsAfterMidnight;
  const applySearch = deps.applySearch;
  const columnSteps = deps.columnSteps;
  const inColumn = deps.inColumn;
  const inOtherColumn = deps.inOtherColumn;
  const replyTally = deps.replyTally;
  const unseenReplies = deps.unseenReplies;
  const followPlacement = deps.followPlacement;
  const heldUnansweredRoot = deps.heldUnansweredRoot;
  const restoreSavedBookmark = deps.restoreSavedBookmark;
  const knownChannel = deps.knownChannel;
  const screens = { view: () => deps.screensView() };
  const olderPager = createOlderPager();
  const screenBelongsToToken = deps.screenBelongsToToken;
  const readModes = deps.readModes;
  const readLedger = deps.readLedger;
  const api = deps.api;
  const currentDiscordLoad = deps.currentDiscordLoad;
  const notePinsRevision = deps.notePinsRevision;
  const noteBookmark = deps.noteBookmark;
  const plainReach = deps.plainReach;
  const foldedOneIds = deps.foldedOneIds;
  const foldedOnes = deps.foldedOnes;
  const readRuns = deps.readRuns;
  const setReadMode = deps.setReadMode;
  const setJumpNewest = deps.setJumpNewest;
  const scrollToNewest = deps.scrollToNewest;
  const keepFilterHold = deps.keepFilterHold;
  let disposed = false;
  const countsAllReadAt = new Map<string, number>();
  const countsAllReadFor = new Map<string, Map<string, number>>();
  const unchangedDeltas = new WeakSet<object>();

const viewKey = deps.viewKey;
  const clock = deps.clock;

/** Whether `message` belongs in `view` (web/src/core/channels `inViewOf`), in this channel's scope by default. */

function inView(message: Message, view: string, thread: string | null, scope: string | null = channelStore.current().scope): boolean {
  return inViewOf(message, view, thread, scope);
}

// `mergeIntoCanon` and `upsertThreadSummary` are web/src/core/channels's, and take the store they
// fold into (`#624 voice-page-modularity`).


/**
 * Fold a page read for a view — the current one by default — into the store. Never a message removed
 * upstream after the read was sent; and `held`, when given, is the store's copies as a late read was
 * sent (`heldNow`): it is folded only over the copies older than it (`pageFold`, `#316
 * late-counts-resurrect`, `#310 stale-live-read-flicker`). What it takes stands where it was sent
 * (`copyOrder`).
 *
 * @param {any} payload
 * @param {boolean} older
 * @param {string} [view]
 * @param {string | null} [thread]
 * @param {Map<string, any> | null} [held]
 */

function pageFold(canon: ChannelCanon, payload: TimelineResponse, held: Map<string, Message> | null): PageFold {
  return pageFoldIn(canon, payload, held, {
    number: sentNumberOf(payload),
    ledger: copyLedger,
    removedAfterRead,
    newerPageSpeaksFor,
    ownPostStands,
  });
}

/**
 * Fold a DELTA — what changed in a view since its forward cursor — into the store (web/src/core/channels
 * `foldDeltaIn`, `#203 incremental-refresh`, which says why each rule is what it is): false, and nothing
 * touched, when the cover no longer holds the cursor the read was sent with. What it does to the page
 * besides the store is passed in, to happen where it always did: Done forgotten for a deleted message,
 * read state folded, the replies the store lacks noted, an unchanged delta marked (`unchangedDeltas`).
 */

function foldTimelinePage(payload: TimelineResponse, older: boolean, view: string = selection().view, thread: string | null = selection().threadId, held: Map<string, Message> | null = null): void {
  const channel = String(selectedChannelId());
  // `#529 page-build-stale-notice`.
  noteServerPageBuild(payload);
  // `#427 server-thread-hues`: the colours this read carries, taken before anything it brought
  // is drawn, so its rows never show a fallback colour the server has already decided.
  noteServerThreadHuesIn(payload.messages || [],
    [...(payload.threads || []), payload.thread].filter((x): x is ThreadSummary => Boolean(x)), channel);
  if (channelStore.current().channel !== channel) {
    loadCanon(channel);
    // Copies of another store say nothing of this one's.
    held = null;
  }
  // The store's part (web/src/core/channels `foldPageIn`); what it does to the page besides the store is
  // passed in, to happen where it always did.
  foldPageIn(channelStore.current(), payload, older, view, thread, viewKey(view, thread), held, {
    ledger: copyLedger,
    removals: removalRecord,
    numberOf: () => sentNumberOf(payload),
    foldOf: (heldNow) => pageFold(channelStore.current(), payload, heldNow),
    scopeChanged: () => {
      Promise.resolve().then(renderColumns);
    },
    staleAsOf: () => timelineReader.staleAsOf(payload),
    foldReadState: () => {
      foldReadState(payload, view);
    },
    noteReplyGaps: (before) => noteReplyGaps(payload, view, older, false, before),
  });
}

/**
 * What a page of a view, `payload`, may fold into store `canon` (web/src/core/channels `pageFoldIn`,
 * `#316 late-counts-resurrect`, which says why each rule is what it is), its read placed in the order of
 * reads (`sentNumberOf`), with the copy ledger, the server's record of removals (`removedAfterRead`),
 * whether a newer newest page speaks for a message (`newerPageSpeaksFor`) and whether the reader's own
 * fresh post stands (`ownPostStands`).
 *
 * @param {any} canon
 * @param {VibeTalk.TimelineResponse} payload
 * @param {Map<string, any> | null} held
 */

function foldTimelineDelta(payload: TimelineResponse, sent: string | null, view: string = selection().view, thread: string | null = selection().threadId): boolean {
  // `#529 page-build-stale-notice`: a delta says which page the server serves as a full read does.
  noteServerPageBuild(payload);
  // `#427 server-thread-hues`: a delta's new threads carry their colours, as a page's do.
  noteServerThreadHuesIn(payload.messages || [],
    [...(payload.threads || []), payload.thread].filter((x): x is ThreadSummary => Boolean(x)));
  return foldDeltaIn(channelStore.current(), payload, sent, view, viewKey(view, thread), {
    ledger: copyLedger,
    removals: removalRecord,
    numberOf: () => sentNumberOf(payload),
    readsSent: () => readOrder.asked(),
    forgetArchived: (id) => doneState().archived.delete(id),
    foldReadState: () => foldReadState(payload, view),
    noteReplyGaps: (before) => noteReplyGaps(payload, view, false, true, before),
    markUnchanged: () => { unchangedDeltas.add(payload); },
    staleAsOf: () => timelineReader.staleAsOf(payload),
  });
}

/** The delta pages `foldTimelineDelta` found changed nothing. `#226 page-energy-profile`. */

function foldLiveMessage(message: Message, live = false, replayed = false, read: number | null = null): Message {
  const channel = String(selectedChannelId());
  if (channelStore.current().channel !== channel) loadCanon(channel);
  // `#427 server-thread-hues`: a live message carries its thread's colour, taken before it is
  // drawn — a thread met first through the stream is the server's colour from its first row.
  noteServerThreadHuesIn([message], [], channel);
  return foldLiveIn(channelStore.current(), message, live, replayed, read, {
    ledger: copyLedger,
    orderNow: () => orderNow(),
    nextArrival: () => unplacedArrivals.next() as number,
  });
}

/**
 * The current view's rows from the store, or null when no page has covered this view
 * (web/src/core/channels `projectionOf`).
 */

function projectView(view: string = selection().view, thread: string | null = selection().threadId): Message[] | null {
  return projectionOf(channelStore.current(), viewKey(view, thread), view, thread) as Message[];
}

/** The read state of the rows on screen is the store's read state for those rows. */

function holdingMessage(mutate: () => void): void {
  const area = el("scroll-area");
  const mark = captureScroll();
  const position = channelReadPosition(area);
  mutate();
  if (mark.pinned || (mark.anchor && mark.anchor.parentNode)) {
    restoreScroll(mark);
    return;
  }
  const held = heldRowAfter(position);
  if (held && screens.view() === "discord") deps.nudge(area, held.row.getBoundingClientRect().top - held.top);
  renderJumpNewest();
}

/** Re-project the already fetched timeline after a local preference or archive change. */

function loadCanon(channel: string): void {
  channelStore.open(channel);
  const cache = messageCache.readMessageCache();
  const entry = cache ? cache.scopes[String(channel)] : undefined;
  if (!entry || entry.mode !== "timeline") return;
  // What the device kept of it (web/src/core/channels `restoreCanon`).
  restoreCanon(channelStore.current(), entry as SavedTimeline);
  restoreSavedBookmark(channel, entry.bookmark);
  // `#427 server-thread-hues`: the saved rows were saved as they were served, colours on them,
  // so a reload draws the server's colours before any read lands — not this browser's fallback.
  noteServerThreadHuesIn(channelStore.current().messages, channelStore.current().threads, String(channel), true);
  entry.usedAt = Date.now();
  messageCache.writeMessageCache(cache, String(channel));
}

/**
 * Draw the current view from the store, with no request. False when no page has covered it. Saved
 * rows go through the same applier a read uses, so they are built, filtered and grouped exactly as
 * fetched ones — but WITHOUT retiring anything from the outbox: only the server seeing a message
 * proves it was delivered.
 */

function drawProjection(): boolean {
  const cover = channelStore.current().views.get(viewKey());
  if (!cover) return false;
  const channel = selectedChannelId();
  const payload = {
    channel: knownChannel(channel),
    thread: selection().view === "thread"
      ? channelStore.current().threads.find((held) => String(held.id) === selection().threadId) || selection().thread
      : null,
    has_threads: channelStore.current().hasThreads,
    has_more: cover.more,
    next_before: cover.cursor,
    notice: cover.notice,
    dismissed: [...channelStore.current().dismissed],
  };
  try {
    timelineRows().messages = [];
    timelineRows().threads = [];
    applyTimelinePage(payload as TimelineResponse, false, true);
  } catch (_error) {
    // A saved row this version cannot draw is a damaged entry, not a reason to break the page.
    clearChannelScreen();
    messageCache.forgetChannelScopes((id) => id === String(channel));
    return false;
  }
  listState().newestId = timelineRows().messages.length > 0 ? String((timelineRows().messages[timelineRows().messages.length - 1] as Message).id) : null;
  loadState().freshAt = cover.at;
  delete cover.landedHidden;
  setChannelFreshness(cover.live ? "fresh" : "saved");
  // Main drawn from All's cover, read in this page: older history is known to exist, and the step
  // back to it is Main's own first read (`walkCursorOwed`). `#220 view-switch-instant`.
  if (cover.fromAll && cover.live && cover.more) {
    olderPages().moreAbove = true;
    renderOlderControl();
  }
  // A delivered send whose row is saved here shows once, as that row — but its receipt stays until
  // the server itself is seen to have the message.
  renderOutgoingMessages();
  return true;
}

/**
 * Whether the view on screen has older history but no cursor to walk back on: Main drawn from All's
 * cover, which has All's floor and All's word that more exists but not a cursor of Main's own. Its
 * first step back is Main's newest page, read behind the rows (`readWalkCursor`): Main's fifty
 * reach further back than the roots among All's fifty, and the page brings the cursor the walk goes
 * on from. `#220 view-switch-instant`.
 */

function walkCursorOwed(): boolean {
  if (olderPages().cursor || channelStore.current().channel !== String(selectedChannelId())) {
    return false;
  }
  // A page that said more exists and carried no cursor, whatever the view's cover: one a
  // server before `#510 cache-serve-every-read` answered from its message cache.
  if (channelReads.cursorOwed()) return true;
  if (!providerFacts.threading()) return false;
  const cover = channelStore.current().views.get(viewKey());
  return Boolean(cover && cover.fromAll && cover.live && cover.more);
}

/**
 * The rows of the current view the store holds though no page has covered the view, to draw while
 * its read is on the wire. `#220 view-switch-instant`. ONLY THE STRETCH ANOTHER COVER VOUCHES FOR,
 * so that what is drawn is one continuous piece of the view, never two with the messages between
 * them unread: for All, Main's cover, which holds every message posted to the channel itself from
 * its floor up, with the replies the page has met among them; for a thread, All's, from its floor
 * up (a thread All reaches from its start is covered already, `inheritAllCover`). An old thread read
 * for itself is not joined to Main's newest page as All, nor a thread's root to its newest replies.
 * Empty when no cover vouches — and for Main, which is drawn from All or read, and the Threads list.
 *
 * `#252 thread-button-state`. A thread nobody has answered, whose root the store holds, is that root
 * alone, by its own count: drawn from it when All does not reach it, so the grey Thread tag opens it
 * at once, ready for its first reply, wherever the root was found. Its read still follows.
 */

function heldViewRows(): Message[] {
  if (!providerFacts.threading() || (selection().view !== "flat" && selection().view !== "thread") ||
      channelStore.current().channel !== String(selectedChannelId())) return [];
  const base = channelStore.current().views.get(viewKey(selection().view === "flat" ? "main" : "flat"));
  const rows = !base || (base.more && base.floor === null) ? [] : channelStore.current().messages.filter((message) =>
    inView(message, selection().view, selection().threadId) && !(base.more && timeOf(message, "timestamp") < (base.floor as number)));
  const root = rows.length === 0 && selection().view === "thread" ? heldUnansweredRoot(selection().threadId) : null;
  return root ? [root] : rows;
}

/**
 * Draw a view no page has covered from `rows`, rows of it the store already holds, with no request:
 * as current as the view the reader came from, `freshAt` and `freshness`. `#220
 * view-switch-instant`. Main to All before All was ever read is the case: Main's rows and every
 * reply the page has met between them are up at once, and the read behind them adds what only All
 * can say. NEVER A COVER — nothing here proves the view complete — so the view's read is still owed,
 * and the walk back waits for it. False, with nothing drawn, when there is nothing to draw.
 */

function drawHeldRows(rows: readonly Message[], freshAt: number, freshness: string): boolean {
  if (!rows.length) return false;
  const channel = selectedChannelId();
  const payload = {
    channel: knownChannel(channel),
    thread: selection().view === "thread"
      ? channelStore.current().threads.find((held) => String(held.id) === selection().threadId) || selection().thread
      : null,
    has_threads: channelStore.current().hasThreads,
    // Whether older history exists is unknown until the read says.
    has_more: true,
    next_before: null,
    notice: "",
    dismissed: [...channelStore.current().dismissed],
  };
  try {
    timelineRows().messages = [];
    timelineRows().threads = [];
    applyTimelinePage(payload as TimelineResponse, false, true, false, rows);
  } catch (_error) {
    // Rows this version cannot draw: the read draws the view instead.
    return false;
  }
  listState().newestId = timelineRows().messages.length > 0 ? String((timelineRows().messages[timelineRows().messages.length - 1] as Message).id) : null;
  loadState().freshAt = freshAt;
  setChannelFreshness(freshness);
  renderOutgoingMessages();
  return true;
}

/**
 * Record what the channel holds. Called after every read that succeeded and after every local
 * change to the rows — a live arrival, an acknowledged send, an archive — so a reload shows what
 * the reader last saw.
 *
 * NOT in the legacy to-do filter: that list is `/todo`, a filtered answer, and saving it as the
 * channel would show a reload a channel with its dealt-with messages missing.
 */

function queueReadAllForCounts(): void {
  const channel = String(selectedChannelId());
  Promise.resolve().then(() => readAllForCounts(channel)).catch(() => {});
}

/**
 * The roots the store holds whose threads have replies it has not brought (`unseenReplies`), by
 * thread id, with the count each is reported to have.
 *
 * @returns {Map<string, number>}
 */

function outrunningThreads(): Map<string, number> {
  const tally = replyTally();
  /** @type {Map<string, number>} */
  const found = new Map();
  for (const message of channelStore.current().messages) {
    const id = threadOf(message);
    if (!id || !message.thread || message.thread.is_root !== true || id === channelStore.current().scope) continue;
    if (unseenReplies(id, message, tally) > 0) found.set(id, exactReplyCount(id, message, tally) || 0);
  }
  return found;
}

/**
 * Read All's newest page behind Main, for Main's counts, when they call for it and it is due: see
 * above. Folded as a read of All landing behind Main is (`foldLateTimelinePage`), which brings Main's
 * rows and chips up to date from it, and saves it with the device's copy — only over the copies older
 * than it, and never bringing back a message removed upstream since (`pageFold`, `#316
 * late-counts-resurrect`).
 */

async function readAllForCounts(channel: string, soon = false): Promise<void> {
  if (!providerFacts.threading() || !channel || String(selectedChannelId()) !== channel || selection().view !== "main" ||
      channelStore.current().channel !== channel || channelStore.current().scope || countsAllReadInFlight.busy() ||
      document.visibilityState === "hidden") return;
  const all = channelStore.current().views.get(viewKey("flat"));
  const now = Date.now();
  if (!soon && now - Math.max(countsAllReadAt.get(channel) || 0, all ? all.at || 0 : 0) < COUNTS_ALL_READ_MS) return;
  const outrun = outrunningThreads();
  const saw = countsAllReadFor.get(channel);
  if (![...outrun].some(([id, count]) => !(saw && (saw.get(id) || 0) >= count))) return;
  countsAllReadAt.set(channel, now);
  countsAllReadInFlight.begin(channel);
  // What the page can speak for: the store's copies as it is sent, and those older than it (`copyOrder`).
  // One the store gains while it is out — a reply the stream delivers, placed by a delta of All while
  // this page is still on its way, or the reader's own send — is newer than the page, which would take
  // it out as deleted (`#310`); one a later read replaces — an edit Main's read brings — the page would
  // undo; one it loses, the page would put back.
  const read = { context: JSON.stringify([channel, "flat", null]), channel, view: "flat", thread: null, canon: channelStore.current(),
    held: heldNow(channelStore.current()) };
  const seen = unplacedArrivals.count();
  try {
    const payload = await within(CHANNEL_READ_TIMEOUT_MS, (signal) => timelineReader.readTimeline(
      `/api/v1/channels/${encodeURIComponent(channel)}/timeline?view=flat&limit=${DISCORD_PAGE_LIMIT}&reason=counts`,
      { signal }));
    // `#614 timeline-read-failures`: the server's own copy, the chat service having failed or kept
    // it waiting, may lack the very replies the counts outran: the next counts read asks again.
    if (!refreshFailed(payload)) countsAllReadFor.set(channel, outrun);
    // `#650 cache-first-older-reads`: ...and asks once more soon, for the read the server made behind it.
    else if (!soon) clock.setTimeout(() => void readAllForCounts(channel, true).catch(() => {}), COUNTS_SOON_MS);
    const current = channelStore.current() === read.canon && read.canon.channel === channel &&
      String(selectedChannelId()) === channel;
    const number = sentNumberOf(payload);
    // Not over a newest page of All sent after this one — the reader switched to All, or tapped N
    // replies, and that read answered first: this answer is the older, and folded over it would put
    // back what that page says is gone, and its older copies over that page's. Sent after, not landed
    // after (`#308`): one sent before this and landing while it was on the wire is the older, and this
    // is folded over it.
    if (!newerPage(channelStore.current(), "flat", number)) {
      foldLateTimelinePage(payload, read);
    } else if (current && placeFromPage(payload, read.canon, number, read.held, copyLedger)) {
      // Not folded, it still says where each message the stream left unplaced that it carries is.
      followPlacement();
    }
    // ...and, folded or not, answers for what the stream had left unplaced before it was sent, as any
    // newest page of All does. `#308 live-reply-in-main`.
    if (current) answerUnplaced(read.canon, seen);
  } catch (_error) {
    // The counts stay as they were until a read of All or of the thread; the next read of Main past
    // the interval asks again.
  } finally {
    countsAllReadInFlight.end();
  }
}

// `#222 unread-replies`. DONE HERE RACES THE READS ON THE WIRE. A read sent before a Done or a put
// back reached the server answers with the read state from before it, and folding that would undo
// the reader's own act until the next read: a reply dealt with in its thread counted as unread on
// Main again. So each local write is remembered by message id until a read sent after it settled can
// speak for that message: while the write is on its way no read does, and once it is settled only a
// read sent after. The epoch moves at every write's start and end; a read records it when sent. The
// epoch, the local writes and when each read was sent are the ledger's (web/src/core/annotations
// `createReadLedger`, `#624 voice-page-modularity`), here where they stood.

function rowsAsDrawn(pages: readonly TimelineResponse[]): boolean {
  if (selection().view === "threads" || gatheredParent() !== null || entryState().threadOwed !== null) return false;
  if (!pages.every((page) => (page.dismissed || []).every((id) => doneState().archived.has(String(id))))) return false;
  const projected = projectView();
  if (!projected) return false;
  const keeps = readFilterKeeps();
  const groups = glom(projected.filter((message) => !hidingRead() || keeps(message)));
  const rows = el("discord-log").children;
  if (rows.length !== groups.length) return false;
  for (let i = 0; i < groups.length; i += 1) {
    const drawn = rowMessages(rows[i] as Element);
    const group = groups[i] as Message[];
    if (drawn.length !== group.length || group.some((message, at) => drawn[at] !== message)) return false;
  }
  return true;
}

/**
 * Draw a page: a newest page, an older one (`older`), saved rows (`saved`), or a delta the caller
 * has already folded into the store (`delta`, `#203 incremental-refresh`). A delta is drawn exactly
 * as a refresh that keeps the reader's place is — the store's projection of the view — but it says
 * nothing about older history, so the walk back keeps its cursor, and it names only the archived
 * rows among its own. `rows`, with `saved`, draws those rows of the store in place of a projection:
 * a view no page has covered yet, drawn from what the store holds for it (`drawHeldRows`). `held`, for
 * a newest page, is the store's copies as its read was sent (`heldNow`): it is folded only over the
 * copies older than it (`pageFold`, `#310 stale-live-read-flicker`).
 *
 * @param {any} payload
 * @param {boolean} [older]
 * @param {boolean} [saved]
 * @param {boolean} [delta]
 * @param {any[] | null} [rows]
 * @param {Map<string, any> | null} [held]
 */

function applyTimelinePage(payload: TimelineResponse, older = false, saved = false, delta = false, rows: readonly Message[] | null = null, held: Map<string, Message> | null = null): Message[] {
  if (!saved) observeOutgoingMessages(payload.messages || []);
  const previousCount = selection().view === "threads" ? timelineRows().threads.length : timelineRows().messages.length;
  const incoming = selection().view === "threads" ? payload.threads || [] : payload.messages || [];
  // Into the channel's store, and this view back out of it: what the reader sees is the store's
  // projection, so a row another view corrected or the stream delivered is here too.
  if (!saved && !delta) foldTimelinePage(payload, older, selection().view, selection().threadId, held);
  const merged = (rows || projectView() || []) as Message[];
  if (selection().view === "threads") timelineRows().threads = merged as unknown as ThreadSummary[];
  else timelineRows().messages = merged;
  // Retain the oldest cursor when a refresh retained the history already walked back to.
  if (saved) {
    // Saved on the device, a cursor is never kept, so whether older history exists is unknown
    // until a read supplies one — the seam says so and the walk-back control waits. A projection
    // of a view read in this page has its live cursor.
    olderPages().moreAbove = payload.has_more === true ? (payload.next_before ? true : undefined) : false;
    olderPages().cursor = payload.next_before || null;
  } else if (!delta && (older || merged.length <= incoming.length || previousCount === 0 ||
      // No cursor to retain — saved rows, or Main drawn from All's cover (`walkCursorOwed`, `#220
      // view-switch-instant`), whose history reaches below this page: walking back from the page's
      // own cursor re-reads rows the store holds, which only add, rather than never walking at all.
      (olderPages().moreAbove !== false && !olderPages().cursor))) {
    olderPages().moreAbove = payload.has_more === true;
    olderPages().cursor = payload.next_before || null;
    channelReads.oweCursor(payload.has_more === true && !payload.next_before);
  }
  const cover = channelStore.current().views.get(viewKey());
  if (cover && !saved && !delta) cover.cursor = olderPages().cursor;
  threadPresence.note(payload.has_threads, selection().view === "thread");
  if (payload.thread) selection().thread = payload.thread;
  noteArchived(payload, !delta && !older && merged.length <= incoming.length);
  const keeps = readFilterKeeps();
  const shown = timelineRows().messages.filter((message) => !hiddenEmpty(message) && (!hidingRead() || keeps(message)));
  if (older) {
    // Keep existing elements attached to the same messages: scroll restoration holds one of
    // those elements as its anchor, and recreating it would throw away the reader's position.
    const list = el("discord-log");
    const heldIds = new Set([...list.children].flatMap(idsOf));
    list.replaceChildren(...glom(shown.filter((m) => !heldIds.has(String(m.id)))).map(discordNode), ...list.children);
    const cards = el("thread-list");
    const heldThreads = new Set([...cards.children].map((row) => row.getAttribute("data-context-id")));
    cards.replaceChildren(...threadCards(timelineRows().threads.filter((t) => !heldThreads.has(String(t.id)))), ...cards.children);
  } else {
    // The reader's text selection is kept across the rebuild, as at a landing (`#1209`).
    const list = el("discord-log");
    const keptSelection = deps.selectionIn ? deps.selectionIn(list) : null;
    patchRows(list, glom(shown), (group) => discordNode(group), [], (drawn, incoming) => drawn === incoming, selection().view);
    el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
    if (keptSelection && deps.restoreSelectionIn) deps.restoreSelectionIn(list, keptSelection);
  }
  // The arrows are measured once, by the pass over the search every caller ends with. `#226`.
  holdingReplyArrows(renderChannelRows);
  renderChannelNavigation();
  renderOlderControl();
  const notice = allNoticeWords(payload.notice || "", {
    threading: providerFacts.threading(), view: selection().view,
    messages: timelineRows().messages, tally: () => replyTally(),
  });
  el("timeline-notice").textContent = notice;
  el("timeline-notice").hidden = !notice;
  el("inbox-note").textContent = (payload as { read_state_notice?: string }).read_state_notice || "";
  listState().backlog = shown.length;
  renderTodoControls();
  el("clear-backlog").hidden = true;
  const label = selection().view === "threads"
    ? `${timelineRows().threads.length} recent thread${timelineRows().threads.length === 1 ? "" : "s"}`
    : channelSummary(shown.length, olderPager.loadedIsWhole(olderPages().moreAbove), selection().view === "thread"
      ? openThreadName() : channelName(payload.channel));
  renderChannelSeam(label);
  return (selection().view === "threads" ? timelineRows().threads : shown) as Message[];
}


/**
 * `preservingScroll` for a redraw that REBUILDS the rows, which takes its anchor with it: the reader
 * is held on the same MESSAGE, found again by id, or on the line of the collapsed run it has joined.
 * `#221 read-modes`: a message dealt with under Fold read joins a run, a mode change collapses
 * or hides whole runs above the reader, and a pixel offset kept across either lands on whatever
 * message has moved into that pixel. Kept to the offset as before when the message is not drawn.
 */

function renderCachedTimeline(): void {
  if (!providerFacts.threading() || selection().view === "threads") return;
  const keeps = readFilterKeeps();
  const shown = timelineRows().messages.filter((message) => !hiddenEmpty(message) && (!hidingRead() || keeps(message)));
  const redraw = () => {
    // The selection is kept across the rebuild (`#1209 selection-survives-updates`), and the
    // rebuild itself patches in place (`#1229 keyed-row-patching`).
    const list = el("discord-log");
    const keptSelection = deps.selectionIn ? deps.selectionIn(list) : null;
    patchRows(list, glom(shown), (group) => discordNode(group), [], (drawn, incoming) => drawn === incoming, selection().view);
    if (keptSelection && deps.restoreSelectionIn) deps.restoreSelectionIn(list, keptSelection);
    holdingReplyArrows(renderChannelRows);
    listState().backlog = shown.length;
    renderTodoControls();
    renderChannelSeam(channelSummary(
      shown.length,
      olderPager.loadedIsWhole(olderPages().moreAbove),
      selection().view === "thread"
        ? openThreadName()
        : channelName(knownChannel(selectedChannelId()))
    ));
  };
  holdingMessage(redraw);
  renderScrollTools();
  requestVisibleSummaries();
  guardQuietly(readAloudOwner().prepareSpeech)();
}

// --- reading only what changed -------------------------------------------------------------------
//
// `#203 incremental-refresh`. The owner: "This is the common case inner loop for how we use the
// application all the time. It is important for this to be efficient. Right now my refreshes feel
// very slow." Every refresh used to re-read the newest page of the view, and behind a bridge that
// could mean re-scanning a channel's whole history. Now a newest read hands back `next_after`, and
// a refresh with that cursor asks only for what changed since: a DELTA, folded into the store
// without the newest page's deletion-by-absence, which a delta must never apply.
//
// WHICH READ. A full read on entering a view, changing channel, a stream reset, an expired or
// mismatched cursor, a delta that runs past `TIMELINE_DELTA_PAGES`, and every
// `TIMELINE_RECONCILE_MS`. Otherwise a delta — as far as the channel's deltas reach. A bridge with a
// change record answers COMPLETE deltas, edits and deletions included, so every refresh there is
// one. Any other backend answers additions only, so its channel keeps the full poll it always had
// and its edits and deletions are no slower; deltas there serve a live arrival and a return to the
// page, where speed is the point.

// How old a view's last newest read may be (`TIMELINE_RECONCILE_MS`), how many delta pages one refresh
// follows (`TIMELINE_DELTA_PAGES`), how long a hidden page refreshes after (`VISIBLE_REFRESH_MS`), when
// the pill says the service is slow (`STALE_AS_OF_MS`) and why a read may be a delta (`DELTA_REASONS`)
// are web/src/core/sync's, with the choice of cursor (`#624 voice-page-modularity`).

/** Whether an edit or removal from the replay tail is waiting for a read to answer it. */

function mutationAwaitsRead(): boolean {
  return replayLedger.mutationAwaits();
}



// Whether the server can no longer continue from a cursor (`isCursorRefusal`) is web/src/core/sync's.

/**
 * After a page the server answered from its own copy, the next read comes at the normal poll
 * cadence: whether a stand-in answer ever shortens that wait is web/src/core/sync's
 * (`createStandInRetries`, `#624 voice-page-modularity`); since `#1227 poll-storm-stale-thread`
 * it never does — the server refreshes on its own schedule and pushes what it brings.
 *
 * @type {import("./src/core/sync/index.ts").StandInRetries}
 */

async function loadTimeline(options?: TimelineReadOptions): Promise<void> {
  if (disposed) return;
  // `remote-ops-audit` W7. No poll while a read of this view the page gave up on may still be running at
  // the server. Every other read goes.
  if (abandonedReads.held(abandonedReads.holdKey(), options)) return;
  const generation = ++loadState().generation;
  // `#229 desktop-two-column`. The column this read is for: its work after every wait is done there.
  const steps = columnSteps();
  const context = channelContextKey();
  const holdKey = abandonedReads.holdKey();
  const channel = String(selectedChannelId());
  const [view, thread, canon] = [selection().view, selection().threadId, channelStore.current()];
  if (!channel) return;
  if (!channelReads.claim(context)) {
    queueDiscordLoad(options);
    return;
  }
  const read = replayLedger.start();
  const area = el("scroll-area");
  const position = channelReadPosition(area);
  const keepPosition = Boolean(options && options.keepPosition);
  // `#220 view-switch-instant`. A read behind rows a view switch drew: no loading line over them,
  // and what it adds was only not drawn before, not something that arrived. Asked of a list with
  // rows in it: a quiet read queued with one for a view that has nothing to draw shows the line.
  const ownAct = Boolean(options && options.ownAct);
  const rowsUp = el(view === "threads" ? "thread-list" : "discord-log").children.length > 0;
  // `#226 page-energy-profile`. A refresh nobody asked for, over rows on screen, shows its line only
  // once it has been out for `LOADING_LINE_DELAY_MS`: the pill already says "refreshing…".
  renderChannelLoading(true, Boolean(options && options.quiet) && rowsUp, rowsUp && backgroundRead(options));
  /** @type {VibeTalk.TimelineResponse[]} every page this refresh folded, in order */
  const landed: TimelineResponse[] = [];
  /** @type {VibeTalk.TimelineResponse[]} the delta pages among them, still to be drawn */
  const deltas: TimelineResponse[] = [];
  let sent = timelineReader.forwardCursor(options && options.reason);
  // What a refresh asking for this view again soon after may count as read already. `#231`.
  const record = { context, read, full: sent === null };
  columnReadsSent.set(steps.column, record);
  const sameStore = () => canon === channelStore.current() && canon.channel === channel &&
    String(selectedChannelId()) === channel;
  const stillCurrent = () =>
    inColumn(steps.column, () => generation === loadState().generation && context === channelContextKey());
  // `#220 view-switch-instant`. Rows a switch drew from the store for a view no page has covered,
  // read behind them (`drawHeldRows`): the newest page replaces them, and the message the reader is
  // on may be older than that page reaches.
  const behindHeld = keepPosition && view !== "threads" && sameStore() && !canon.views.has(viewKey(view, thread)) &&
    timelineRows().messages.length > 0;
  /** Draw the delta pages folded so far, as the one refresh they are. */
  const drawDeltas = () => {
    renderChannelLoading(false);
    // `#226 page-energy-profile`. Nothing changed, and every row on screen is the row a redraw
    // would draw: the refresh says how fresh the rows are and draws nothing.
    if (keepPosition && !ownAct && entryState().readOwed !== context &&
        deltas.every((page) => unchangedDeltas.has(page)) && rowsAsDrawn(deltas)) {
      timelineRows().messages = projectView() || timelineRows().messages;
      observeOutgoingMessages(deltas.flatMap((page) => page.messages || []));
      noteFreshRead(deltas[deltas.length - 1] as TimelineResponse);
      saveChannelScope();
      // The thread picker's ages ("13h · 16") are one thing on screen a clock moves; the days
      // between the rows, past midnight, are the other (`#250 date-separators`).
      renderChannelNavigation();
      seamsAfterMidnight();
      return;
    }
    // Where the reader is now, not where they were when the read went out. `#280 scroll-stability`.
    const at = landedReadPosition(area, keepPosition, position.wasAtNewest);
    // One pass over the scroll tools, at the end, however often settling asks. `#226`.
    holdingScrollTools(() => {
      const messages = applyTimelinePage(timelineReader.joinedDeltas(deltas), false, false, true);
      settleAfterRead(messages as readonly Message[], { area, ...at, ownAct });
      noteFreshRead(deltas[deltas.length - 1] as TimelineResponse);
      saveChannelScope();
      renderScrollTools();
    });
    requestVisibleSummaries();
    if (entryState().readOwed === context) entryState().readOwed = null;
    payThreadEntry();
    // What it changed in the store may show in the other column too. `#229 desktop-two-column`.
    followInOtherColumn();
  };
  try {
    for (;;) {
      const path = timelineReader.timelinePath(null, sent, timelineReader.readReasonOf(options ? { readAs: options.readAs, reason: options.reason } : options));
      // What the stream had left unplaced when this went out, which a newest page of All answers for.
      const seenArrivals = unplacedArrivals.count();
      // `#310 stale-live-read-flicker`. And the store's copies as it went out: a newest page is folded
      // only over the copies older than it (`pageFold`). A message the stream drew while it was out is
      // newer than it, and this page, landing after, redrew the column without it until the next read
      // put it back.
      const held = heldNow(canon);
      /** @type {VibeTalk.TimelineResponse} */
      let payload;
      try {
        steps.pause();
        // `remote-ops-audit` W7. Given up on, the read is kept, and its view held (`abandonRead`).
        const abandoned = { key: holdKey, context, channel, view, thread, canon, column: steps.column, sent, held,
          sentAt: Date.now(), live: liveEvents.seen(channel), news: liveEvents.news(channel), number: 0 };
        const asked = within(CHANNEL_READ_TIMEOUT_MS, (signal) => timelineReader.readTimeline(path, { signal }),
          // Once it settles, the placing read that waited for it goes (`placeUnplacedWaits`). `#308`.
          (answer) => abandonedReads.abandon(answer, abandoned, (late) => landAbandonedTimelineRead(late, abandoned),
            queuePlaceUnplaced));
        abandoned.number = readOrder.asked();
        payload = await asked;
        steps.resume();
      } catch (error) {
        steps.resume();
        // A cursor the server can no longer continue is not a failed refresh. Forget it and read
        // the newest page now, in the same hold: the pill says "refreshing…" and nothing else.
        if (sent && isCursorRefusal(error) && sameStore()) {
          const cover = canon.views.get(viewKey(view, thread));
          if (cover && cover.newest === sent) cover.newest = null;
          sent = null;
          continue;
        }
        throw error;
      }
      const current = generation === loadState().generation && context === channelContextKey();
      // Folded by what ARRIVED, never by what was asked: a bridge that ignored `after` answers
      // with the newest page it is, and that is folded as one.
      if (sent && payload.delta) {
        if (!sameStore() || !foldTimelineDelta(payload, sent, view, thread)) {
          // Overtaken partway through a chain — the thread selector's own read of the list moved
          // its cursor on meanwhile. What the pages before this one brought is in the store
          // already, so it is drawn now rather than left off screen until the next read.
          if (deltas.length && current && sameStore()) drawDeltas();
          return;
        }
        landed.push(payload);
        // `#308 live-reply-in-main`. All's changes, to the last of them, say where every message
        // they carry is, and that one they do not carry is not new in All.
        if (view === "flat" && !payload.delta.more) answerUnplaced(canon, seenArrivals);
        if (!current) {
          // Read for a view the reader has since left, or overtaken by a read queued behind it:
          // the store has it, so that view is current from its next showing.
          (canon.views.get(viewKey(view, thread)) as { landedHidden: boolean }).landedHidden = true;
          if (context === channelContextKey()) {
            // Words and all, unless that read is queued behind this one: it draws them.
            catchUpHeldView(!channelReads.queued(steps.column));
            renderChannelFreshness();
          } else if (view === "flat") {
            catchUpFromAll();
          }
          // A read of All or of a thread brings replies, and their read state, that Main counts.
          syncUnreadReplies();
          followInOtherColumn();
          saveChannelScope();
          return;
        }
        deltas.push(payload);
        if (payload.delta.more) {
          if (deltas.length < TIMELINE_DELTA_PAGES && payload.next_after) {
            sent = payload.next_after;
            continue;
          }
          // Still more after that many pages: read the newest page, AFTER dropping the view's
          // cover, as an explicit refresh does. The store keeps its rows, but the new page starts
          // coverage afresh instead of claiming continuity across what was not read.
          timelineRows().messages = [];
          timelineRows().threads = [];
          olderPages().cursor = null;
          channelReads.payCursor();
          olderPages().moreAbove = false;
          channelStore.current().views.delete(viewKey());
          deltas.length = 0;
          sent = null;
          continue;
        }
        drawDeltas();
        return;
      }
      landed.push(payload);
      // `#308 live-reply-in-main`. All's newest page has said where every message it carries is, and
      // that one it does not carry is not there to be placed.
      // Not a page the server answered from its own copy (`#611`), which may lag what arrived.
      if (view === "flat" && sameStore() && !refreshFailed(payload)) answerUnplaced(canon, seenArrivals);
      // Under rows drawn from the store, a reader off the newest line keeps their message: the view
      // is walked back to it before anything is drawn, so the list changes once, under them. The
      // message they are on NOW, which the wait for this page gave them time to change. `#277`.
      const reading = behindHeld && current ? channelReadPosition(area) : position;
      const anchored = behindHeld && current && !reading.wasAtNewest && reading.anchorId
        ? timelineRows().messages.find((message) => String(message.id) === reading.anchorId) : null;
      const placeAt = timeOf(anchored, "timestamp");
      steps.pause();
      const older = Number.isFinite(placeAt)
        ? await inColumn(steps.column, () => pagesBackTo(payload, placeAt, stillCurrent)) : [];
      steps.resume();
      if (!current || !stillCurrent()) {
        foldLateTimelinePage(payload, { context, channel, view, thread, canon, column: steps.column, held });
        return;
      }
      // Remove the in-flow indicator before measuring or restoring scroll. In
      // real browsers scrollTop is clamped when it disappears, but making the
      // order explicit also keeps the viewport model deterministic.
      renderChannelLoading(false);
      // Where the reader is now — among the held rows, which the walk gave them time to change, or
      // in whatever list was up while the read was out. `#280 scroll-stability`.
      const landedAt = landedReadPosition(area, keepPosition, position.wasAtNewest);
      const at = behindHeld ? { ...landedAt, heldPlaceAt: placeAt } : landedAt;
      // ...unless what is on screen came from the device or a read that failed: those rows are what
      // the reader has, and the refresh MERGES into them rather than replacing them with one page.
      // Not for a page the server answered from its own copy (`#611 timeline-read-failures`): what is
      // on screen stands, as it does when a read fails.
      if (!keepPosition && loadState().freshness === "fresh" && !refreshFailed(payload)) {
        // An explicit refresh starts a fresh provider snapshot. Retaining old pages here would
        // retain their expiring cursor forever, even though the newest request made a new one.
        timelineRows().messages = [];
        timelineRows().threads = [];
        olderPages().cursor = null;
        olderPages().moreAbove = false;
        channelStore.current().views.delete(viewKey());
      }
      // A thread entered on rows the store held has landed on its first unread among them: asked
      // BEFORE the rows are rebuilt, which takes the landed row with it. `#221 read-modes`. So has
      // one entered on rows it held for replies they lack (`entryState().readOwed`, `#222 unread-replies`).
      const owed = entryState().readOwed === context;
      const landing = behindHeld || owed ? entryLandingMark() : null;
      holdingScrollTools(() => {
        let messages = applyTimelinePage(payload, false, false, false, null, sameStore() ? held : null);
        for (const page of older) messages = applyTimelinePage(page, true);
        settleAfterRead(messages as readonly Message[], { area, ...at, ownAct });
        noteFreshRead(payload);
        saveChannelScope();
        renderScrollTools();
      });
      requestVisibleSummaries();
      // A thread entered with nothing of it on the page lands here, on its first read, and one landed
      // on held rows is landed again on what the read drew: LAST, once the freshness pill and the
      // chips it is measured against are up, as on every other entry. `#221 read-modes`.
      if (owed) entryState().readOwed = null;
      if (landing) keepEntryLanding(landing);
      payThreadEntry();
      followInOtherColumn();
      return;
    }
  } catch (error) {
    if (generation === loadState().generation && context === channelContextKey()) {
      // A landing left to this read is not paid by a later one: that finds a reader who has been
      // reading the rows the failure left up. `#221 read-modes`.
      if (entryState().threadAwaitsRead) entryState().threadOwed = null;
      entryState().threadAwaitsRead = false;
      if (entryState().readOwed === context) entryState().readOwed = null;
      noteChannelReadFailure(error);
      throw error;
    }
  } finally {
    settleReplays(read, timelineReader.landedAnswer(landed));
    // A read that brought nothing back answers for nothing.
    if (!landed.length && columnReadsSent.get(steps.column) === record) columnReadsSent.delete(steps.column);
    steps.pause();
    await finishDiscordLoad(steps.column);
  }
}

/**
 * The reader switched views while this newest page was on its way. It is still the newest page of
 * the view it was read for, so the store keeps it — the same store, for the same channel and
 * token: a cleared screen or another channel is a new store, and the page is dropped. Without
 * this, a switch during the cold-start refresh would leave that view "saved" until the poll.
 * `read.held`, when the read gives it, is the store's copies as it was sent (`pageFold`).
 */

function foldLateTimelinePage(payload: TimelineResponse, read: TimelineFoldRead): void {
  if (!providerFacts.threading() || channelStore.current() !== read.canon || read.canon.channel !== read.channel ||
      String(selectedChannelId()) !== read.channel) return;
  // Asked in the column the read was for, which is the one whose view it may be. `#229`.
  if (read.column && read.column !== columnWork.active()) {
    inColumn(read.column as TimelineColumn, () => foldLateTimelinePage(payload, read));
    return;
  }
  foldTimelinePage(payload, false, read.view, read.thread, read.held || null);
  (channelStore.current().views.get(viewKey(read.view, read.thread)) as { landedHidden: boolean }).landedHidden = true;
  if (read.context === channelContextKey()) {
    // Back on the view it was read for: that view is current now, not only on its next showing.
    // `#221 read-modes`. The read behind held rows that another read overtook — a live message, the
    // poll, the page shown again, or the reader out of the thread and back before it landed — is
    // still that read: a landing made on the held rows is kept through it, and one left to it is paid.
    // On what the page says is read, as that read would have drawn it: the first unread message and
    // the runs of read messages are decided by it.
    const landing = entryLandingMark();
    noteArchived(payload, false);
    // Words and all, unless a read of this view is queued behind this one: it draws them.
    catchUpHeldView(!channelReads.queued(columnWork.active()));
    renderChannelFreshness();
    if (entryState().readOwed === read.context) entryState().readOwed = null;
    if (landing) keepEntryLanding(landing);
    payThreadEntry();
  } else if (read.view === "flat") {
    // All's page answers for Main, and for a thread drawn from All, now on screen — in either column.
    catchUpFromAll();
    inOtherColumn(catchUpFromAll);
  }
  // A read of All or of a thread brings replies, and their read state, that Main counts.
  syncUnreadReplies();
  if (read.view !== "flat") followInOtherColumn();
  saveChannelScope();
}

/**
 * The older pages of the view `payload` is the newest page of, read until one reaches back to
 * `placeAt` — the time of the message the reader is on — for at most `REPLY_JUMP_PAGES`, while
 * `current()` holds. `#220 view-switch-instant`. A step that fails ends the walk: what was read is
 * drawn, and the reader is put as near their message as that reaches (`settleAfterRead`).
 *
 * @param {VibeTalk.TimelineResponse} payload
 * @param {number} placeAt
 * @param {() => boolean} current
 * @returns {Promise<VibeTalk.TimelineResponse[]>}
 */

async function pagesBackTo(payload: TimelineResponse, placeAt: number, current: () => boolean): Promise<TimelineResponse[]> {
  const steps = columnSteps();
  /** @type {VibeTalk.TimelineResponse[]} */
  const pages: TimelineResponse[] = [];
  let last = payload;
  try {
    while (pages.length < REPLY_JUMP_PAGES && last.has_more === true && last.next_before &&
        !(timeOf((last.messages || [])[0], "timestamp") <= placeAt)) {
      const path = timelineReader.timelinePath(last.next_before, null, "older");
      try {
        steps.pause();
        last = await within(CHANNEL_READ_TIMEOUT_MS, (signal) => timelineReader.readTimeline(path, { signal }));
        steps.resume();
      } catch (_error) {
        steps.resume();
        break;
      }
      if (!current()) break;
      pages.push(last);
    }
    return pages;
  } finally {
    steps.pause();
  }
}

/**
 * The first step back in a view with no cursor to take it from (`walkCursorOwed`): the view's own
 * newest page, read behind its rows, keeping the reader's place. The control says the step is under
 * way meanwhile, as for any step. `#220 view-switch-instant`.
 */

async function readWalkCursor(): Promise<void> {
  const steps = columnSteps();
  olderPages().inFlight = true;
  renderOlderControl();
  try {
    steps.pause();
    await loadDiscord({ keepPosition: true, quiet: true, reason: "enter", readAs: timelineReader.olderReadReason() });
    steps.resume();
  } catch (error) {
    steps.resume();
    if (!(error && (error as { shownInPill?: boolean }).shownInPill)) throw error;
  } finally {
    olderPages().inFlight = false;
    renderOlderControl();
    steps.pause();
  }
}

/** @param {AbortSignal} [signal] Stops the read on the wire: see `loadOlder`. */

async function loadOlderTimeline(signal?: CancelSignal): Promise<void> {
  if (!olderPages().cursor && olderPages().moreAbove && !olderPages().inFlight && walkCursorOwed()) {
    await readWalkCursor();
    return;
  }
  if (!olderPages().moreAbove || !olderPages().cursor || olderPages().inFlight) return;
  // `#229 desktop-two-column`. The column this step is for, which its work after the wait is done in.
  const steps = columnSteps();
  const context = channelContextKey();
  const generation = loadState().generation;
  olderPages().inFlight = true;
  renderOlderControl();
  try {
    const path = timelineReader.timelinePath(olderPages().cursor, null, timelineReader.olderReadReason());
    steps.pause();
    let payload;
    try {
      payload = await timelineReader.readTimeline(path, signal ? { signal } : undefined);
    } finally {
      steps.resume();
    }
    if (context !== channelContextKey() || generation !== loadState().generation) return;
    olderPages().inFlight = false;
    preservingScroll(() => {
      applyTimelinePage(payload, true);
      // `#262 search-load-older`. The search's pass over the rows that just arrived, inside the same
      // anchored mutation, where a filter chooses rows: drawn and only then hidden, they would take
      // their height away outside it, and a reader who is not at the head of the list — one who
      // scrolled away from a walk for the search — would be moved by it, or dropped at the foot.
      if (readRunsSuspended()) applySearch();
    });
    saveChannelScope();
    renderScrollTools();
    requestVisibleSummaries();
  } catch (error) {
    // Stopped by the walk that asked for it: no failure, and nothing to draw. `#262 search-load-older`.
    if (signal && signal.aborted) return;
    if (generation !== loadState().generation || context !== channelContextKey()) return;
    if (/cursor.*expired|snapshot.*expired/i.test((error as { detail?: string; message?: string }).detail || (error as { message?: string }).message || "")) {
      olderPages().inFlight = false;
      olderPages().cursor = null;
      olderPages().moreAbove = false;
      steps.pause();
      try {
        await inColumn(steps.column, () => loadTimeline({ keepPosition: false, reason: "expired" }));
      } finally {
        steps.resume();
      }
      if (context === channelContextKey()) {
        setStatus("This history snapshot expired. Refreshed the newest messages; scroll up to load older history again.");
      }
      return;
    }
    throw error;
  } finally {
    olderPages().inFlight = false;
    renderOlderControl();
    steps.pause();
  }
}

/** Save before clearing an editor; storage refusal never prevents an intentional send (`outbox`). */

function applyNewestPage(payload: TimelineResponse, saved = false): number {
  const list = el("discord-log");
  const messages = payload.messages || [];
  if (!saved) observeOutgoingMessages(messages);
  const oldest = messages.length ? (messages[0] as Message).id : null;
  // Compared on the row's NEWEST constituent, so a combined row is kept only when the whole of it
  // is older than this page — otherwise a message would be on screen twice, once in each row.
  const newestOf = (li: Element) => {
    const ids = idsOf(li);
    return ids.length > 0 ? ids[ids.length - 1] : li.getAttribute("data-id");
  };
  const before = [...list.children].filter(isMessageRow);
  const kept =
    payload.has_more === true && oldest
      ? [...list.children].filter((li) => olderPager.snowflakeOlder(newestOf(li), oldest))
      : [];
  // `#578 search-status-clarity`. Every message row kept is older than the whole page: none of it
  // reaches down into what was loaded, so messages may lie between that no read brought, and the rows
  // kept are scattered below the page (`historyReach`). Once a gap, always one, until a fresh list.
  const keptRows = kept.filter(isMessageRow);
  plainReach.noteNewest(saved, keptRows.length, before.length, messages.length,
    messages.length > 0 ? (messages[0] as Message).timestamp : undefined);
  // The reader's text selection is kept across the rebuild, as at a landing (`#1209`).
  const keptSelection = deps.selectionIn ? deps.selectionIn(list) : null;
  patchRows(list, glom(messages), (group) => discordNode(group), kept,
    (drawn, incoming) => drawn === incoming || sameMessage(drawn, incoming), selection().view);
  if (keptSelection && deps.restoreSelectionIn) deps.restoreSelectionIn(list, keptSelection);
  // The archive BEFORE the derivation that reads it. `kept` rows were already on screen and their
  // ids are still in the set from the read that brought them, so a reset here would un-grey them.
  noteArchived(payload, kept.length === 0);
  // AFTER the rows exist: every row's replied state and speaker are facts about the list as it now
  // stands rather than about the page that just arrived.
  renderChannelRows();
  // Only when nothing was kept. If older rows survived, the cursor that belongs to them is the one
  // the older walk left behind, and overwriting it with this page's would rewind the walk.
  if (kept.length === 0) {
    olderPages().moreAbove = olderPager.normaliseHasMore(payload.has_more);
    olderPages().cursor = payload.next_before || null;
  }
  renderOlderControl();
  return list.children.length;
}

/**
 * The step back under way, or null: a column's `olderPages().step`.
 *
 * `#204 reply-arrow`. A second caller while a step is in flight is handed THAT step rather than
 * turned away. The automatic trigger and the button never notice the difference — they asked for
 * a step, and one is coming — but the walk a reply's arrow makes counts the pages it reads, and
 * one turned away would have counted a page it never saw and looked for its message too early.
 */

/**
 * One step further back.
 *
 * Guarded against re-entry rather than debounced: the automatic trigger fires on every scroll
 * event, and a phone produces a lot of those.
 *
 * `signal`, `#262 search-load-older`: the walk for the search stops its read on the wire with it. A
 * step aborted that way ends quietly, as no failure. A scroll that asks while it is under way is handed
 * the same step, and goes with it.
 *
 * @param {AbortSignal} [signal]
 */

function loadOlder(signal?: CancelSignal): Promise<void> | null {
  // A click's event is not a signal.
  const abort = signal && typeof signal === "object" && "aborted" in signal ? signal : undefined;
  if (olderPages().step === null) {
    // The step is this column's, and so is forgetting it, in the task it ends in. `#229`.
    olderPages().step = (providerFacts.threading() ? loadOlderTimeline(abort) : loadOlderPage(abort)).finally(inThisColumn(() => {
      olderPages().step = null;
    }));
  }
  return olderPages().step;
}

/**
 * `loadOlder` for a provider without threads: one step of the plain channel page.
 *
 * @param {AbortSignal} [signal]
 */

async function loadOlderPage(signal?: CancelSignal): Promise<void> {
  if (olderPages().moreAbove !== true || !olderPages().cursor || olderPages().inFlight) {
    return;
  }
  const channel = selectedChannelId();
  const generation = loadState().generation;
  if (!channel) {
    return;
  }
  olderPages().inFlight = true;
  renderOlderControl();
  try {
    let payload;
    try {
      payload = await api(
        `/api/v1/channels/${encodeURIComponent(channel)}/page` +
          `?limit=${DISCORD_PAGE_LIMIT}&before=${encodeURIComponent(olderPages().cursor as string)}&render=html` +
          `&reason=${timelineReader.olderReadReason()}`,
        signal ? { signal } : undefined
      );
    } catch (error) {
      // Stopped by the walk that asked for it: no failure, and nothing to draw. `#262`.
      if (signal && signal.aborted) return;
      throw error;
    }
    if (generation !== loadState().generation || channel !== selectedChannelId()) return;
    // `#427 server-thread-hues`: the step back's colours, before its rows are built.
    noteServerThreadHuesIn(payload.messages || [], [], String(channel));
    observeOutgoingMessages(payload.messages || []);
    const list = el("discord-log");
    const arriving = glom(payload.messages || []).map(discordNode);
    // The step is OVER before the anchored mutation, so that every consequence of it — the rows,
    // the summary and the control's final state — is one change of height rather than three. The
    // block below is synchronous, so there is no window in which a second step could start.
    olderPages().moreAbove = olderPager.normaliseHasMore(payload.has_more);
    olderPages().cursor = payload.next_before || null;
    olderPages().inFlight = false;
    // Prepending is a mutation ABOVE the viewport, which is the one case browser scroll anchoring
    // does not handle. Same helper as the fold control, deliberately.
    // Additive: this step PREPENDS, so the rows already on screen keep the archive they arrived
    // with and these older ones bring their own.
    noteArchived(payload, false);
    preservingScroll(() => {
      list.replaceChildren(...arriving, ...list.children);
      // Inside the anchored mutation with everything else that changes height. A step back can
      // reveal the QUESTION a loaded answer belongs to, so this is not merely bookkeeping for the
      // new rows — older rows already on screen can become "replied" because of them.
      renderChannelRows();
      // Re-stated inside the SAME anchored mutation. It sits above everything that just arrived,
      // so rewriting it afterwards would be a second change of height above the viewport and the
      // reader would move by whatever the difference happened to be.
      renderChannelSeam(
        channelSummary(list.children.length, olderPager.loadedIsWhole(olderPages().moreAbove), channelName(payload.channel))
      );
      // `#262 search-load-older`. And the search's pass over them, where a filter chooses rows, as in
      // `loadOlderTimeline`: rows drawn and only then hidden would move a reader not at the head.
      if (readRunsSuspended()) applySearch();
      // ...and so is this, for exactly the same reason and one nobody photographs: the LAST step
      // of the walk HIDES #load-older, which is a sibling above the log inside the scrolling
      // element. Taking its height away outside the anchor jerks the reader by the height of a
      // button on the one step where they have finally arrived at the beginning.
      renderOlderControl();
    });
    saveChannelScope();
    renderScrollTools();
    // The rows that just arrived above the viewport are candidates too, and the reader is right
    // at the top of them. `#49 cached-summaries`.
    requestVisibleSummaries();
  } finally {
    // The success path has already done both, and doing them again is a no-op. This is here for
    // the FAILURE path, where the step must stop reporting itself in flight.
    olderPages().inFlight = false;
    renderOlderControl();
  }
}

/** The reader has arrived at the top of what is loaded. Take the next step for them. */

function maybeLoadOlder() {
  // Not at the head of the Pinned list, which is every pin already: older history adds none to it, and
  // a step taken behind it reads for a list that is not on screen. A pin older than what is loaded is
  // walked back to when the filter goes (`walkToFilterAnchor`, `#243 filter-keeps-selection`).
  if (screens.view() !== "discord" || pinsView().only || olderPages().moreAbove !== true || olderPages().inFlight) {
    return;
  }
  // `#262 search-load-older`. Nor while a filter chooses rows, where the step back offers a bounded walk
  // instead (`searchOffered`). Measured in Chromium: once a search emptied the list, each step's page,
  // drawn and then hidden by the search, moved the scroll position at the head enough to fire the next
  // step, and the page read the channel's whole history back that nobody asked for.
  if (searchOffered()) return;
  if (el("scroll-area").scrollTop > OLDER_TRIGGER_PX) {
    return;
  }
  // `#68 pull-to-refresh`. A FINGER ON THE GLASS SUSPENDS THIS STEP — not just a finger that has
  // already been recognised as a pull. Two reasons, and the second is the one that makes both
  // features reachable at once:
  //
  //   * prepending history under a finger that is mid-drag moves the ground the reader is
  //     steering by, which is the fight this design exists to avoid; and
  //   * the pull can only begin where the list has run out, so a step back that fires the instant
  //     the top comes into range — `preservingScroll` then putting the reader back at a positive
  //     offset — is a step that makes the top UNREACHABLE while any history remains. That is
  //     every channel this issue is about.
  //
  // Suspended, never dropped: the step is remembered and taken the moment the finger lifts, which
  // is what keeps `#65 scrollback-paging` automatic rather than turning it into a button.
  if (pullToRefresh.fingerDown()) {
    olderPages().deferred = true;
    return;
  }
  guardQuietly(loadOlder)();
}

/** The finger has left the glass. Take the step that was refused while it was down. */

function takeDeferredOlder() {
  if (!olderPages().deferred) {
    return;
  }
  olderPages().deferred = false;
  maybeLoadOlder();
}

// --- pulling the channel down to refresh it -----------------------------------------------------
//
// `#68 pull-to-refresh`. The owner found the channel hours out of date and reached for the gesture
// his thumb already makes: "especially when I swipe up on this view and it shows me something very
// stale." `4e3d850` fixed the staleness itself — the view re-reads on entry and polls while it is
// up — and that covers being stale AND WAITING. It gives no way to say "refresh, NOW".
//
// THE TWO GESTURES AT THIS END OF THE LIST ARE TOLD APART BY WHERE THE LIST RAN OUT, and the rule
// is two sentences long because it has to leave BOTH of them reachable on a channel that still has
// history above the reader — which is every channel this issue is about:
//
//   1. A FINGER ON THE GLASS SUSPENDS THE AUTOMATIC STEP BACK (`maybeLoadOlder` above). Deferred,
//      not dropped: it is taken the moment the finger lifts.
//   2. THE PULL'S TRAVEL IS MEASURED FROM WHERE THE LIST RAN OUT, never from where the finger
//      landed. An overscroll begins at the edge, so the pixels the finger spent getting to the top
//      are scrolling and only the pixels after it are a pull.
//
// Together those give the reader one continuous motion for each meaning, and neither is a mode:
//
//   * drag up through the history — the list scrolls; if the finger lifts within OLDER_TRIGGER_PX
//     of the top the deferred step fires and the walk back continues, `#65 scrollback-paging`;
//   * drag DOWN until the list runs out and keep going — the extra PULL_ARM_PX past the edge is
//     the overscroll, and that is the pull. `overscroll-behavior: contain` on #scroll-area is what
//     leaves that overscroll to this page rather than letting the browser's own pull-to-refresh
//     reload the whole application.
//
// The earlier reading of this — "judged by the scroll position at touchstart" — is what rule 2
// replaces, and it was wrong in a way no test then reached: arriving at the top of a paged channel
// fires the step back, `preservingScroll` restores the reader to a positive offset, and so
// `scrollTop === 0` at touchstart was a state a reader on a channel with history could never be
// in. The gesture existed only once the whole channel had been walked back.
//
// Rule 2 keeps what that reading got right, and for the reason it was chosen: a flick started a
// little below the top runs the list out within a single frame, so the first touchmove the page
// sees already reports zero. Anchoring at the edge means that flick has travelled nothing yet —
// judging it from where the finger IS would turn ordinary scrolling into a refresh.
//
// `keepPosition: false`, unlike Refresh and unlike the poll. This is a gesture made AT THE TOP of
// the history asking for what is new, and "keep my place" there means "stay at the oldest thing
// you have loaded", which is the opposite of the request. The button keeps its place because it
// is pressed from wherever the reader happens to be reading.
//
// ...AND PULLING UP PAST THE NEWEST MESSAGE. `#188 pull-refresh-bottom`. The same gesture at the
// other end of the list, which is the end the owner actually reads at: "I want pull UP to trigger
// refresh even IF messages were loaded in the past." The pull above answers only a reader at the
// TOP. Before anything has loaded, that is everyone — an empty list is at its top — which is why
// it seemed to work then; once a channel has loaded, the reader is parked at the newest line, a
// whole history away from it. Bouncing against the bottom did nothing, and showed nothing.
//
// ONE GESTURE, TWO EDGES, and the edge is whichever end of the list ran out under the finger. A
// list too short to scroll is at both ends at once, and there the direction decides: down is the
// top's pull, up is the bottom's. The axis test, the second-finger refusal, the threshold and the
// refusal during a step back are shared, because a second gesture beside this one would be a
// second set of rules about what counts as a pull, and the two would drift. What differs, and why:
//
//   * A PULL UP MUST BEGIN WITH THE READER PARKED AT THE NEWEST LINE — `atBottom` at touchstart,
//     the same test that decides whether a re-read follows them. The top cannot ask this (rule 1:
//     the step back makes "already at the top" unreachable on a paged channel), but nothing pages
//     in at the bottom, so the bottom can. It is what keeps a fling from refreshing: a drag that
//     starts in the history and runs the list out is a scroll however far the finger goes after
//     it. Only a reader who stopped at the bottom and pulled again is asking.
//   * ITS TRAVEL IS STILL MEASURED FROM WHERE THE LIST REALLY RAN OUT, which is rule 2 — to within
//     PULL_EDGE_SLACK_PX, not `atBottom`'s thumb-sized slack. The pixels inside that slack still
//     scroll, and pixels that scroll are not a pull.
//   * `keepPosition: true`, like the poll. The reader is AT the newest line, which is the one place
//     "keep my place" and "show me the newest" are the same instruction — and keeping it means a
//     refresh does not throw away the history they walked back through to get here.
//   * A READ ALREADY IN FLIGHT IS THE REFRESH. A poll, the stream's catch-up or the cold start's
//     re-read is reading what the reader asked for, so the release waits for it and reports it
//     rather than putting a second read on the wire behind it — whether it was already on the wire
//     when the finger landed or began while the finger was down. The top's pull is NOT offered
//     over a read in flight, as before: its release asks `loadDiscord` directly, which can only
//     queue behind that read and return at once, and it would report a refresh that had not
//     happened. The foot has to be offered there all the same. On a weak signal a channel read can
//     hang for as long as the connection stalls, which is just when the owner bounces against the
//     bottom, and a pull that showed nothing then would be "I don't get a refresh that shows in
//     the UI" all over again.
//   * WHAT IT REPORTS IS ABOUT THE VIEW IT WAS ASKED OVER, AND FROM THE RELEASE ON. A reader on a
//     slow read taps another tab, another thread or another channel; the read they pulled for is
//     then folded away for later, and the freshness the page restores belongs to the view they
//     went to. Said at the foot, that would be "Updated" at a time the reader never refreshed,
//     with "new messages" for rows that arrived with a different view. So the result is dropped
//     once the view is not the one pulled, and "Updated" needs a read that landed since the
//     release.
//   * IT SAYS WHAT IT FOUND, ITSELF. At the top the affordance gives way to the freshness pill,
//     which then says "Updated 07:15" or "Refresh failed" in the same place. From the bottom that
//     pill is a screen away, so the affordance says it, for PULL_RESULT_MS, and goes.
//
// Chrome's OWN pull-to-refresh is not a concern at either end, and must not become one: it
// reloads the whole application — the live stream, a call, the reader's place — to re-read a list
// this page can re-read by itself. `overscroll-behavior: none` on the document and `contain` on
// #scroll-area already keep every overscroll on this list: `contain` rather than `none`, so the
// platform's own stretch at the edge still shows, which is the bounce the owner described and a
// fair cue that the list has run out.
//
// The gesture itself — the touch it follows, its thresholds (PULL_ARM_PX, PULL_EDGE_SLACK_PX,
// PULL_RISE_PX, PULL_RESULT_MS, PULL_WAIT_MS) and the affordance's words (PULL_LABELS) — is
// web/src/view/channel's (`createPullToRefresh`, `#624 voice-page-modularity`); what a refresh reads is
// here.

function settleAfterRead(messages: readonly Message[], how: { area: HTMLElement; keepPosition: boolean; ownAct?: boolean; wasAtNewest: boolean; previousTop: number; anchorId?: string | null; anchorTop?: number; around?: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null; heldPlaceAt?: number }): void {
  const previous = listState().newestId;
  const newest = messages.length ? String((messages[messages.length - 1] as Message).id) : null;
  const moved = newest !== null && listState().newestId !== null && newest !== listState().newestId;
  const arrived = moved && how.ownAct !== true;
  listState().newestId = newest;
  // `#243 filter-keeps-selection`. A message a filter change holds in place stays where that change
  // put it, whatever this read measured when it was sent — before the change, perhaps — and whether or
  // not the reader was on the newest line then.
  if (how.keepPosition && keepFilterHold()) {
    if (arrived) {
      const at = messages.findIndex((message) => String(message.id) === previous);
      setJumpNewest(true, "discord", at >= 0 ? messages.length - 1 - at : null);
    } else {
      renderJumpNewest();
    }
    return;
  }
  if (how.keepPosition && !how.wasAtNewest) {
    // A pixel offset is not a place in a conversation: an edit or deletion above it changes which
    // message occupies that pixel. Prefer the same rendered message at the same viewport offset.
    // A deleted anchor has no semantic destination, so only that case falls back to the old offset.
    // ...or the line of the collapsed run it has joined since. `#221 read-modes`.
    // `#612 view-stability`: in the Threads list, not after a card a reply moved to the foot (`heldRowAfter`).
    const held = heldRowAfter(how);
    if (held && Number.isFinite(how.anchorTop)) {
      deps.toPosition(how.area, how.previousTop);
      deps.nudge(how.area, held.row.getBoundingClientRect().top - held.top);
    } else if (how.heldPlaceAt !== undefined) {
      // As near as the read reaches: the top, when everything it brought is newer than the reader's
      // message — the walk back goes on from there — and otherwise the newest line.
      if (!(messages.length > 0 && timeOf(messages[0], "timestamp") > how.heldPlaceAt)) {
        scrollToNewest();
        setJumpNewest(false, "discord");
        return;
      }
      deps.toPosition(how.area, 0);
    } else {
      deps.toPosition(how.area, how.previousTop);
    }
    if (arrived) {
      // How many, counted from the message that was newest before. When it is not in the list any
      // more — more arrived than one read holds, or it was deleted — only that something did.
      // `#207 scrollback-jump`.
      const at = messages.findIndex((message) => String(message.id) === previous);
      setJumpNewest(true, "discord", at >= 0 ? messages.length - 1 - at : null);
    }
  } else {
    scrollToNewest();
    setJumpNewest(false, "discord");
  }
}

/**
 * Where the reader is as a read LANDS, for `settleAfterRead`: measured then, never when the read was
 * sent. `#280 scroll-stability`. The owner, 2026-10-13: "I swear I just saw an instance where it jumped
 * me to the bottom of the screen in the main message view (two column mode)... I don't think I touched
 * anything. I think it was something updating underneath." A read is on the wire for as long as the
 * server takes — seconds, behind a bridge — and every read took the reader's place as it went out:
 * one sent while the reader was on the newest line, who then scrolled up to read, threw them back to
 * the bottom when it landed, and one sent while they were further up put them back where they had been
 * before they scrolled. Measured in Chromium (tests/scroll_stability_browser.py). What was true at the
 * send is kept for one question only: `sentAtNewest`, whether the reader was then on the newest line.
 *
 * A read that would take the reader to the newest message — entering the channel over rows already up,
 * saved on this device or drawn by an earlier read — finds them where they are when it lands too: a
 * reader who was on the newest line when it went out and has scrolled away since is reading, and is
 * left there, with the jump to the newest offered for what arrived. One who was not on the newest line
 * when it went out — a pull at the head of the list, a walk back whose snapshot expired — is taken to
 * the newest as asked, as before.
 *
 * Asked once the in-flow loading line over the rows has gone (`renderChannelLoading(false)`): the rows
 * as they stand without it, which is where a browser's own scroll anchoring has held them as it came
 * and went, and where the reader was before it came.
 *
 * @param {HTMLElement} area
 * @param {boolean} keepPosition
 * @param {boolean} sentAtNewest
 */

function landedReadPosition(area: HTMLElement, keepPosition: boolean, sentAtNewest: boolean): { wasAtNewest: boolean; keepPosition: boolean; previousTop: number; anchorId: string | null; anchorTop: number; around: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null } {
  const at = channelReadPosition(area);
  return { ...at, keepPosition: keepPosition || (sentAtNewest && !at.wasAtNewest) };
}

/**
 * Capture the channel row the reader is looking at before an authoritative replacement.
 *
 * `#612 view-stability`: in the Threads list, whose cards re-sort as replies come, `around` too — the cards on
 * screen, fully shown first, and each card's neighbours — for when the card held is the one that moved
 * (`heldRowAfter`). The lists of messages are in the order sent, and hold their reader by the row alone.
 */

function channelReadPosition(area: HTMLElement): { wasAtNewest: boolean; previousTop: number; anchorId: string | null; anchorTop: number; around: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null } {
  // A message a filter change holds in place is the one kept, and the newest line is not followed.
  // `#243 filter-keeps-selection`. A jump's landed row ahead of it: the reader's latest explicit
  // place, as `captureScroll` prefers it (`#1224 mention-jump-in-view`) — a read landing that
  // rebuilds every row re-finds it by id in `heldRowAfter`, where holding the first row on
  // screen instead would let changed rows between the two move the landed row off its place.
  const anchor = screens.view() === "discord" ? (jumpAnchorRow ? jumpAnchorRow() : null) || heldFilterRow() || scrollAnchor(area) : null;
  const anchorIds = anchor ? idsOf(anchor) : [];
  return {
    wasAtNewest: screens.view() === "discord" && followsNewest(area),
    previousTop: area.scrollTop,
    anchorId: anchor ? anchor.getAttribute("data-context-id") || anchorIds[0] || null : null,
    anchorTop: anchor ? anchor.getBoundingClientRect().top : 0,
    around: anchor && visibleList() === el("thread-list") ? threadListAround(area) : null,
  };
}

/**
 * The Threads list's cards on screen in `area`, fully shown first, with where each stands, and every card's
 * neighbours (`rowNeighbours`). `#612 view-stability`.
 *
 * @param {HTMLElement} area
 * @returns {ListAround}
 */

function threadListAround(area: HTMLElement): { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } {
  const top = area.getBoundingClientRect().top;
  const foot = top + (area.clientHeight || 0);
  const cards = /** @type {HTMLElement[]} */ ([...el("thread-list").children]).filter((row) => row.hasAttribute("data-context-id"));
  const shown = cards.filter((row) => rowShown(row)).map((row) => ({ row, box: row.getBoundingClientRect() }))
    .filter(({ box }: { box: DOMRect }) => box.bottom > top && box.top < foot);
  const whole = ({ box }: { box: DOMRect }) => box.top >= top - 1 && box.bottom <= foot + 1;
  const marks = [...shown.filter(whole), ...shown.filter((one) => !whole(one))]
    .map(({ row, box }) => ({ id: threadCardId(row), top: box.top }));
  return { marks, near: rowNeighbours(cards, threadCardId) };
}

/**
 * The cards of the Threads list on screen as a place was read: each card's id and the top of its box, fully
 * shown first, and every card's neighbours. `#612 view-stability`.
 *
 * @typedef {{marks: {id: string, top: number}[], near: Map<string, {prev: string | null, next: string | null}>}} ListAround
 */

/**
 * The row a list holds its reader by once it is drawn again, as `channelReadPosition` read the place, and the
 * top it stood at then: the same message, found by id, or the line of the collapsed run it has joined since
 * (`#221 read-modes`). Null when it is not drawn.
 *
 * `#612 view-stability`. In the Threads list, not a card that has moved, re-sorted to the foot by a reply: the first
 * card on screen then that still stands beside a card it stood beside, and where it stood. The owner: "Whatever
 * I'm looking at should not jump away."
 *
 * @param {{anchorId?: string | null, anchorTop?: number, around?: ListAround | null}} position
 * @returns {{row: Element, top: number} | null}
 */

function heldRowAfter(position: { anchorId?: string | null; anchorTop?: number; around?: { marks: { id: string; top: number }[]; near: ReturnType<typeof rowNeighbours> } | null }): { row: Element; top: number } | null {
  if (!position.anchorId) return null;
  const rows = [...visibleList().children];
  const around = position.around;
  if (around && visibleList() === el("thread-list")) {
    const now = rowNeighbours(rows.filter((row) => row.hasAttribute("data-context-id")), threadCardId);
    if (!neighbourStayed(position.anchorId, now, around.near)) {
      for (const mark of around.marks) {
        const row = rows.find((one) => one.getAttribute("data-context-id") === mark.id);
        if (row && rowShown(row) && neighbourStayed(mark.id, now, around.near)) return { row, top: mark.top };
      }
    }
  }
  const row = shownRowFor(rows.find((candidate) =>
    candidate.getAttribute("data-context-id") === position.anchorId || idsOf(candidate).includes(position.anchorId as string)));
  return row ? { row, top: Number(position.anchorTop) } : null;
}

/**
 * @param {{keepPosition?: boolean, quiet?: boolean, ownAct?: boolean, reason?: string | (string | undefined)[], readAs?: string}} [options]
 *   `keepPosition` marks a RE-read of a channel already on screen — the background poll, or the
 *   Refresh button. It must not drag the reader to the bottom while they are reading older
 *   messages; it follows the newest line only if that is where they already were. The FIRST load
 *   of a channel is the other case and does not pass it: arriving at the top of a long history
 *   means scrolling past everything already read. `reason` says what asked for the read — one of
 *   `DELTA_REASONS`, or `enter`, `reset`, `expired` — which decides whether a timeline read may ask
 *   only for what changed; absent reads in full. A queued read carries the reasons of every
 *   trigger it stands for (`queueDiscordLoad`). `#203 incremental-refresh`. `quiet` and `ownAct`
 *   are a read behind rows a view switch has already drawn (`readBehindSwitch`, `#220
 *   view-switch-instant`): it shows no loading line over them, and what it adds is not counted as
 *   arrived. `readAs` is only the word the read sends the server (`readReasonOf`), and decides nothing.
 */

async function loadDiscord(options?: TimelineReadOptions): Promise<void> {
  screenBelongsToToken();
  if (providerFacts.threading()) return loadTimeline(options);
  // `remote-ops-audit` W7. As for a timeline read (`loadTimeline`).
  const holdKey = abandonedReads.holdKey();
  if (abandonedReads.held(holdKey, options)) return;
  const generation = ++loadState().generation;
  const keepPosition = Boolean(options && options.keepPosition);
  const channel = selectedChannelId();
  if (!channel) {
    setStatus(deps.noChannelWords);
    return;
  }
  // Not a timeline read: no view's read is out (`channelReads.readContext`).
  if (!channelReads.claim(null)) {
    queueDiscordLoad(options);
    return;
  }
  const area = el("scroll-area");
  const position = channelReadPosition(area);
  renderChannelLoading(true);
  try {
    if (!keepPosition) {
      setStatus("fetching the channel…");
    }
    // `render=html`: this route is also the voice agent's `read_page`, so its bodies are rendered
    // only when asked. `#217 markdown-blocks`.
    const path = timelineReader.withReadReason(
      `/api/v1/channels/${encodeURIComponent(channel)}/page?limit=${DISCORD_PAGE_LIMIT}&render=html`, options);
    const sent = readLedger.epoch();
    // `remote-ops-audit` W7. Given up on, the read is kept, and its view held (`abandonRead`).
    const abandoned = { key: holdKey, channel: String(channel), sentAt: Date.now(),
      live: liveEvents.seen(String(channel)), news: liveEvents.news(String(channel)), canon: null,
      generation };
    const payload = await within(CHANNEL_READ_TIMEOUT_MS, (signal) => api(path, { signal }),
      (answer) => abandonedReads.abandon(answer, abandoned, (late) => {
        if (late && typeof late === "object") readLedger.noteSent(late, sent);
        drawChannelPage(late, area, landedReadPosition(area, true, false));
      }));
    // When it was sent, for `noteBookmark`: a bookmark write settled since speaks over it.
    if (payload && typeof payload === "object") readLedger.noteSent(payload, sent);
    if (!currentDiscordLoad(generation, channel, false)) {
      return;
    }
    renderChannelLoading(false);
    // Where the reader is now, not where they were when the read went out. `#280 scroll-stability`.
    drawChannelPage(payload, area, landedReadPosition(area, keepPosition, position.wasAtNewest));
  } catch (error) {
    if (currentDiscordLoad(generation, channel, false)) noteChannelReadFailure(error);
    throw error;
  } finally {
    await finishDiscordLoad();
  }
}

/**
 * Draw the newest page of a channel without threads that `loadDiscord` read, the reader placed by `at`
 * (`landedReadPosition`). Also the late answer of such a read given up on (`abandonRead`).
 *
 * @param {any} payload
 * @param {HTMLElement} area
 * @param {ReturnType<typeof landedReadPosition>} at
 */

function drawChannelPage(payload: TimelineResponse, area: HTMLElement, at: ReturnType<typeof landedReadPosition>): void {
  // `#427 server-thread-hues`: this walk's messages carry their threads' colours too.
  noteServerThreadHuesIn(payload.messages || [], []);
  const loaded = applyNewestPage(payload);
  // Inline, at the head of the list, rather than on the transient strip. `#63
  // status-line-placement`: this is a standing fact about what you are looking at, and the strip
  // takes itself away after a few seconds.
  renderChannelSeam(channelSummary(loaded, olderPager.loadedIsWhole(olderPages().moreAbove), channelName(payload.channel)));
  // What is drawn: an empty message the reader hides is not news at the foot (`#386 hide-empty-messages`).
  const messages = (payload.messages || []).filter((message) => !hiddenEmpty(message));
  settleAfterRead(messages, { area, ...at });
  loadState().freshAt = Date.now();
  setChannelFreshness("fresh");
  notePinsRevision(payload.pins_revision);
  noteBookmark(payload);
  saveChannelScope();
  renderScrollTools();
  // Every row here is new — `applyNewestPage` replaced the list — so the ones on screen have to
  // be asked about again. The summaries owner's `summariesAsked` is what stops that being a second
  // request for a message already answered, which matters most here: this runs every DISCORD_POLL_MS.
  requestVisibleSummaries();
}

// --- the to-do view: what you have not dealt with yet ---------------------------------------------
//
// `#50 todo-view`. A long backlog of assistant messages is a to-do list in practice, and until now
// nothing on this page could tell the ones that still want attention from the ones already handled.
//
// A SUB-TOGGLE of the channel view rather than a third tab, because it is the same list filtered.
// Turning it on reads a DIFFERENT route — `/todo`, which is the recent window minus what has been
// dealt with — rather than filtering the rows already on screen. That is deliberate: the walk-back
// cursor belongs to the unfiltered channel, and a filtered list paged by an unfiltered cursor would
// step over messages without saying so. So `#load-older` is absent in this mode, and the view is
// honestly the recent window.
//
// THE ONE THING THIS MODE HAS TO SAY OUT LOUD, from `#61 unread-status`: this read state is OURS.
// Discord shares none with a bot, so nothing here is read from the Discord app and nothing here is
// written back to it. `#inbox-note` carries the server's own sentence, quoted rather than rewritten
// here, so the page and the server cannot come to describe the posture differently.
//
// WHAT IS NOT HERE, and why. The issue asks for a SWIPE to dismiss and a PRESS-AND-HOLD to declare
// bankruptcy. Those are a gesture layer — horizontal intent disambiguated from vertical, on the one
// list this page scrolls — and they are a change of their own; this lands the acts themselves, each
// reachable by a control a keyboard can also get to, so the gestures become a second way in rather
// than the only way. It also asks for a message to leave the list when it is REPLIED to; that is
// derived state and needs a reply reference on the server's Message, which is a wire-format change.
// Both are follow-ups, and until the second one lands "dealt with" here is always DECLARED — which
// is why nothing in this file has to decide what happens when derived and declared disagree.

// Is the reader looking at the to-do list rather than the whole channel? Kept across a reload with
// the rest of what was on screen, in `UI_STATE_KEY`. `#189 restore-ui-state`. Since `#221 read-modes`
// this is Hide read, the third of the read modes: the mode being "hide", set only with it. It keeps a
// question of its own (`readModes.hiding()`) because every read path already asks it the one question
// it answers, which is whether read messages are left out of the list.
//
// The mode itself, its names (READ_MODES, READ_MODE_LABELS, READ_MODE_VERBS, READ_MODE_SAID,
// READ_MODE_TITLES), its kept copy (READ_MODE_KEY), what an old record means (`savedReadMode`) and the
// cycle (`nextReadMode`) are web/src/present/rows' (`createReadMode`, `#624 voice-page-modularity`):
// `readModes.mode()` is the mode, and `readModes.hiding()` this question.

/**
 * Put one row at the end of the channel list, and keep everything that DESCRIBES the list in step.
 *
 * TWO paths add a row without a read: a message arriving on the live stream, and a reply this page
 * has just posted. Both are the same fact — the list on screen is now longer than the last `/todo`
 * answer said — and both are wrong in the same way if only the row moves. In to-do mode a message
 * that has just arrived has by definition not been dealt with, so it is one more thing to do, and
 * the head of the list, the backlog count and the bulk control have to say so in the same moment
 * the row appears. Otherwise the view contradicts itself: "nothing left to deal with" written
 * directly above a message, a bulk control hidden while there is something to clear, or a count
 * that says two over a list of three — which `clearBacklog` would then clear all three of.
 *
 * Outside the mode this is an ordinary append, because nothing on screen is claiming a count.
 *
 * `live` says which of the two it is: only the stream's copy can leave a message's thread unknown.
 */

function appendChannelRow(arriving: Message, live = false, folded = false): void {
  // The store's copy, which may keep a thread record this one arrived without. `folded`: the caller
  // folded it already, for every column at once (`appendToColumns`).
  const message = providerFacts.threading() && !folded ? foldLiveMessage(arriving, live) : arriving;
  // Saved once, from the left column: the store is the channel's, not a column's. `#229`.
  const save = () => {
    if (!columnWork.side()) saveChannelScope();
  };
  if ([...el("discord-log").children].some((row) => idsOf(row).includes(String(message.id)))) {
    if (providerFacts.threading()) save();
    return;
  }
  if (providerFacts.threading()) {
    const id = threadOf(message);
    if (selection().view === "threads" ||
        (selection().view === "thread" && id !== selection().threadId) ||
        (selection().view === "main" && !inView(message, "main", null))) {
      // Not this view's row, but the store has it, and so will the next start. On Main a reply is
      // one more to count on its root's chip, and may bring the root back under Hide read.
      syncUnreadReplies();
      save();
      return;
    }
    if (!timelineRows().messages.some((held) => String(held.id) === String(message.id))) timelineRows().messages.push(message);
  }
  // `#196 auto-read-noise`. A placeholder arriving while Hide read is on is already read: it is
  // held with the rest of the channel, and the list — and its count — are not told about it.
  if (hidingRead() && isNoise(message)) {
    save();
    noiseSettings.renderCount();
    return;
  }
  // `#386 hide-empty-messages`. An empty message is held with the rest and drawn nowhere.
  if (hiddenEmpty(message)) {
    save();
    return;
  }
  const list = el("discord-log");
  const append = () => {
    // A LIVE ARRIVAL CAN JOIN THE ROW ABOVE IT, and it has to be given the chance: the second half
    // of a split post usually arrives over the stream a fraction of a second after the first, so
    // appending it unconditionally would leave exactly the pair this feature exists for as two rows
    // until the next poll happened to redraw them together.
    const rows = [...list.children];
    const last = rows[rows.length - 1];
    const held = rowMessages(last);
    if (held.length > 0 && joinsGroup(held[held.length - 1], message, combineSwitch.on())) {
      // The row is REBUILT rather than edited: one constructor for a row, wherever it came from.
      list.replaceChildren(...rows.slice(0, -1), discordNode([...held, message]));
    } else {
      list.append(discordNode([message]));
    }
    // `#84 reply-aware-dismissal`. Re-derive the row states with the new row in place. It is here, in the one
    // appender, rather than at each of its callers: an arriving message can be the ANSWER to
    // something already on screen, so the row that changes is not necessarily the one just added.
    renderChannelRows();
  };
  // `#215 reply-coalesce`. With replies gathered, a new reply to the parent joins the stack, which
  // is ABOVE the foot of the list: the reader below it is held where they are reading.
  if (gatheredParent() !== null) preservingScroll(append);
  else append();
  // A live arrival and an acknowledged send are both what a reload should show next.
  save();
}

/**
 * A message that arrived, or was sent from here: into the store once, and onto each column showing
 * messages it belongs in. `#229 desktop-two-column`. Answers the copy the store kept. `replayed`, a
 * copy the stream replayed (`foldLiveMessage`).
 */

function appendToColumns(arriving: Message, live = false, replayed = false): Message {
  const message = providerFacts.threading() ? foldLiveMessage(arriving, live, replayed) : arriving;
  inColumn(columns.main, () => appendChannelRow(message, live, true));
  if (sideShowing()) inColumn(columns.side, () => appendChannelRow(message, live, true));
  return message;
}

/**
 * Archive or unarchive ONE row, whichever it is asking for.
 *
 * Both the swipe and the row's button come through here, so the two cannot drift into two notions
 * of what the gesture means.
 */

function readRunPlan(rows: unknown, key: string, joins: unknown, idsFor: unknown = idsOf, forcedIds: Set<string> = new Set<string>()): Map<Element, { role: string; count: number }> {
  return readRuns.plan(rows, key, joins, idsFor, forcedIds);
}

/**
 * The messages the reader folded away one by one (Shift+O, `#480 fold-one-message-key`), per view
 * as `readRuns` keeps opened runs: ids, so a redraw draws them folded again, in any read mode — a
 * thread being worked through keeps the parts closed up until their line is opened, which takes
 * them out of this set (`openReadRun`). In memory only, as an opened run is: a reload lays the
 * thread out afresh.
 */

function readRunRows(row: Element): Element[] {
  const rows = [...(row.parentNode || el("discord-log")).children];
  let head = rows.indexOf(row as never);
  while (head > 0 && (rows[head] as Element).getAttribute("data-read-run") === "member") head -= 1;
  const run = [rows[head] as Element];
  for (let at = head + 1; at < rows.length && (rows[at] as Element).getAttribute("data-read-run") === "member"; at += 1) {
    run.push(rows[at] as Element);
  }
  return run;
}

function applyReadRuns(): void {
  const key = viewKey();
  const forced = foldedOneIds(key);
  // `#480 fold-one-message-key`: messages folded away one by one collapse in any read mode.
  const collapsing = (readModes.mode() === "fold" || forced.size > 0) && !readRunsSuspended();
  if (!collapsing && listState().runRows === 0) return;
  const list = el("discord-log");
  const rows = [...list.children];
  const rootId = collapsing ? openThreadRootId() : null;
  // Nor a row of replies the reader gathered under their message (`#215 reply-coalesce`): they were
  // gathered to be read together, and a line in the stack would break the bridge. Nor a root on Main
  // whose thread holds an unread reply (`#222 unread-replies`): Hide read keeps it, so Fold read
  // does, by the attribute the pass has just set from the same rule.
  const placed = collapsing ? readRunPlan(rows, key, (row: Element) => !isOpenThreadRoot(row, rootId) &&
    !row.hasAttribute("data-coalesce") && !row.hasAttribute("data-unread-replies") &&
    ((readModes.mode() === "fold" && rowIsRead(row)) || idsOf(row).some((id) => forced.has(id))),
    idsOf, forced) : new Map();
  listState().runRows = placed.size;
  const want = (row: Element) => placed.get(row) || { role: null, count: 0 };
  const changed = rows.some((row: Element) => {
    const { role, count } = want(row);
    return (row.getAttribute("data-read-run") || null) !== role ||
      (role === "head" && readRunWords(row) !== readRunText(count, runDaysOf(row)));
  });
  if (!changed) return;
  const apply = () => {
    for (const row of rows) {
      const { role, count } = want(row);
      placeInReadRun(row, role, count);
    }
  };
  // Only where the list is the one on screen: anywhere else there is no reader in it to hold.
  if (deps.screensScreen() === "main" && screens.view() === "discord") preservingScroll(apply);
  else apply();
  // Which row is directly above a reply has changed with what is drawn, and its arrow with it.
  renderReplyLinks();
}

/**
 * The rows of the collapsed run `row` belongs to, its line's row first: in its own list, the channel's
 * or the cards' with threads unpacked under them, where a card ends a block's run (`#407
 * unpack-row-parity`).
 */

function openReadRun(row: Element): void {
  if (!row || !row.hasAttribute("data-read-run")) return;
  const thread = row.getAttribute("data-unpack-thread");
  // `#432 read-mode-unify`. A run of read threads (and Main's read messages) over the cards: kept open by
  // what its cards and rows stand for, and the cards drawn again.
  if (thread === null && row.parentNode === leftEl("side-cards")) {
    const runRows = cardRunRows(row);
    deps.collapseRevealed(runRows);
    readRuns.open(cardRunKey(), runRows.flatMap(cardRunIds));
    cardsDrawn.forget();
    renderSideColumn();
    keepSelection();
    return;
  }
  const key = thread === null ? viewKey() : viewKey("thread", thread);
  const runRows = readRunRows(row);
  const ids = runRows.flatMap(idsOf);
  deps.collapseRevealed(runRows);
  readRuns.open(key, ids);
  // Opening a run is also un-folding what was folded into it one by one (`#480 fold-one-message-key`).
  const folded = foldedOnes.get(key);
  if (folded) for (const id of ids) folded.delete(id);
  if (folded && folded.size === 0) foldedOnes.delete(key);
  if (thread !== null) {
    cardsDrawn.forget();
    renderSideColumn();
    // The block is new rows: the selection, its Tab stop and its focus go to the run's first row's.
    keepSelection();
    return;
  }
  applyReadRuns();
  renderScrollTools();
  requestVisibleSummaries();
}

/**
 * Peel one edge of the folded run `row` heads: reveal its oldest (`first`) or newest (`last`)
 * `READ_RUN_PEEL_SIZE` messages while the rest stays folded behind its line, whose count drops
 * by what was revealed. Every revealed row starts collapsed, as a whole-line open's do.
 */
function peelReadRun(row: Element, edge: "first" | "last"): void {
  if (!row || !row.hasAttribute("data-read-run")) return;
  const thread = row.getAttribute("data-unpack-thread");
  if (thread === null && row.parentNode === leftEl("side-cards")) {
    const runRows = cardRunRows(row);
    const take = edge === "first" ? runRows.slice(0, READ_RUN_PEEL_SIZE) : runRows.slice(-READ_RUN_PEEL_SIZE);
    if (take.length === 0) return;
    deps.collapseRevealed(take);
    readRuns.peel(cardRunKey(), take.flatMap(cardRunIds));
    cardsDrawn.forget();
    renderSideColumn();
    keepSelection();
    return;
  }
  const key = thread === null ? viewKey() : viewKey("thread", thread);
  const runRows = readRunRows(row);
  const ordered = edge === "first" ? runRows : [...runRows].reverse();
  const take: Element[] = [];
  let revealed = 0;
  for (const one of ordered) {
    take.push(one);
    revealed += idsOf(one).length;
    if (revealed >= READ_RUN_PEEL_SIZE) break;
  }
  if (take.length === 0) return;
  const ids = take.flatMap(idsOf);
  deps.collapseRevealed(take);
  readRuns.peel(key, ids);
  const folded = foldedOnes.get(key);
  if (folded) for (const id of ids) folded.delete(id);
  if (folded && folded.size === 0) foldedOnes.delete(key);
  if (thread !== null) {
    cardsDrawn.forget();
    renderSideColumn();
    keepSelection();
    return;
  }
  applyReadRuns();
  renderScrollTools();
  requestVisibleSummaries();
}

/**
 * The row now standing in the cards for `row`, a card or a Main row a redraw of the cards took away (an
 * opened run of read threads, `#432 read-mode-unify`): the new one of the same kind and id, or null.
 *
 * @param {Element} row
 * @returns {HTMLElement | null}
 */

function renderReadModeControl() {
  const button = el("todo-filter");
  // `#1399 toggle-labels-active-style`: the label is fixed ("Fold read"); on/off is aria-pressed and the shared active style.
  const label = READ_MODE_LABELS.fold;
  // Two words in two spans, so a desk can stack them; made here when the markup's are absent.
  const words = el("todo-filter-label");
  let verb = childByClass(words, "read-mode-verb");
  if (!verb) {
    verb = document.createElement("span");
    verb.className = "read-mode-verb";
    const noun = document.createElement("span");
    noun.className = "read-mode-noun";
    noun.textContent = " read";
    words.replaceChildren(verb, noun);
  }
  if (verb.textContent !== READ_MODE_VERBS.fold) verb.textContent = READ_MODE_VERBS.fold as string;
  // The mode's own icon, and only it, by `hidden`: an icon is something a test of the dock can ask
  // for by that attribute, as it asks of every other control's.
  for (const icon of [...button.children]) {
    if (!String(icon.getAttribute("class") || "").split(/\s+/).includes("read-mode-icon")) continue;
    const mine = icon.getAttribute("data-mode") === readModes.mode();
    if (icon.hasAttribute("hidden") === mine) icon.toggleAttribute("hidden", !mine);
  }
  // Written only when they change: this runs with every redraw of the controls.
  const said = {
    "data-read-mode": readModes.mode(),
    "aria-pressed": readModes.mode() === "fold" ? "true" : "false",
    "aria-label": label,
    title: READ_MODE_TITLES[readModes.mode()],
  };
  for (const [name, value] of Object.entries(said)) {
    if (button.getAttribute(name) !== value) button.setAttribute(name, value as string);
  }
}

/** Is `row` read, by the one rule both read modes share? Every message it stands for has to be. */

function cycleReadMode() {
  setReadMode(nextReadMode(readModes.mode()));
}

/**
 * Which LOADED messages the reader has archived, as the server reported them.
 *
 * Held here rather than read off the rows because it is the server's answer, not a fact about the
 * DOM: a row is greyed BECAUSE it is in this set, and the set survives the list being rebuilt by a
 * poll. The ordinary channel view dims these; the To do filter never shows them at all, so in that
 * mode this stays empty and nothing consults it.
 *
 * Only ever the ids in the window on screen — see `ops::dismissed_within`. The store holds every
 * dismissal the channel ever had, and sending the lot would grow without bound.
 *
 * A column's own record (`#624 voice-page-modularity`, plan step 15, as `freshOlderPages`): `archived`,
 * the set just described, a fresh empty one for each column.
 *
 * @returns {{archived: Set<string>}}
 */

function rowIsRead(row: Element): boolean {
  const held = rowMessages(row);
  return held.length > 0 && held.every((message) => !stillToDo(message));
}

/** Is `row` the root of the thread on screen? By identity, never by position: older replies may not be loaded. */

function readRunsSuspended() {
  return searchField.open() && (searchField.links() || authorFilters.chosen() !== "everyone" || searchTyped(searchField.query()));
}

/**
 * Whether Hide read is leaving read messages out of the list now. `#262 search-load-older`: not while a
 * filter chooses rows on a provider with threads, where Hide read steps aside as Fold read does
 * (`readRunsSuspended`) — an old match is read by definition, and a search that walks back under Hide
 * read would draw almost nothing of what it read and say it had searched it. The read mode the reader
 * chose is untouched, and closing the search brings it back. Without threads Hide read is a read of
 * its own (`loadTodo`), and stays in force.
 */


  /**
   * Settle where a reply to an unplaced message goes, before anything is posted, and answer the
   * thread it belongs in or null for the main channel. `#185 reply-new-thread`.
   *
   * From the store when a read has placed the message since Reply was opened; otherwise from one
   * read of All, which carries every message with the thread record its live copy lacked. A message
   * that read does not carry cannot be placed and the reply is not posted at all: guessing the main
   * channel is exactly what turned the answer of 2026-10-04 into a thread of its own.
   *
   * NOR ONE REMOVED UPSTREAM (`noteRemoved`), before the read or while it was out: the reply is not
   * posted, and no read is made for it. `#315 removed-reply-target`. Its bare copy, held until the read
   * behind the removal lands, is not a message placed on the main channel.
   */
  async function placeReplyTarget(entry: { replyTo?: unknown; channel?: string }): Promise<string | null> {
    const id = String(entry.replyTo);
    const current = (): boolean => String(deps.selectedChannelId()) === String(entry.channel);
    const refuseRemoved = (): void => {
      if (!current() || !deps.removedUpstream(id)) return;
      const error = new Error("the message you answered was removed from the channel") as Error & { removed?: boolean };
      error.removed = true;
      throw error;
    };
    refuseRemoved();
    const held = current() ? deps.heldMessage(id) : null;
    if (held && !deps.channelStore.current().unplaced.has(id)) return answeredThread(held);
    const payload = await timelineReader.readTimeline(
      `/api/v1/channels/${encodeURIComponent(String(entry.channel))}/timeline?view=flat&limit=${DISCORD_PAGE_LIMIT}` +
        "&reason=reply-place"
    );
    const messages = payload.messages || [];
    if (current()) {
      const number = deps.sentNumberOf(payload);
      for (const message of messages) {
        if (deps.channelStore.current().unplaced.has(String(message.id))) foldLiveMessage(message, false, false, number);
      }
      deps.saveChannelScope();
    }
    refuseRemoved();
    const found = messages.find((message) => String(message.id) === id);
    if (!found) throw new Error("it is not among the channel's newest messages");
    return answeredThread(found);
  }

  return {
    placeReplyTarget,
    loadCanon, drawProjection, walkCursorOwed, heldViewRows, drawHeldRows,
    queueReadAllForCounts, readAllForCounts, rowsAsDrawn, applyTimelinePage,
    renderCachedTimeline, mutationAwaitsRead, loadTimeline, foldLateTimelinePage,
    loadOlderTimeline, foldLiveMessage, inView, projectView, holdingMessage, foldTimelinePage, foldTimelineDelta,
    loadDiscord, loadOlder, maybeLoadOlder, takeDeferredOlder, appendChannelRow, appendToColumns, applyReadRuns, readRunPlan, readRunRows, cycleReadMode, readRunsSuspended, rowIsRead, openReadRun, peelReadRun, renderReadModeControl, applyNewestPage, channelReadPosition, settleAfterRead, landedReadPosition,
    dispose() { disposed = true; },
  };
}

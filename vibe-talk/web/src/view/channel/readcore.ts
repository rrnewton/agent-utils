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
  threadOf,
  timeOf,
} from "../../core/channels/index.ts";
import type { ChannelCanon, ChannelStore, CopyOrder, PageFold, Removals, ReplyTally, SavedTimeline } from "../../core/channels/index.ts";
/** The reader (core/sync `createTimelineReader`), as this view uses it. */
export interface TimelineReaderLike {
  readTimeline(path: string, options?: { signal?: CancelSignal }): Promise<TimelineResponse>;
  timelinePath(before?: string | null, after?: string | null, reason?: string | null): string;
  readReasonOf(options: { readAs?: string; reason?: string | readonly (string | undefined)[] } | null | undefined): string | null;
  olderReadReason(): string;
  forwardCursor(reason: string | readonly (string | undefined)[] | undefined): string | null;
  joinedDeltas(pages: readonly TimelineResponse[]): TimelineResponse;
  landedAnswer(pages: readonly TimelineResponse[]): { messages: Message[]; threads: ThreadSummary[]; delta: { complete: boolean } | null } | null;
  staleAsOf(payload: TimelineResponse): number | null;
}
import { allNoticeWords } from "../../present/rows/index.ts";
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
  listState(): { newestId: string | null; backlog: number };
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
    abandon<T>(answer: Promise<T>, read: TimelineAbandonedRead, land: (payload: T) => void, settled?: ((landed: boolean, unused: T | null) => void) | null): void;
  };
  channelReads: {
    claim(context: string | null): boolean;
    cursorOwed(): boolean;
    oweCursor(owed: boolean): void;
    queued(column: TimelineColumn): boolean;
    payCursor(): void;
  };
  queueDiscordLoad(options?: TimelineReadOptions): void;
  loadDiscord(options?: TimelineReadOptions): Promise<void>;
  finishDiscordLoad(column?: TimelineColumn): Promise<void>;
  replayLedger: { start(): unknown; mutationAwaits(): boolean };
  columnReadsSent: Map<TimelineColumn, { context: string; read: unknown; full: boolean }>;
  landedReadPosition(area: HTMLElement, keepPosition: boolean, sentAtNewest: boolean): { wasAtNewest: boolean; keepPosition: boolean; [key: string]: unknown };
  landAbandonedTimelineRead(payload: TimelineResponse, read: TimelineFoldRead): void;
  queuePlaceUnplaced(): void;
  setStatus(text: string): void;
  prepareSpeech(): void;
  guardQuietly(fn: () => void): () => void;
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
  channelReadPosition(area: HTMLElement): { wasAtNewest: boolean; anchorId: string | null; anchorTop: number; around: unknown };
  renderChannelFreshness(): void;
  preservingScroll(mutate: () => void): void;
  columnWork: { active(): TimelineColumn };
  settleAfterRead(messages: readonly { id: unknown }[], how: unknown): void;
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
  readRunsSuspended(): boolean;
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
  unplacedNext(): number;
  threadPresence: { note(hasThreads: boolean | undefined, inThread: boolean): void };
  openThreadName(): string;
  captureScroll(): { pinned: boolean; anchor: Element | null; top: number };
  restoreScroll(mark: { pinned: boolean; anchor: Element | null; top: number }): void;
  heldRowAfter(position: { anchorId?: string | null; anchorTop?: number; around?: unknown }): { row: Element; top: number } | null;
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
  const loadDiscord = deps.loadDiscord;
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
  const landedReadPosition = deps.landedReadPosition;
  const landAbandonedTimelineRead = deps.landAbandonedTimelineRead;
  const queuePlaceUnplaced = deps.queuePlaceUnplaced;
  const setStatus = deps.setStatus;
  const guardQuietly = deps.guardQuietly;
  const readAloudOwner = () => ({ prepareSpeech: deps.prepareSpeech });
  const renderChannelFreshness = deps.renderChannelFreshness;
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
  const heldRowAfter = deps.heldRowAfter;
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
  const channelReadPosition = deps.channelReadPosition;
  const settleAfterRead = deps.settleAfterRead;
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
  const readRunsSuspended = deps.readRunsSuspended;
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
  const olderPager = { loadedIsWhole: (moreAbove: boolean | undefined) => (moreAbove === undefined ? undefined : !moreAbove) };
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
  if (held && screens.view() === "discord") area.scrollTop += held.row.getBoundingClientRect().top - held.top;
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
    el("discord-log").replaceChildren(...glom(shown).map(discordNode));
    el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
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
    el("discord-log").replaceChildren(...glom(shown).map(discordNode));
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
 * `#617 cold-load-first-paint`. After a page the server answered from its own copy, the next reads
 * come soon, one after the other, rather than at the next poll (`STAND_IN_RETRY_MS`): how many have
 * been spent, and whether the next poll should be one, are web/src/core/sync's (`createStandInRetries`,
 * `#624 voice-page-modularity`).
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

  return {
    loadCanon, drawProjection, walkCursorOwed, heldViewRows, drawHeldRows,
    queueReadAllForCounts, readAllForCounts, rowsAsDrawn, applyTimelinePage,
    renderCachedTimeline, mutationAwaitsRead, loadTimeline, foldLateTimelinePage,
    loadOlderTimeline, foldLiveMessage, inView, projectView, holdingMessage, foldTimelinePage, foldTimelineDelta,
    dispose() { disposed = true; },
  };
}

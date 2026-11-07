// Walking back for older rows, and for a search (`#65 scrollback-paging`, `#262
// search-load-older`, `#578 search-status-clarity`; `#624 voice-page-modularity`, moved from the
// legacy script into view/channel; mu-readcore): the step-back control and seam, the search's
// bounded walk over older pages, the server search and the cards' replies walk rendering. The
// walk record itself lives on the column (`column.searchWalk`), the same object the page's own
// readers see; this owner only drives it. Distinct from view/voice/readcontrols.ts
// (`mountReadControls`, the read-aloud controls) — names here are the OlderWalks surface.

import type { Message, SearchScope } from "../../contract/index.ts";
import { DISCORD_PAGE_LIMIT, exactReplyCount, threadOf } from "../../core/channels/index.ts";
import type { ChannelStore, ReplyTally } from "../../core/channels/index.ts";
import { failureWords, searchMomentWords, searchWalkProgress, searchWalkWords } from "../../present/notices/index.ts";
import { setTextIfChanged } from "../dom/index.ts";
import { isMessageRow, rowSentMs, rowShown } from "./rows.ts";
import type { TimelineColumn, TimelineColumnSteps, TimelineOlderPages, TimelineSelection } from "./readcore.ts";
import type { CancelSignal } from "../../ports/index.ts";

/** One walk back for a search, on its column (`column.searchWalk`). */
export interface SearchWalk {
  key: string;
  context: string;
  ticket: number;
  held: Set<string>;
  pages: number;
  read: number;
  cursorRead: boolean;
  found: number;
  arrived: number;
  started: number;
  controller: AbortController | null;
  stopping: boolean;
  ended: boolean;
  anchored: boolean;
  chipTop: number;
  promise: Promise<void> | null;
}

/** A column with its search walk on it. */
export type OlderWalkColumn = TimelineColumn & { searchWalk: SearchWalk | null; searchNote: { key: string; text: string } | null; searched?: { loaded: number } | null };

/** How far back the rows on screen reach, as the search line says it. */
export interface OlderHistoryReach {
  total: number; proven: boolean; saved: boolean; dense: number; scattered: number;
  from: number | null; start: boolean; whole: boolean; topLevel: boolean; threads: number; full: number;
}

/** What walking back asks of the page: the legacy names, each asked as it is needed. */
export interface OlderWalksDependencies {
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  selection(): TimelineSelection;
  selectedChannelId(): string;
  channelContextKey(): string;
  viewKey(view?: string, thread?: string | null): string;
  olderPages(): TimelineOlderPages;
  channelStore: ChannelStore;
  providerFacts: { threading(): boolean };
  readModes: { hiding(): boolean; mode(): string };
  searchField: { open(): boolean; query(): string; links(): boolean };
  searchIdentity(query: string): string;
  searchTyped(query: string): boolean;
  filterKey(): string;
  linkKindsHidden(): Iterable<string>;
  authorChoosing(): string;
  pinsOnly(): boolean;
  readRunsSuspended(): boolean;
  screensView(): string;
  columnWork: { active(): OlderWalkColumn; side(): boolean };
  columns: { main: OlderWalkColumn; side: OlderWalkColumn & { mode: string } };
  sideShowing(): boolean;
  inColumn<T>(column: TimelineColumn, fn: () => T): T;
  columnSteps(): TimelineColumnSteps;
  filterChanges: { walking(): boolean };
  replyJumpCurrent(): number;
  endFilterWalk(): void;
  readerActed(): void;
  guardQuietly(fn: (...args: never[]) => unknown): (...args: never[]) => unknown;
  renderJumpNewest(): void;
  applySearch(): void;
  preservingScroll(mutate: () => void): void;
  setStatus(text: string): void;
  walkCursorOwed(): boolean;
  loadOlder(signal?: CancelSignal): Promise<void> | null;
  pullFingerDown(): boolean;
  idsOf(row: Element | null | undefined): string[];
  rowMessages(row: Element | null | undefined): Message[];
  rowDay(row: Element): number;
  plainReach: { saved(): boolean; gapFrom(): number | null };
  replyTally(): ReplyTally;
  threadPresenceAny(): boolean;
  providerOfChannel(id: string): { name?: unknown } | null;
  channelDirectoryName(): string;
  serverSearchOwner: {
    state(): { key: string; ticket: number; loading: boolean; failed: string; hits: Message[]; scopes: SearchScope[]; examined: number; resume: number | null };
    allChannels(): boolean;
    schedule(key: string, open: boolean): void;
    chooseAllChannels(on: unknown): void;
    run(more: boolean): Promise<void>;
    more(): Promise<void>;
    forget(): void;
  };
  serverFactsTokenScope(): string;
  serverCopyOpen(): boolean;
  historyReadToStart(): boolean;
  keepingCardPlace(change: () => void): void;
  api(path: string, options?: unknown): Promise<unknown>;
  channelName(channel: unknown): string;
  channelDirectoryChannels(): { id: unknown }[];
  jumpToMessageLink(ref: unknown): void;
  messageBody(message: Message): Node;
  heldMessagesById(): Map<string, Message>;
  markSearchHits(row: Element, terms: unknown): void;
  searchParsed(query: string): { terms: string[] };
  gathering: { stop(): void; start(channel: string): Promise<void>; state(): { channel: string; failed: string; state: { reached_ms?: number; oldest_ms?: number; eta_s?: number; complete?: boolean; held_reason?: string } | null } };
  searchFurtherRunning(): boolean;
  repliesWalks: {
    current(): { stopping: boolean; arrived: number; pages: number } | null;
    search(): void;
    end(): void;
    holds(walk: unknown): boolean;
    noteFor(key: () => string): string;
  };
  sideCardsShown(): boolean;
  layoutColumnsShown(): boolean;
  sidePaneValue: unknown;
  screensScreen(): string;
  lastPaneCurrent(): unknown;
  channelName(channel: string): string;
}

/** The older walks' commands: the legacy names, unchanged. */
export interface OlderWalks {
  renderOlderControl(): void;
  olderControlPressed(): void;
  searchOlder(): void;
  searchOlderFromField(): boolean;
  searchReplies(): void;
  searchOffered(): boolean;
  searchKey(): string;
  endSearchWalks(): void;
  endSearchWalk(column: OlderWalkColumn): Promise<void> | null;
  stopSearchWalk(column: TimelineColumn): void;
  renderSearchOlderNote(): void;
  searchFurtherPressed(): Promise<void>;
  chooseSearchAllChannels(on: boolean): void;
  renderRepliesSearch(): void;
  renderServerSearch(): void;
  serviceWords(): string;
  repliesFloor(): number | null;
  repliesSearchOffered(): boolean;
  allCover(): { live: boolean; more: boolean; cursor: string | null } | undefined | null;
  allLoaded(): boolean;
  repliesKey(): string;
  endRepliesWalk(): void;
  historyReach(): OlderHistoryReach;
  scheduleServerSearch(): void;
  channelViewWords(): string;
  oldestLoadedMs(): number | null;
  renderServerSearch(): void;
  dispose(): void;
}

/** How many older pages one walk for a search reads at most. */
export const SEARCH_OLDER_PAGES = 5;
/** How long one walk for a search may run, in milliseconds. */
export const SEARCH_OLDER_MS = 15000;
/** How many hits end a walk for a search early. */
export const SEARCH_OLDER_HITS = 3;
/** How soon a second tap on the step-back control stops the walk, in milliseconds. */
export const SEARCH_OLDER_TAP_MS = 500;

/** The older walks over `deps`. */
/** How long a walk waits with a finger down (view/channel pull). */
const PULL_WAIT_MS = 50;

export function mountOlderWalks(deps: OlderWalksDependencies): OlderWalks {
  const el = deps.el;
  const leftEl = deps.leftEl;
  const selection = deps.selection;
  const selectedChannelId = deps.selectedChannelId;
  const channelContextKey = deps.channelContextKey;
  const viewKey = deps.viewKey;
  const olderPages = deps.olderPages;
  const channelStore = deps.channelStore;
  const providerFacts = deps.providerFacts;
  const readModes = deps.readModes;
  const searchField = deps.searchField;
  const searchIdentity = deps.searchIdentity;
  const searchTyped = deps.searchTyped;
  const filterKey = deps.filterKey;
  const authorChoosing = deps.authorChoosing;
  const columnWork = deps.columnWork;
  const columns = deps.columns;
  const sideShowing = deps.sideShowing;
  const inColumn = deps.inColumn;
  const columnSteps = deps.columnSteps;
  const filterChanges = deps.filterChanges;
  const guardQuietly = deps.guardQuietly;
  const renderJumpNewest = deps.renderJumpNewest;
  const applySearch = deps.applySearch;
  const preservingScroll = deps.preservingScroll;
  const setStatus = deps.setStatus;
  const walkCursorOwed = deps.walkCursorOwed;
  const loadOlder = deps.loadOlder;
  const idsOf = deps.idsOf;
  const rowMessages = deps.rowMessages;
  const plainReach = deps.plainReach;
  const replyTally = deps.replyTally;
  const linkKinds = { hidden: () => deps.linkKindsHidden() };
  const readRunsSuspended = deps.readRunsSuspended;
  const screens = { view: () => deps.screensView(), screen: () => deps.screensScreen() };
  const rowDay = deps.rowDay;
  const threadPresence = { any: () => deps.threadPresenceAny() };
  const providerOfChannel = deps.providerOfChannel;
  const channelDirectory = { providerName: () => deps.channelDirectoryName(), channels: () => deps.channelDirectoryChannels() };
  const endFilterWalk = deps.endFilterWalk;
  const readerActed = deps.readerActed;
  const pullToRefresh = { fingerDown: () => deps.pullFingerDown() };
  const sideCardsShown = deps.sideCardsShown;
  const layoutShown = { columns: () => deps.layoutColumnsShown() };
  const lastPane = { current: () => deps.lastPaneCurrent() };
  const sidePane = deps.sidePaneValue;
  const repliesWalks = deps.repliesWalks;
  const serverSearchOwner = deps.serverSearchOwner;
  const serverSearchNow = () => deps.serverSearchOwner.state();
  const gathering = deps.gathering;
  const serverFacts = { tokenScope: () => deps.serverFactsTokenScope() };
  const stopSearchFurther = () => deps.gathering.stop();
  const searchFurtherRunning = deps.searchFurtherRunning;
  const serverCopyOpen = deps.serverCopyOpen;
  const historyReadToStart = deps.historyReadToStart;
  const keepingCardPlace = deps.keepingCardPlace;
  const api = deps.api;
  const channelName = deps.channelName;
  const jumpToMessageLink = deps.jumpToMessageLink;
  const messageBody = deps.messageBody;
  const heldMessagesById = deps.heldMessagesById;
  const markSearchHits = deps.markSearchHits;
  const searchParsed = deps.searchParsed;
  const pinsView = () => ({ only: deps.pinsOnly() });
  const replyJumpTicket = { current: () => deps.replyJumpCurrent() };
  const SERVER_SEARCH_REASONS: Record<string, string> = {
    no_cache: "it keeps no copy",
    serve_off: "answering from its copy is off",
    not_kept: "it keeps no copy of this channel",
    unverified: "its account is still being checked",
    suspended: "the chat service refused its credential",
    in_doubt: "access to this channel is in doubt",
    read_scope: "this device holds the read-only token",
  };
  let disposed = false;
  void disposed;

const searchKey = () => JSON.stringify([filterKey(), searchField.open() ? searchIdentity(searchField.query()) : null, searchField.open() && searchField.links(),
  [...linkKinds.hidden()], authorChoosing(), readModes.mode()]);

/**
 * Whether the step back of the column being worked on offers to search further back: a filter choosing
 * rows over a list of the channel's messages with older history, and not over the Pinned list. Not on a
 * chat service without threads under Hide read, whose list is a read of its own (`loadTodo`), where the
 * empty sentence says how out instead.
 */

function searchOffered() {
  return readRunsSuspended() && screens.view() === "discord" && selection().view !== "threads" && !pinsView().only &&
    olderPages().moreAbove === true && !(readModes.hiding() && !providerFacts.threading()) && (!columnWork.side() || sideShowing());
}

/** The view a column shows, as a sentence names it. */

function channelViewWords() {
  if (!providerFacts.threading()) return "this channel";
  return selection().view === "thread" ? "this thread" : selection().view === "flat" ? "All" : "Main";
}

/** When the oldest message loaded in the column being worked on was sent, in milliseconds, or null. */

function oldestLoadedMs() {
  for (const row of el("discord-log").children) {
    if (!isMessageRow(row)) continue;
    if (rowDay(row) !== null) return rowSentMs(row);
  }
  return null;
}

// --- how far back, with no gaps -------------------------------------------------------------------
//
// `#578 search-status-clarity`. The owner, 2026-10-27, of "Nothing in the 82 messages loaded so far,
// back to yesterday": "It doesn't explain what are these 82 messages loaded. Is that a dense
// chronological history from time T0-present? Or just whatever random sparse set we have loaded? I
// think a dense suffix of messages is the easiest to reason about. You could even say that we have 'X
// messages back to DATETIME with no gaps, plus Y scattered messages before that.'" So the channel's
// summary, the sentence over a search that matched nothing, and what a walk further back found all say
// it in those terms, from one account of the list (`historyReach`), to the minute: a day alone did not
// move when a press brought fifty more messages of the same day, and the owner saw "THE SAME button".
//
// A STRETCH WITH NO GAPS IS ONE THE READS PROVED. Where the chat service has threads, the view's cover
// in the channel's store says how far back its pages reach (`coverView`): each step back is read from
// the cursor the one before it handed over, and a newest read that reaches down into what an earlier
// read carried joins it. One that does not — the page was away while more than a page of messages
// arrived — leaves a gap the rows do not show, and the cover keeps where it is (`cover.dense`): the
// messages above it are the stretch, and those below are SCATTERED. Without threads, the plain page's
// newest read does the same (`PlainReach.gapFrom`). Rows saved on this device that no read has checked claim
// no stretch at all, nor does a list no read has said anything of.
//
// ALL ON A CHAT SERVICE WITH THREADS IS TOP-LEVEL MESSAGES FIRST. Its pages hold every top-level message
// and, on Slack, the replies of at most ten of their threads; and a reply posted since in a thread
// started further back is in no page of the stretch. So All claims its top-level messages, and says of
// the threads started in the stretch how many it holds every reply of (`exactReplyCount`): "every
// top-level message back to Oct 26, 3:14 PM (no gaps), and every reply in 20 of their 31 threads". It
// is whole only when it reaches the start and holds every reply of every thread: the "112 messages …
// all of it is loaded" the owner was shown on 2026-10-25 was the start of the top-level messages alone
// (`#521 server-side-search`, its Phase 0).
//
// WHERE: the search runs in this browser, over what is loaded, and a walk further back fetches older
// messages from the chat service through the server. The server's own search of its message cache,
// and the chat services' search, are designed in `#521 server-side-search` and not built, so nothing
// here says either searched.

/**
 * Without threads: the instant below which the rows loaded are scattered, and whether the rows on screen
 * were drawn from this device with no read landed since (web/src/core/channels `createPlainReach`, `#624
 * voice-page-modularity`).
 */

function historyReach(): { total: number; proven: boolean; saved: boolean; dense: number; scattered: number; from: number | null; start: boolean; whole: boolean; topLevel: boolean; threads: number; full: number } {
  const rows = [...el("discord-log").children].filter((row) => isMessageRow(row) && !row.hasAttribute("data-absorbed"));
  const weight = (row: Element) => Math.max(1, idsOf(row).length);
  const total = rows.reduce((sum, row) => sum + weight(row), 0);
  // A channel with no thread at all has nothing in All but its top-level messages.
  const topLevel = providerFacts.threading() && selection().view === "flat" && threadPresence.any();
  /** @type {HistoryReach} */
  const reach = { total, proven: false, saved: false, dense: 0, scattered: total, from: null as number | null, start: false,
    whole: false, topLevel, threads: 0, full: 0 };
  /** @type {number | null} below this instant, a row is scattered */
  let floor: number | null = null;
  if (providerFacts.threading()) {
    const cover = channelStore.current().channel === String(selectedChannelId()) ? channelStore.current().views.get(viewKey()) : undefined;
    if (!cover) return reach;
    // Saved on this device and not read since: what it held when saved, and nothing it may have missed.
    reach.saved = !cover.live;
    const gap = Number.isFinite(cover.dense) ? cover.dense : null;
    if (cover.more === false && gap === null) reach.start = true;
    else if (reach.saved) floor = -Infinity;
    else if (gap !== null) floor = Number.isFinite(cover.floor) ? Math.max(gap, cover.floor as number) : gap;
    else if (cover.more === true && Number.isFinite(cover.floor)) floor = cover.floor;
    else return reach;
  } else {
    // Hide read's own list is a to-do list, not a stretch of the channel (`loadTodo`).
    if (readModes.hiding() || olderPages().moreAbove === undefined) return reach;
    // Drawn from this device: what was saved, and nothing it may have missed since.
    reach.saved = plainReach.saved();
    if (plainReach.gapFrom() !== null) floor = plainReach.gapFrom();
    else if (olderPages().moreAbove === false) reach.start = true;
    else floor = -Infinity;
  }
  reach.proven = !reach.saved;
  reach.scattered = 0;
  const tally = topLevel ? replyTally() : null;
  for (const row of rows) {
    const sent = rowSentMs(row);
    // A row whose time does not read is one the reads brought all the same, and counted with them.
    if (floor !== null && sent < floor) {
      reach.scattered += weight(row);
      continue;
    }
    reach.dense += weight(row);
    if (Number.isFinite(sent) && (reach.from === null || sent < reach.from)) reach.from = sent;
    if (!tally) continue;
    for (const message of rowMessages(row)) {
      const id = threadOf(message);
      if (!id || !message.thread || message.thread.is_root !== true) continue;
      const count = exactReplyCount(id, message, tally);
      if (count === 0) continue;
      reach.threads += 1;
      const entry = tally.threads.get(id);
      if (count !== null && (entry ? entry.held : 0) >= count) reach.full += 1;
    }
  }
  reach.whole = reach.start && reach.full === reach.threads;
  return reach;
}

/** The chat service the channel on screen is read from, as a sentence names it: "Slack", or "the chat service". */

function serviceWords() {
  const provider = providerOfChannel(selectedChannelId());
  const name = provider && typeof provider.name === "string" ? provider.name.trim() : "";
  return name || channelDirectory.providerName() || "the chat service";
}

/** What the last walk in the column being worked on found, under its control, until the search changes. */

function renderSearchOlderNote() {
  const note = el("search-older-note");
  const held = columnWork.active().searchNote;
  if (held && held.key !== searchKey()) columnWork.active().searchNote = null;
  const current = columnWork.active().searchNote;
  const said = current ? current.text : "";
  setTextIfChanged(note, said);
  if (note.hidden !== (said === "")) note.hidden = said === "";
}

/**
 * Whether walk `walk` in `column` is still the reader's: the same search, the same list, no jump since,
 * no filter change's walk back, and in the right column a thread still showing. Asked in that column.
 */

function searchWalkHolds(walk: SearchWalk, column: OlderWalkColumn): boolean {
  return !walk.ended && column.searchWalk === walk && searchKey() === walk.key &&
    channelContextKey() === walk.context && replyJumpTicket.current() === walk.ticket && !filterChanges.walking() &&
    (column !== columns.side || sideShowing());
}

/**
 * End `column`'s walk where it is, saying nothing of it: the read on the wire is aborted, and its
 * control says what it says with no walk. The walk's promise, to wait for, or null with none.
 *
 * @param {ColumnFields} column
 */

function endSearchWalk(column: OlderWalkColumn): Promise<void> | null {
  const walk = column.searchWalk;
  if (!walk) return null;
  walk.ended = true;
  column.searchWalk = null;
  if (walk.controller) walk.controller.abort();
  inColumn(column, renderOlderControl);
  return walk.promise;
}

/**
 * End the walk in either column, and the cards' (`endRepliesWalk`): something else is about to read
 * older pages, or move the reader.
 */

function endSearchWalks() {
  endSearchWalk(columns.main);
  endSearchWalk(columns.side);
  endRepliesWalk();
}

/**
 * Count what `walk` has found so far in the column being worked on: the messages, in shown rows, of the
 * rows above every row that was loaded when it started — the rows it prepended — and how many messages
 * those rows hold in all. A live arrival, or a poll, lands below them, and is never counted.
 */

function countSearchWalk(walk: SearchWalk): void {
  const rows = [...el("discord-log").children].filter(isMessageRow);
  let found = 0;
  let arrived = 0;
  for (const row of rows) {
    const ids = idsOf(row);
    if (ids.some((id) => walk.held.has(id))) break;
    arrived += ids.length;
    if (rowShown(row)) found += ids.length;
  }
  walk.found = found;
  walk.arrived = arrived;
}

/**
 * What a walk that has ended says under its control: see the head of this section, and
 * web/src/present/notices `searchWalkWords`, which says why it always says something; from this page's
 * service name, budget and oldest row.
 *
 * @param {any} walk
 * @param {string} reason
 * @param {unknown} [failure] what the read that failed threw, with `reason` "failed"
 */

function searchWalkNote(walk: SearchWalk, reason: string, failure: unknown = null): string {
  return searchWalkWords(walk, reason, {
    service: serviceWords(),
    budgetSeconds: Math.round(SEARCH_OLDER_MS / 1000),
    oldest: oldestLoadedMs(),
    viewWords: () => channelViewWords(),
    failure: () => failureWords(failure),
  });
}

/**
 * Search further back in the column being worked on: read older pages until the bounds at the head of
 * this section stop it, and say what it found. Does nothing where the step back does not offer it.
 */

function searchOlder() {
  if (!searchOffered() || columnWork.active().searchWalk || (filterChanges.walking() && !columnWork.side())) return Promise.resolve();
  const column = columnWork.active();
  // `#229 desktop-two-column`. The column the walk is in, which its work after every wait is done in.
  const steps = columnSteps();
  // One at a time on the page: a walk in the other column ends, and is waited for, so two reads never
  // overlap; so does the cards' (`searchReplies`). A filter change's walk back gives way, and the tap is
  // the reader's act in the list.
  const others = [endSearchWalk(column === columns.main ? columns.side : columns.main), endRepliesWalk()]
    .filter((one): one is Promise<void> => one !== null);
  endFilterWalk();
  readerActed();
  const area = el("scroll-area");
  const chip = el("load-older");
  // The walk starts with its control in view: older matches arrive under it, and that is where the
  // reader should be looking — not far down a list of matches, nor still in the search field.
  if (chip.getBoundingClientRect().top < area.getBoundingClientRect().top) area.scrollTop = 0;
  const walk: SearchWalk = {
    key: searchKey(),
    context: channelContextKey(),
    ticket: replyJumpTicket.current(),
    held: new Set([...el("discord-log").children].flatMap(idsOf)),
    pages: 0,
    // The steps that landed, Main's own newest page among them (`walkCursorOwed`): what a stopped walk read.
    read: 0,
    cursorRead: false,
    found: 0,
    arrived: 0,
    started: Date.now(),
    controller: typeof AbortController === "function" ? new AbortController() : null,
    stopping: false,
    ended: false,
    // The control holds still while the walk runs (`captureScroll`), until the reader moves it.
    anchored: true,
    chipTop: chip.getBoundingClientRect().top,
    promise: /** @type {Promise<void> | null} */ (null),
  };
  column.searchWalk = walk;
  columnWork.active().searchNote = null;
  renderOlderControl();
  renderJumpNewest();
  walk.promise = (async () => {
    let reason = "done";
    /** @type {unknown} what the read that failed threw */
    let failure = null;
    try {
      if (others.length > 0) {
        steps.pause();
        await Promise.all(others.map((one) => one.catch(() => {})));
        steps.resume();
      }
      let last = 0;
      while (walk.pages < SEARCH_OLDER_PAGES) {
        if (!searchWalkHolds(walk, column) || walk.stopping) break;
        if (olderPages().moreAbove !== true) {
          reason = "start";
          break;
        }
        if (!olderPages().cursor && !walkCursorOwed() && olderPages().step === null) {
          // Older history said to exist, and no cursor to reach it by: said, never a page that was not read.
          if (walk.pages === 0 && !walk.cursorRead) reason = "nowhere";
          break;
        }
        if (walk.pages > 0 && Date.now() - walk.started + last > SEARCH_OLDER_MS) {
          reason = "budget";
          break;
        }
        // A finger on the glass holds the next page, as it holds a step back (`maybeLoadOlder`).
        while (pullToRefresh.fingerDown() && searchWalkHolds(walk, column) && !walk.stopping) {
          steps.pause();
          await new Promise((resolve) => setTimeout(resolve, PULL_WAIT_MS));
          steps.resume();
        }
        if (!searchWalkHolds(walk, column) || walk.stopping) break;
        // Main drawn from All's cover takes its first step from its own newest page (`walkCursorOwed`),
        // which is no older page, and is not counted as one; once, for the cursor the walk goes on from.
        // What older messages it brings are counted as found like any others.
        const owed = olderPages().step === null && !olderPages().cursor && walkCursorOwed();
        if (owed && walk.cursorRead) break;
        walk.cursorRead = walk.cursorRead || owed;
        if (!owed) walk.pages += 1;
        renderOlderControl();
        const began = Date.now();
        const step = loadOlder(walk.controller ? walk.controller.signal : undefined);
        steps.pause();
        try {
          await step;
        } finally {
          steps.resume();
        }
        if (!owed) last = Date.now() - began;
        // A step Stop aborted ends quietly, and brought nothing.
        if (!(walk.controller && walk.controller.signal.aborted)) walk.read += 1;
        if (!searchWalkHolds(walk, column) || walk.stopping) break;
        countSearchWalk(walk);
        // How many have come so far, on the control (`searchWalkProgress`).
        renderOlderControl();
        if (walk.found >= SEARCH_OLDER_HITS) break;
      }
      if (walk.stopping) reason = "stopped";
      else if (reason !== "budget" && olderPages().moreAbove === false && walk.pages > 0) reason = "start";
    } catch (error) {
      if (walk.ended || walk.stopping || (walk.controller && walk.controller.signal.aborted)) {
        reason = walk.stopping ? "stopped" : "done";
      } else {
        reason = "failed";
        failure = error;
        // The failure itself is reported where every failed read is; this says the search stopped, and
        // the note under the control says why, until the search changes (`searchWalkNote`).
        setStatus("Could not search further back — try again.");
        throw error;
      }
    } finally {
      if (column.searchWalk === walk) {
        // A page that landed as Stop was pressed is counted with the rest. A walk another list or
        // another search has taken over from says nothing there.
        const holds = searchWalkHolds(walk, column);
        if (holds) countSearchWalk(walk);
        column.searchWalk = null;
        const text = holds ? searchWalkNote(walk, reason, failure) : "";
        column.searchNote = text ? { key: walk.key, text } : null;
        const write = () => {
          renderOlderControl();
          // The empty sentence's count and day, held while the walk read, now said once.
          applySearch();
        };
        // Under a control that held still, the note simply opens under it; under a reader who moved
        // away, their place is kept.
        if (walk.anchored || area.scrollTop === 0) write();
        else preservingScroll(write);
      }
      steps.pause();
    }
  })();
  return walk.promise;
}

/** Stop `column`'s walk at the end of the read on the wire, which is aborted; its control says "Stopping…". */

function stopSearchWalk(column: OlderWalkColumn): void {
  const walk = column.searchWalk;
  if (!walk || walk.stopping) return;
  walk.stopping = true;
  if (walk.controller) walk.controller.abort();
  inColumn(column, renderOlderControl);
}

/**
 * The step back pressed, in the column being worked on: Stop while a walk for the search is under way
 * (not in its first moments, which are the same tap again), the search further back while it offers
 * that, and else one step back, as ever.
 */

function olderControlPressed() {
  const walk = columnWork.active().searchWalk;
  if (walk) {
    if (Date.now() - walk.started >= SEARCH_OLDER_TAP_MS) stopSearchWalk(columnWork.active());
    return undefined;
  }
  if (searchOffered() && !(filterChanges.walking() && !columnWork.side())) return searchOlder();
  return loadOlder();
}

/**
 * Enter in the search field: search further back in the column holding the keys while the right
 * column shows a thread, else the left; nothing where its step back does not offer that. Whether a
 * walk began.
 *
 * Over the cards, their threads' older replies (`searchReplies`) instead: across the window as the
 * thread list, where Main behind it is out of sight; and in two columns with the keys in the cards, or
 * where Main has nothing older to search.
 */

function searchOlderFromField() {
  if (sideCardsShown()) {
    const left = layoutShown.columns() && inColumn(columns.main, () => searchOffered() && columns.main.searchWalk === null);
    const cards = !left || lastPane.current() === sidePane;
    // The cards' walk under way is not the left column's to end.
    if (cards && repliesWalks.current() !== null) return false;
    if (cards && repliesSearchOffered()) {
      guardQuietly(searchReplies)();
      return true;
    }
    if (!left) return false;
  }
  const column = sideShowing() && lastPane.current() === sidePane ? columns.side : columns.main;
  if (!inColumn(column, () => searchOffered() && column.searchWalk === null)) return false;
  guardQuietly(() => inColumn(column, searchOlder))();
  return true;
}

// --- the server's copy, searched (`#605`, `#607` and `#608 server-side-search`) ----------------------
//
// The owner, 2026-10-27: search the server's own copy first; upstream search is deferred, so the line
// says "no upstream search"; and it says exactly what was searched — "No upstream search · this page:
// back to DATE · server: back to DATE (complete / still expanding) · N messages searched". Hits the
// server holds older than what this page has loaded come alone (question 9), each with "Show in
// channel", the in-app jump (`#543`); "All channels" widens the server's search to every channel the
// server answers for (question 8; the current channel by default).
//
// THE SERVER ANSWERS ONLY FROM WHAT IT MAY HAND OUT. Its copy is searched under the rules a served read
// keeps (src/server_search.rs): a channel it may not answer for says why, and the line says so rather
// than claiming a search that did not happen. The page asks only where client-config says a copy is
// open, and it asks once per search, a moment after the typing pauses; a change of search, channel or
// the box asks again, and an answer to an older question is dropped.

// The search sent to the server — its key, ticket, hits, scopes, how much it examined and where it
// resumes, the rest it waits for (SERVER_SEARCH_DELAY_MS), and whether it covers every channel
// (`#search-all-channels`, for this page's life) — is web/src/core/search's
// (`createServerSearch`, `#624 voice-page-modularity`); which search is on screen, its request and
// drawing what came back are here.

function serverSearchKeyNow() {
  if (!searchField.open() || screens.screen() !== "main" || screens.view() !== "discord" || !searchTyped(searchField.query())) return "";
  const channel = String(selectedChannelId() || "");
  if (!channel) return "";
  return JSON.stringify([serverSearchOwner.allChannels() ? "*" : channel, searchIdentity(searchField.query())]);
}

/**
 * Ask the server again if the search wanted has changed since it was last asked: after every pass of
 * the search in the left column, and when All channels is ticked. Cheap when nothing changed.
 */

function scheduleServerSearch() {
  const key = serverSearchKeyNow();
  // A gathering asked for elsewhere, or with the search closed, is no longer watched here; the
  // server's own bump clears itself once the walk reaches the channel's beginning.
  if (gathering.state().channel !== "" && (key === "" || !searchFurtherRunning())) stopSearchFurther();
  serverSearchOwner.schedule(key, serverCopyOpen());
}

/** This browser's time zone, which the server resolves `before:` and `after:` in. */

function serverScopeWords() {
  const serverSearch = serverSearchNow();
  if (serverFacts.tokenScope() === "read") return `server: not searched (${SERVER_SEARCH_REASONS.read_scope})`;
  if (!serverCopyOpen()) return "server: keeps no copy";
  if (serverSearch.failed) return `server: could not be searched (${serverSearch.failed})`;
  if (serverSearch.loading && serverSearch.scopes.length === 0) return "server: searching…";
  const scopes = serverSearch.scopes;
  if (scopes.length === 0) return "server: not searched yet";
  const searched = scopes.filter((scope) => scope.state === "searched");
  const more = serverSearch.resume !== null;
  if (serverSearchOwner.allChannels()) {
    const whole = searched.filter((scope) => scope.whole).length;
    // Every channel searched reaches back at least as far as the NEWEST of their reaches, and only
    // when each one says how far it reached.
    const backs = searched.map((scope) => scope.searched_back_to_ms);
    const all = backs.length > 0 && backs.every((ms) => typeof ms === "number");
    const back = all ? `, all back to ${searchMomentWords(Math.max(.../** @type {number[]} */ (backs)))}` : "";
    const left = scopes.length - searched.length;
    return `server: ${searched.length} of ${scopes.length} channel${scopes.length === 1 ? "" : "s"}` +
      `${back}${more ? " so far" : ""}${whole > 0 ? `, ${whole} whole` : ""}${left > 0 ? `, ${left} not searched` : ""}`;
  }
  const scope = scopes[0] as SearchScope;
  if (scope.state !== "searched") {
    return `server: not searched (${SERVER_SEARCH_REASONS[scope.reason || ""] || "it could not answer"})`;
  }
  const state = scope.whole ? " (complete)" : scope.expanding ? " (still expanding)" : "";
  if (typeof scope.searched_back_to_ms === "number") {
    return `server: back to ${searchMomentWords(scope.searched_back_to_ms)}${more ? " so far" : ""}${state}`;
  }
  // No stretch the server vouches it holds whole was searched to its end: say what was done.
  if (more) return `server: its newest part searched so far${state}`;
  return `server: all it holds searched, with gaps${state}`;
}

/** The line: what was searched, where, and how far back, in one sentence. */

function searchScopeLine() {
  const loaded = columns.main.searched ? columns.main.searched.loaded : 0;
  const from = oldestLoadedMs();
  const page = historyReadToStart() && historyReach().whole ? "this page: back to the first message"
    : from === null ? "this page: nothing loaded" : `this page: back to ${searchMomentWords(from)}`;
  // Over every channel the server searched this channel's newest too: counted once.
  const total = (serverSearchOwner.allChannels() && serverSearchNow().scopes.length > 0 ? 0 : loaded) + serverSearchNow().examined;
  return ["No upstream search", page, serverScopeWords(), searchFurtherWords(),
    `${total} message${total === 1 ? "" : "s"} searched`].filter((part) => part !== "").join(" · ");
}

/** One hit of the server's, alone: who, where, when, what it says, and the way to it in its channel. */

function serverHitNode(hit: Message): HTMLElement {
  const li = document.createElement("li");
  li.className = "server-hit";
  li.setAttribute("data-id", String(hit.id));
  li.setAttribute("data-channel", String(hit.channel_id));
  const meta = document.createElement("div");
  meta.className = "meta server-hit-meta";
  const who = document.createElement("span");
  who.className = "who";
  who.textContent = String(hit.author || "");
  meta.append(who);
  if (serverSearchOwner.allChannels()) {
    // The answer names every channel it searched, the one the page knows by the owner's name.
    const scope = serverSearchNow().scopes.find((one) => String(one.channel.id) === String(hit.channel_id));
    const where = document.createElement("span");
    where.textContent = `#${channelName(channelDirectory.channels().find((one) => String(one.id) === String(hit.channel_id)) ||
      (scope ? scope.channel : { id: hit.channel_id }))}`;
    meta.append(where);
  }
  const when = document.createElement("span");
  const at = Date.parse(hit.timestamp);
  when.textContent = Number.isNaN(at) ? "" : searchMomentWords(at);
  meta.append(when);
  if (hit.thread && hit.thread.is_root === false) {
    const reply = document.createElement("span");
    reply.textContent = "in a thread";
    meta.append(reply);
  }
  const show = document.createElement("button");
  show.className = "chip server-hit-show";
  show.setAttribute("type", "button");
  show.textContent = "Show in channel";
  show.addEventListener("click", () => guardQuietly(() => jumpToMessageLink({
    service: String(channelDirectory.providerName() || ""),
    channel: String(hit.channel_id),
    message: String(hit.id),
    thread: hit.thread && hit.thread.is_root === false ? String(hit.thread.id) : null,
  }))());
  // As a row's parts are, so the search's marks find its words (`findableParts`).
  const body = document.createElement("div");
  body.className = "body";
  body.append(messageBody(hit));
  li.append(meta, body, show);
  markSearchHits(li, searchParsed(searchField.query()).terms);
  return li;
}

/** Draw the server's search as it stands: the line, the hits this page does not already hold, More. */

function renderServerSearch() {
  const serverSearch = serverSearchNow();
  const section = el("server-search");
  const wanted = serverSearch.key !== "";
  if (section.hidden === wanted) section.hidden = !wanted;
  if (!wanted) {
    if (el("server-hits").children.length > 0) el("server-hits").replaceChildren();
    return;
  }
  setTextIfChanged(el("search-scope"), searchScopeLine());
  const checkbox = el("search-all-channels") as HTMLInputElement;
  if (checkbox.checked !== serverSearchOwner.allChannels()) checkbox.checked = serverSearchOwner.allChannels();
  const channel = String(selectedChannelId() || "");
  const held = heldMessagesById();
  const fresh = serverSearch.hits.filter((hit) => !(String(hit.channel_id) === channel && held.has(String(hit.id))));
  const drawn = [...el("server-hits").children].map((li) => `${li.getAttribute("data-channel")} ${li.getAttribute("data-id")}`);
  const want = fresh.map((hit) => `${hit.channel_id} ${hit.id}`);
  if (drawn.join("\n") !== want.join("\n")) el("server-hits").replaceChildren(...fresh.map(serverHitNode));
  const more = serverSearch.resume !== null && !serverSearch.loading && !serverSearch.failed;
  if (el("server-more").hidden === more) el("server-more").hidden = !more;
  renderSearchFurther();
}

// `#608 server-side-search`. "Search further back": the server is asked to gather this channel's older
// history next (`#602 history-settings-and-limits`' prioritize, POST .../history/prioritize), at its
// own pace under the chat service's limits; the page asks how far it has got every few seconds, says
// so on the line, and searches the server's copy again each time it holds older messages. Stop takes
// the request back (DELETE). Never automatic: the owner chose a button over background scans.

// Gathering the channel's older history — the ask, its run, the walk's state, why it failed and the
// poll (SEARCH_FURTHER_POLL_MS) — is web/src/core/search's (`createGathering`, `#624
// voice-page-modularity`); its requests and drawing it are here.

function searchFurtherOffered() {
  if (serverSearchNow().key === "" || serverSearchOwner.allChannels() || !serverCopyOpen() || serverFacts.tokenScope() === "read") return false;
  const scope = serverSearchNow().scopes[0] as SearchScope | undefined;
  return scope !== undefined && scope.state === "searched" && !scope.whole;
}

/** The line's words for a gathering under way, or "". */

function searchFurtherWords() {
  if (!searchFurtherRunning()) return "";
  if (gathering.state().failed) return `gathering older history: ${gathering.state().failed}`;
  const state = gathering.state().state;
  if (!state) return "asking the server to gather older history…";
  const reached = typeof state.reached_ms === "number" ? state.reached_ms : state.oldest_ms;
  const back = typeof reached === "number" ? ` back to ${searchMomentWords(reached)}` : "";
  const eta = typeof state.eta_s === "number" ? `, about ${Math.max(1, Math.round(state.eta_s / 60))} min left` : "";
  if (state.complete) return `gathered the whole channel${back}`;
  if (state.held_reason === "rate_limited") return `gathering older history paused by the chat service's limit${back}`;
  return `gathering older history${back}${eta}`;
}

/** Show Search further back, or Stop while a gathering this page asked for runs. */

function renderSearchFurther() {
  const button = el("search-further") as HTMLButtonElement;
  const running = searchFurtherRunning();
  const offered = running || searchFurtherOffered();
  if (button.hidden === offered) button.hidden = !offered;
  setTextIfChanged(button, running ? "Stop gathering older history" : "Search further back");
}

/** The press: ask the server to gather this channel's older history next, or take that back. */

async function searchFurtherPressed() {
  const channel = String(selectedChannelId() || "");
  if (!channel) return;
  if (searchFurtherRunning()) {
    stopSearchFurther();
    await api(`/api/v1/channels/${encodeURIComponent(channel)}/history/prioritize`, { method: "DELETE" })
      .catch(() => null);
    return;
  }
  await gathering.start(channel);
}

/** All channels ticked or unticked: the server is asked again over the channels it now names. */

function chooseSearchAllChannels(on: boolean): void {
  serverSearchOwner.chooseAllChannels(on);
  scheduleServerSearch();
}

// --- searching the threads' replies further back ----------------------------------------------------
//
// `#262 search-load-older`, the cards' half. The owner's search found nothing in two columns, and the
// walk above was not enough to find it there: their left column is Main, which holds a thread's first
// message and none of its replies, and the cards on the right were searched by the replies the page
// already held (`filterSideCards`) with no way to read any further back. Measured in the page suite, a
// word said only in older replies was found nowhere in two columns, nor over the thread list, however
// far back Main was walked, where one column on All walks back and finds every one.
//
// So WHILE A SEARCH IS TYPED OVER THE CARDS — in two columns, or the thread list on its own at any
// width, a phone's included — the head of the cards says "Search replies before Oct 3": the day All's
// pages reach back to, every reply since then being held. A tap on it, or Enter in the search field
// where the cards have the keys (`searchOlderFromField`), reads All's older pages, which hold every
// thread's replies, into the channel's store, drawing them in no list; the cards are searched again by
// the replies held, a card found by them says "in replies" (`markReplyMatch`), and its thread opened
// shows what was found. Where All has no cursor of its own — never drawn, or saved on this device —
// its newest page is read first, behind the rows as a read of All behind Main is, and is not counted.
// The bounds are the walk's above, in threads: at most `SEARCH_OLDER_PAGES` pages, within
// `SEARCH_OLDER_MS`, ending at the end of the page that brings `SEARCH_OLDER_HITS` threads. Meanwhile
// the control says "Fetching… 50 older messages · Stop", and a tap on it stops the read on the wire.
//
// WHAT IT FOUND is the threads whose cards the filters show now and did not when it started, among those
// a page it read brought a message of: a thread arriving live meanwhile is shown, but is not the walk's
// find, nor ends it. Said once under the control until the search changes, with how far back the replies
// now reach. A reply it read that says every word, in a thread with no card — not in the threads loaded,
// its first message older than All reaches — is said too, so that "back to" claims no more than was
// searched. It says how it ended whatever it found: stopped, at the start of the channel, out of time,
// or with nothing more.
// Once All reaches the start of the channel no walk is offered, and a list the search empties says all
// of it is loaded (`sideListState`). Any change to the search, a thread selected on the right, the
// cards leaving the screen or another channel ends a walk where it is; one walk at a time on the page,
// with the columns' (`searchOlder`), each ending the other.
//
// THE PAGE ONLY. It reads All's older pages as a step back in All does; nothing on the server is new.

// The cards' walk under way and what the last one found, and the walk itself: one owner in
// web/src/core/search/replies.ts (`#624 voice-page-modularity`), made below where its parts are.

/** Everything the cards' walk depends on: a change in either ends it, and clears its note. */

const repliesKey = () => JSON.stringify([String(selectedChannelId()), searchField.open() ? searchIdentity(searchField.query()) : null]);

/** All's cover in the selected channel's store, or null where no page of All has covered it. */

function allCover() {
  if (channelStore.current().channel !== String(selectedChannelId())) return null;
  return channelStore.current().views.get(viewKey("flat")) || null;
}

/** Whether All reaches the start of the channel, so that the store holds every reply it has. */

const allLoaded = () => {
  const cover = allCover();
  return Boolean(cover && cover.more === false);
};

/** Whether the right column's cards are on screen: two columns with no thread on the right, or the thread list. */

function repliesSearchOffered() {
  return providerFacts.threading() && sideCardsShown() && searchField.open() && searchTyped(searchField.query()) &&
    channelStore.current().channel === String(selectedChannelId()) && !channelStore.current().scope && !allLoaded();
}

/** The instant All's pages reach back to, or null where that is not known. */

function repliesFloor() {
  const cover = allCover();
  return cover && cover.more !== false && Number.isFinite(cover.floor) ? cover.floor : null;
}


/** The cards the search and the read mode show now, by thread id. */

function repliesWalkHolds(walk: unknown): boolean {
  return repliesWalks.holds(walk);
}

/** End the cards' walk where it is, saying nothing of it; its read on the wire is aborted. Its promise, or null. */

function endRepliesWalk() {
  return repliesWalks.end();
}

/**
 * The control at the head of the cards and the note under it, as the walk, the search and All's reach
 * say: hidden where nothing is offered and nothing is under way. A walk that a change has made nobody's
 * ends first. The reader's place in the cards is kept as either comes or goes (`keepingCardPlace`).
 */

function renderRepliesSearch() {
  const under = repliesWalks.current();
  if (under && !repliesWalkHolds(under)) {
    endRepliesWalk();
    return;
  }
  const walk = repliesWalks.current();
  const said = repliesWalks.noteFor(repliesKey);
  const button = leftEl("side-search-replies") as HTMLButtonElement;
  const note = leftEl("side-replies-note");
  const offered = walk !== null || repliesSearchOffered();
  let label = button.textContent;
  if (walk) {
    label = walk.stopping ? "Stopping…" : searchWalkProgress(walk);
  } else if (offered) {
    // `#578 search-status-clarity`. To the minute, as the columns' control says it.
    const floor = repliesFloor();
    label = floor === null ? "Search thread replies" : `Search replies before ${searchMomentWords(floor)}`;
  }
  const draw = () => {
    if (button.hidden === offered) button.hidden = !offered;
    setTextIfChanged(note, said);
    if (note.hidden !== (said === "")) note.hidden = said === "";
  };
  if (button.hidden === offered || note.textContent !== said || note.hidden !== (said === "")) keepingCardPlace(draw);
  setTextIfChanged(button, label);
  const disabled = walk ? walk.stopping : false;
  if (button.disabled !== disabled) button.disabled = disabled;
  const name = offered && !walk ? `${label}: fetches up to ${SEARCH_OLDER_PAGES * DISCORD_PAGE_LIMIT} older messages ` +
    `of the whole channel, replies and all, from ${serviceWords()}, and searches them here.` : "";
  if ((button.getAttribute("aria-label") || "") !== name) {
    if (name) button.setAttribute("aria-label", name);
    else button.removeAttribute("aria-label");
  }
}

/**
 * One page of the cards' walk into the channel's store, as one of All's own goes in: an older page as a
 * step back in All folds one (`applyTimelinePage`), All's cover reaching back with it and keeping its
 * cursor for All's own step back where this page is where that cover now ends; All's newest page, read
 * for a cursor, as a read of All behind Main is (`foldLateTimelinePage`), from the store's copies as it
 * was sent (`held`). Main's counts follow the replies it brings, and the cards are drawn and searched
 * again from the store.
 *
 * @param {VibeTalk.TimelineResponse} payload
 * @param {boolean} newest
 * @param {Map<string, any> | null} held
 * @param {{channel: string, canon: any}} walk
 */


/**
 * What a cards' walk that has ended says under its control: see the head of this section. And after it,
 * the matching replies it read in threads with no card (`repliesUnlisted`).
 */


/** How many threads with a reply cards' walk `walk` read saying every word have no card. */


/**
 * How cards' walk `walk` ended, and what it found: see the head of this section, and web/src/present/notices
 * `repliesWalkWords`; from this page's service name, budget and All's reach.
 *
 * @param {any} walk
 * @param {string} reason
 * @param {unknown} [failure] what the read that failed threw, with `reason` "failed"
 */


/**
 * The cards' walk back through All's older pages for the search, while one is under way, and what the last
 * one found (web/src/core/search `createRepliesWalk`, `#624 voice-page-modularity`): the bounds at the head
 * of this section, the reads, and how it ended. Reading a page, folding it and drawing the control are here.
 */

function searchReplies() {
  return repliesWalks.search();
}

/** Stop the cards' walk at the end of the read on the wire, which is aborted; its control says "Stopping…". */


function renderOlderControl() {
  const button = el("load-older") as HTMLButtonElement;
  // `#262 search-load-older`. A walk for the search that a change has made nobody's ends first, so
  // nothing below says it is still searching.
  const running = columnWork.active().searchWalk;
  if (running && !searchWalkHolds(running, columnWork.active())) endSearchWalk(columnWork.active());
  const walk = columnWork.active().searchWalk;
  // Not over the Pinned filter: its list is every pin already, and older history adds none.
  const hidden = olderPages().moreAbove !== true || pinsView().only;
  if (button.hidden !== hidden) button.hidden = hidden;
  // `#243 filter-keeps-selection`. A walk back to the message a filter change is keeping says what it
  // is doing, where the reader waits for it.
  const walking = filterChanges.walking() && !columnWork.side();
  const offered = !walking && walk === null && searchOffered();
  // A tap while a step of the scroll's is on the wire starts the search, which takes that step as its
  // first page; and a walk under way is stopped by one, unless it is already stopping.
  const disabled = walk ? walk.stopping : olderPages().inFlight && !offered;
  if (button.disabled !== disabled) button.disabled = disabled;
  let label = olderPages().inFlight ? (walking ? "Loading earlier messages…" : "Loading older messages…") : "Older messages";
  if (walk) {
    label = walk.stopping ? "Stopping…" : searchWalkProgress(walk);
  } else if (offered) {
    // `#578 search-status-clarity`. To the minute, so that every press that brings anything older moves it.
    const sent = oldestLoadedMs();
    label = `Search ${selection().view === "thread" ? "replies " : ""}${sent === null ? "older messages" : `before ${searchMomentWords(sent)}`}`;
  }
  setTextIfChanged(button, label);
  const name = offered ? `${label}: fetches up to ${SEARCH_OLDER_PAGES * DISCORD_PAGE_LIMIT} older messages from ` +
    `${serviceWords()} and searches them here. Enter in the search field does the same.` : "";
  if ((button.getAttribute("aria-label") || "") !== name) {
    if (name) button.setAttribute("aria-label", name);
    else button.removeAttribute("aria-label");
  }
  renderSearchOlderNote();
}

// --- searching further back ------------------------------------------------------------------------
//
// `#262 search-load-older`. The owner, 2026-10-12: "searching for a term XYZ in two column view found
// zero hits; in one column, scrolling up while searching loaded older pages and found 3 ... 2 column
// view should not PRECLUDE loading hits". The search filters what is loaded, and a step back was fired
// only by the list's scroll position reaching its head, which under a filter nothing bounded: measured in
// Chromium, a search that emptied the list chained steps back through the channel's whole history, each
// page drawn and hidden moving the position enough to fire the next; on Main, which holds no replies,
// that is a walk to the start that finds nothing. The step back meanwhile said "Older messages" and
// nothing about finding. So the scroll's own step gives way while a filter chooses rows
// (`maybeLoadOlder`), and the search's walk below is the one way back, bounded, and the reader's.
//
// WHILE A FILTER CHOOSES ROWS (a term, Links or whose messages), each column's step back says "Search
// before Oct 3, 9:02 AM", the oldest message that column reaches back to (to the minute since `#578
// search-status-clarity`). A tap on it, or Enter in the search field, walks
// back AT MOST `SEARCH_OLDER_PAGES` older pages within `SEARCH_OLDER_MS`, and stops at the end of the
// page that brings `SEARCH_OLDER_HITS` new matches. Every tap is the reader's consent to that much and
// no more: nothing chains pages on its own. Meanwhile the control says "Fetching… 50 older messages · Stop",
// and a tap on it stops the read on the wire. Any change to the search ends the walk, and so do closing the
// right column's thread, leaving two columns, another channel or view, a jump, and a filter change's
// own walk back (`walkToFilterAnchor`). One walk at a time on the page.
//
// WHAT IT FOUND is counted only among messages older than where it started, so a message arriving
// live, or a poll, can neither stop it nor be reported as found; and in messages, as the count is. It
// is said once, under the control, until the search changes (`searchNote`), with what was fetched and
// from where, or why nothing could be (`searchWalkNote`). The matches appear under
// the control, which holds still while the walk runs, with `#250 date-separators`' line saying how many
// unmatched messages are between, and `#249 search-highlight`'s marks on them.
//
// THE PAGE ONLY. No route is new, and nothing reaches the server that a scroll up would not have sent.

/** How many older pages one tap of "Search before …", or Enter in the search field, reads at most. */

  return {
    renderOlderControl, olderControlPressed, searchOlder, searchOlderFromField,
    searchReplies, searchOffered, searchKey, endSearchWalks, endSearchWalk, stopSearchWalk,
    renderSearchOlderNote, historyReach, renderServerSearch, searchFurtherPressed, chooseSearchAllChannels, renderRepliesSearch, scheduleServerSearch, channelViewWords, oldestLoadedMs, serviceWords, repliesFloor, repliesSearchOffered, allCover, allLoaded, repliesKey, endRepliesWalk,
    dispose() { disposed = true; },
  };
}

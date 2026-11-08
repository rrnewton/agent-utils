// Keeping the channel on screen current (`#203 incremental-refresh`, `#226
// page-energy-profile`; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel; mu-readcore): the read queue's release, the poll, what a hidden page pauses
// and resumes, and landing a read the page gave up on. The queue, poll and hidden-stream
// state machines are core/sync's; this owner only binds them to the page's reads and columns.
// Reads asked for here go through the page's own `loadDiscord` name, passed in, so what wraps
// that name in the page still sees every read.

import type { TimelineResponse } from "../../contract/index.ts";
import type { Message } from "../../contract/index.ts";
import type { ChannelStore } from "../../core/channels/index.ts";
import type { TimelineAbandonedRead, TimelineColumn, TimelineReadOptions } from "./readcore.ts";

/** The read queue (core/sync `createChannelReads`), as refresh uses it. */
export interface RefreshReads {
  queue(column: TimelineColumn, options?: TimelineReadOptions): void;
  release(columns: TimelineColumn[]): { column: TimelineColumn; options: TimelineReadOptions } | null;
  busy(): boolean;
}

/** The poll (core/sync `createChannelPoll`), as refresh uses it. */
export interface RefreshPoll {
  stop(): void;
  schedule(delay?: number): void;
  pause(): void;
  resume(how: string): void;
  armed(): boolean;
  dueAt(): number;
}

/** What refreshing asks of the page. */
export interface ChannelRefreshDependencies {
  channelReads: RefreshReads;
  channelPoll: RefreshPoll;
  hiddenStream: { hide(): void; show(): void; noteArrivals(): void; takeArrivals(owed: () => boolean): { mutation: boolean; arrived: boolean } };
  el(id: string): HTMLElement;
  channelStore: ChannelStore;
  viewKey(view?: string, thread?: string | null): string;
  channelReadPosition(area: HTMLElement): { wasAtNewest: boolean; previousTop: number; anchorId: string | null; anchorTop: number; around: unknown };
  landedReadPosition(area: HTMLElement, keepPosition: boolean, sentAtNewest: boolean): { wasAtNewest: boolean; keepPosition: boolean; previousTop: number; anchorId: string | null; anchorTop: number; around: unknown };
  settleAfterRead(messages: readonly Message[], how: unknown): void;
  applyTimelinePage(payload: TimelineResponse, older?: boolean, saved?: boolean, delta?: boolean, rows?: readonly Message[] | null, held?: Map<string, Message> | null): Message[];
  foldTimelineDelta(payload: TimelineResponse, sent: string | null, view?: string, thread?: string | null): boolean;
  holdingScrollTools<T>(draw: () => T): T;
  noteFreshRead(payload: TimelineResponse): void;
  saveChannelScope(): void;
  renderScrollTools(): void;
  requestVisibleSummaries(): void;
  followInOtherColumn(): void;
  catchUpFromAll(): void;
  catchUpHeldView(words?: boolean): boolean;
  readFilterKeeps(thread?: unknown): (message: Message) => boolean;
  hiddenEmpty(message: Message): boolean;
  hidingRead(): boolean;
  timelineRows(): { messages: Message[] };
  syncUnreadReplies(): void;
  inOtherColumn(fn: () => void): void;
  providerFacts: { threading(): boolean };
  screensView(): string;
  liveFollow: { generation(): number; channel(): string | null };
  token(): string;
  startChannelStream(channel: string, from: unknown): void;
  replayWaiting(): number;
  channelContextKey(): string;
  columnWork: { active(): TimelineColumn };
  columns: { main: TimelineColumn; side: TimelineColumn };
  inColumn<T>(column: TimelineColumn, fn: () => T): T;
  loadDiscord(options?: TimelineReadOptions): Promise<void>;
  loadTimeline(options?: TimelineReadOptions): Promise<void>;
  foldLateTimelinePage(payload: TimelineResponse, read: TimelineAbandonedRead): void;
  renderChannelLoading(on: boolean, quiet?: boolean, background?: boolean): void;
  queuePlaceUnplaced(): void;
  loadStateGeneration(): number;
  selectedChannelId(): string;
  readModesHiding(): boolean;
  refreshQuietly(fn: () => Promise<void>): () => Promise<void>;
  configApplied(): boolean;
  signIn(): Promise<boolean>;
  replayStarted(): unknown;
  readSideColumn(reason: string, since: unknown): Promise<void>;
  screensView(): string;
  discordPollMs: number;
  startChannelStream(): void;
  renderChannelNavigation(): void;
  applySearch(): void;
}

/** Refreshing's commands: the legacy names, unchanged. */
export interface ChannelRefresh {
  queueDiscordLoad(options?: TimelineReadOptions): void;
  finishDiscordLoad(column?: TimelineColumn): Promise<void>;
  currentDiscordLoad(generation: number, channel: string, readingTodo: boolean): boolean;
  stopDiscordPolling(): void;
  scheduleDiscordPoll(delay?: number): void;
  pollDiscord(): void;
  pauseWhileHidden(): void;
  resumeLiveStream(resume: { generation: number; channel: string; from: unknown } | null): void;
  resumePolling(how: string): void;
  catchUpHiddenArrivals(): string | null;
  landAbandonedTimelineRead(payload: TimelineResponse, read: TimelineAbandonedRead): void;
  dispose(): void;
}

/** Refreshing over `deps`. */
export function createChannelRefresh(deps: ChannelRefreshDependencies): ChannelRefresh {
  const channelReads = deps.channelReads;
  const channelPoll = deps.channelPoll;
  const hiddenStream = deps.hiddenStream;
  const columnWork = deps.columnWork;
  const columns = deps.columns;
  const inColumn = deps.inColumn;
  const loadDiscord = deps.loadDiscord;
  const loadTimeline = deps.loadTimeline;
  const foldLateTimelinePage = deps.foldLateTimelinePage;
  const renderChannelLoading = deps.renderChannelLoading;
  const queuePlaceUnplaced = deps.queuePlaceUnplaced;
  const loadState = () => ({ generation: deps.loadStateGeneration() });
  const selectedChannelId = deps.selectedChannelId;
  const readModes = { hiding: () => deps.readModesHiding() };
  const refreshQuietly = deps.refreshQuietly;
  const serverFacts = { configApplied: () => deps.configApplied() };
  const signIn = deps.signIn;
  const replayLedger = { started: () => deps.replayStarted(), waiting: () => deps.replayWaiting() };
  const readSideColumn = deps.readSideColumn;
  const screens = { view: () => deps.screensView() };
  const DISCORD_POLL_MS = deps.discordPollMs;
  const el = deps.el;
  const channelStore = deps.channelStore;
  const viewKey = deps.viewKey;
  const channelReadPosition = deps.channelReadPosition;
  const landedReadPosition = deps.landedReadPosition;
  const settleAfterRead = deps.settleAfterRead;
  const applyTimelinePage = deps.applyTimelinePage;
  const foldTimelineDelta = deps.foldTimelineDelta;
  const holdingScrollTools = deps.holdingScrollTools;
  const noteFreshRead = deps.noteFreshRead;
  const saveChannelScope = deps.saveChannelScope;
  const renderScrollTools = deps.renderScrollTools;
  const requestVisibleSummaries = deps.requestVisibleSummaries;
  const followInOtherColumn = deps.followInOtherColumn;
  const catchUpFromAll = deps.catchUpFromAll;
  const catchUpHeldView = deps.catchUpHeldView;
  const readFilterKeeps = deps.readFilterKeeps;
  const hiddenEmpty = deps.hiddenEmpty;
  const hidingRead = deps.hidingRead;
  const timelineRows = deps.timelineRows;
  const syncUnreadReplies = deps.syncUnreadReplies;
  const inOtherColumn = deps.inOtherColumn;
  const providerFacts = deps.providerFacts;
  const liveFollow = deps.liveFollow;
  const token = deps.token;
  const startChannelStream = deps.startChannelStream;
  const channelContextKey = deps.channelContextKey;
  let disposed = false;
  void disposed;
  void loadTimeline;

function queueDiscordLoad(options?: TimelineReadOptions): void {
  // Every reason kept, the newest options otherwise, `quiet` from either (web/src/core/sync `joinQueuedReads`).
  channelReads.queue(columnWork.active(), options);
}

/**
 * Release the fetch lock, then perform the newest read that was requested while it was held: the
 * left column's first, in its own column. `column` is the column of the read that has just ended,
 * whose loading line goes when nothing is queued behind it.
 */

async function finishDiscordLoad(column: TimelineColumn = columnWork.active()): Promise<void> {
  const queued = channelReads.release([columns.main, columns.side]);
  if (queued) {
    const next = queued.column;
    await inColumn(next, () => loadDiscord(/** @type {any} */ (queued.options)));
    // The read handed on may not have gone at all: a poll put off while the server still reads one
    // given up on (`readHeld`, `remote-ops-audit` W7). Then no read is out, and its column's loading
    // line, which nothing else would take down, goes here.
    if (!channelReads.busy()) inColumn(next, () => renderChannelLoading(false));
    return;
  }
  inColumn(column, () => renderChannelLoading(false));
  // The channel's reads have gone quiet: what the stream left unplaced and they did not place is
  // read for now. `#308 live-reply-in-main`.
  queuePlaceUnplaced();
}

/** Whether a completed read still describes the channel and mode currently selected. */

function currentDiscordLoad(generation: number, channel: string, readingTodo: boolean): boolean {
  return (
    generation === loadState().generation &&
    String(channel) === String(selectedChannelId()) &&
    readingTodo === readModes.hiding()
  );
}

/** Stop the poll, and forget a wait held for the page to be shown. */

function stopDiscordPolling(): void {
  channelPoll.stop();
}

/**
 * Arm the next poll of the channel on screen after `delay`, held while the page is hidden
 * (web/src/core/sync `ChannelPoll.schedule`, which says why it reschedules itself).
 *
 * @param {number} [delay]
 */

function scheduleDiscordPoll(delay: number = DISCORD_POLL_MS): void {
  channelPoll.schedule(delay);
}

/** One poll of the channel on screen, which arms the next. */

function pollDiscord(): void {
  refreshQuietly(async () => {
    // A failed poll is the case polling exists for — the network comes back — so the next one
    // is armed whatever this one did. A page that opened offline never heard `/client-config`
    // either, and it asks again first.
    let refused = false;
    try {
      refused = !serverFacts.configApplied() && !(await signIn());
      const since = replayLedger.started();
      if (!refused) await loadDiscord({ keepPosition: true, reason: "poll" });
      // `#229 desktop-two-column`. And the right column's thread, or its list of threads: another
      // view, one delta, never Main's again — nor the thread's again, when it was selected under
      // Main's read and read for that (`#231 two-column-polish`).
      if (!refused) await readSideColumn("poll", since);
    } finally {
      if (screens.view() === "discord" && !refused) {
        scheduleDiscordPoll();
      }
    }
  })();
}

// --- a hidden page -------------------------------------------------------------------------------
//
// `#226 page-energy-profile`. The owner keeps this page open as an installed app, on a desk and on
// a phone, and asked that it not drain either. Measured with the window minimized, so that the page
// really was hidden: the poll kept its forty-five seconds, because a timer re-armed from a fetch's
// continuation is not one Chromium's background throttling slows; every live message was re-read
// and every row rebuilt, forced layouts and all, for a page nobody could see; and the stream's
// keep-alive, every fifteen seconds, is what keeps a phone's radio from ever going idle.
//
// So, while the page is hidden:
//
//   * THE POLL WAITS. One armed, or come due, resumes when the page is shown: a fresh wait, or now
//     when its time passed while hidden.
//   * A LIVE MESSAGE GOES INTO THE STORE AND NO FURTHER. No row, no read. An edit or a removal waits
//     for a read the same way. The agent of a call still hears it (`relayToAgent`): a call is what
//     the phone in a pocket is most likely to be doing.
//   * AFTER `LIVE_HIDDEN_CLOSE_MS` WITHOUT A CALL, THE STREAM IS LET GO, with the last event it saw.
//     During a call it stays: that is how the agent hears the channel.
//
// Coming back pays for all of it at once (`onVisibility`): what the store took in is drawn from the
// store as a read would have drawn it, the newest line followed or the reader's place kept and what
// arrived counted; then one read of what changed; then the stream, resumed from its last event, so
// what it replays is held already and nothing between is skipped. The pill says "Updated" rather
// than "Live · updated" from the stream's going until it is back, which is the only thing a reader
// can see of this, and only in the moment after a return.

// What arrived while hidden, the timer that lets the stream go after `LIVE_HIDDEN_CLOSE_MS` with no call
// listening, and the stream let go to resume: one owner in web/src/core/sync/hidden.ts (`#624
// voice-page-modularity`).

function pauseWhileHidden(): void {
  channelPoll.pause();
  hiddenStream.hide();
}

/** Follow the stream let go while hidden again, from its last event, unless something else has since. */

function resumeLiveStream(resume: { generation: number; channel: string; from: unknown } | null): void {
  if (resume === null || resume.generation !== liveFollow.generation() || liveFollow.channel() !== null) return;
  if (String(selectedChannelId()) !== resume.channel || !token()) return;
  startChannelStream(resume.channel, resume.from);
}

/**
 * Take up the poll a hidden page paused: now, a whole wait, or the rest of the one it was in
 * (web/src/core/sync `ChannelPoll.resume`).
 *
 * @param {"now" | "fresh" | "rest"} how
 */

function resumePolling(how: string): void {
  channelPoll.resume(how);
}

/**
 * Back in sight: draw what the stream put in the store while the page was hidden, as a read would
 * have drawn it. Answers what is still owed a read — "mutation" for an edit or a removal, "live" for
 * a message, which the server places (its thread, its counts) — or null for nothing.
 */

function catchUpHiddenArrivals(): string | null {
  const { mutation, arrived } = hiddenStream.takeArrivals(() => replayLedger.waiting() > 0);
  if (arrived && screens.view() === "discord" && providerFacts.threading() &&
      channelStore.current().channel === String(selectedChannelId()) && channelStore.current().views.has(viewKey())) {
    const area = el("scroll-area");
    const position = channelReadPosition(area);
    if (catchUpHeldView()) {
      const keeps = readFilterKeeps();
      settleAfterRead(timelineRows().messages.filter((message) => !hiddenEmpty(message) && (!hidingRead() || keeps(message))),
        { keepPosition: true, area, ...position });
    } else {
      // Nothing of this view's own, but a reply on Main is one more on its root's chip.
      syncUnreadReplies();
    }
    // ...and in the thread on the right. `#229 desktop-two-column`.
    inOtherColumn(() => {
      const there = el("scroll-area");
      const place = channelReadPosition(there);
      if (catchUpHeldView()) {
        const keeps = readFilterKeeps();
        settleAfterRead(timelineRows().messages.filter((message) => !hiddenEmpty(message) && (!hidingRead() || keeps(message))),
          { keepPosition: true, area: there, ...place });
      }
    });
    saveChannelScope();
  }
  return mutation ? "mutation" : arrived ? "live" : null;
}

// --- resuming an earlier conversation ------------------------------------------------------------
//
// `#46 conversation-replay`. The vendor documents no way to resume a conversation once the socket
// closes — the initiation message and the signed-URL endpoint both take an `agent_id` and neither
// accepts a `conversation_id`. But the EFFECT can be rebuilt from this end: the server holds the
// transcript, so a new call can be handed the earlier exchange as text.
//
// THIS IS A RECONSTRUCTION AND THE SCREEN MUST NEVER SAY OTHERWISE. That is the whole risk of the
// feature, and it is why there are six states here rather than an on/off light: resuming can be
// off, armed, PARTIAL, FAILED, or one of the two that send nothing — an earlier conversation that
// held nothing to replay, and one the budget could not fit a single line of. Each is a different
// sentence, and the last two matter most because they look identical from `included` alone and
// mean opposite things about what the reader said earlier.
//
// A failed replay never aborts the call. It degrades to a fresh one and says so — the point of a
// call is the call, and a lost reconstruction is not a reason to refuse to connect.

function landAbandonedTimelineRead(payload: TimelineResponse, read: TimelineAbandonedRead): void {
  inColumn(read.column, () => {
    const delta = Boolean(read.sent && payload.delta);
    if (delta && ((payload.delta as { more: boolean }).more || !foldTimelineDelta(payload, read.sent, read.view, read.thread))) return;
    if (read.context !== channelContextKey()) {
      if (delta) {
        (channelStore.current().views.get(viewKey(read.view, read.thread)) as { landedHidden: boolean }).landedHidden = true;
        if (read.view === "flat") catchUpFromAll();
        syncUnreadReplies();
        followInOtherColumn();
        saveChannelScope();
      } else {
        foldLateTimelinePage(payload, read);
      }
      return;
    }
    const area = el("scroll-area");
    const at = landedReadPosition(area, true, false);
    holdingScrollTools(() => {
      const messages = applyTimelinePage(payload, false, false, delta, null, read.held);
      settleAfterRead(messages, { area, ...at });
      noteFreshRead(payload);
      saveChannelScope();
      renderScrollTools();
    });
    requestVisibleSummaries();
    followInOtherColumn();
  });
}

/**
 * The poll's timer — armed after each poll, held while the page is hidden and taken up when it is shown,
 * its wait shortened for a stand-in read's quick retry — is web/src/core/sync's (`createChannelPoll`,
 * `#624 voice-page-modularity`); the poll itself (`pollDiscord`) is here.
 */

  return {
    queueDiscordLoad, finishDiscordLoad, currentDiscordLoad, stopDiscordPolling,
    scheduleDiscordPoll, pollDiscord, pauseWhileHidden, resumeLiveStream, resumePolling,
    catchUpHiddenArrivals, landAbandonedTimelineRead,
    dispose() { disposed = true; },
  };
}

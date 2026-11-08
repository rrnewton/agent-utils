// The live stream's frames as the page takes them (`#308 live-reply-in-main`, `#43
// replay-burst-double-read`, `#315 removed-reply-target`; `#624 voice-page-modularity`, moved
// from the legacy script into view/channel; mu-readcore): a frame decoded, a live message
// folded and placed, an edit that names a thread placing its message at once, and what waits
// for a read to settle. The stream owner itself (attach, resume, cursor) is core/sync's; the
// reads asked for here go through the page's own `loadDiscord` name, passed in.

import type { Message } from "../../contract/index.ts";
import { isNoise } from "../../present/rows/index.ts";
import type { ChannelStore } from "../../core/channels/index.ts";
import type { TimelineReadOptions } from "./readcore.ts";

/** One decoded stream frame. */
export interface LiveFrame {
  event: string;
  data?: string;
}

/** What taking live frames asks of the page. */
export interface LiveStreamDependencies {
  channelStore: ChannelStore;
  selectedChannelId(): string;
  providerFacts: { threading(): boolean };
  loadDiscord(options?: TimelineReadOptions): Promise<void>;
  refreshQuietly(fn: () => Promise<void>): () => Promise<void>;
  setStatus(text: string): void;
  noteLiveEvent(channel: unknown, fromTail: boolean): void;
  contractIs(name: string, value: unknown): boolean;
  watchAllAttached(): boolean;
  voiceCanSendText(): boolean;
  relayMode(): string;
  reportUnrelayed(messages: unknown[], reason: string): void;
  refreshServerSettings(): Promise<void>;
  onSettingsFrame(frame: LiveFrame): void;
  hiddenNow(): boolean;
  noteHiddenMessage(): void;
  relayToAgent(message: Message, selfPosted: boolean, replayed?: boolean, arrival?: unknown): void;
  refreshAfterMutation(payload: unknown): void;
  noteChannelReadFailure(error: unknown): void;
  foldLiveMessage(message: Message, live?: boolean, replayed?: boolean, read?: unknown): Message;
  appendChannelRow(message: Message, live?: boolean, folded?: boolean): void;
  appendToColumns(message: Message, live?: boolean, replayed?: boolean): Message;
  renderOutgoingEverywhere(): void;
  noteServerThreadHuesIn(messages: readonly Message[]): void;
  threadOf(message: Message): string | null;
  loadCanon(channel: string): void;
  replayLedger: {
    settle(read: unknown, answer: unknown, channel: string): void;
    attachedAfter(): unknown;
    started(): unknown;
    awaitRead(key: string, channel: string, arrivedAfter: unknown): boolean;
    waiting(): number;
  };
  replaysWaiting(): number;
  readLedgerEpoch(): number;
  buildCopyOn(copy: Message, from: Message): void;
  noteRemoved(canon: unknown, id: string, observedMs: number | null): void;
  liveFollow: { generation(): number; resumed(): boolean };
  inColumn<T>(column: unknown, fn: () => T): T;
  liveEvents: { noteRemoval(channel: string, id?: string): void };
  ownPosts: { removed(id: string): void };
  saveChannelScope(): void;
  entryLandingHolds(): boolean;
  sideShowing(): boolean;
  listStateNewestId(id: string): void;
  scrollToNewest(): void;
  hiddenEmpty(message: Message): boolean;
  setJumpNewest(wanted: boolean, view?: string, arrived?: number): void;
  renderScrollTools(): void;
  requestVisibleSummaries(): void;
  mutationRefresh: { request(): void };
  columns: { main: unknown; side: unknown };
  sideShowing(): boolean;
  el(id: string): HTMLElement;
  idsOf(row: Element | null | undefined): string[];
  renderOutgoingMessages(): void;
  screens: { view(): string; screen(): string };
  followsNewest(area: HTMLElement): boolean;
  noteOutgoingEcho(message: Message): void;
  settleFromServer(message: Message): void;
  noteSelfAuthor(authorId: unknown): void;
  loadState(): { freshness: string; freshAt: number; generation: number };
  renderChannelFreshness(): void;
  channelReads: { busy(): boolean; queued(column: unknown): boolean };
  queueDiscordLoad(options?: TimelineReadOptions): void;
  noteStartsRead(messages: Message[]): void;
  hiddenStream: { noteMessage(): void; noteMutation(): void };
  followReplyStore(message?: Message): void;
  followPlacement(): void;
  inSideThread(message: Message): boolean;
  inColumn<T>(column: unknown, fn: () => T): T;
}

/** The live stream's commands: the legacy names, unchanged. */
export interface LiveStream {
  onStreamFrame(frame: LiveFrame): void;
  receiveLiveMessage(message: Message, selfPosted: boolean, replayed: boolean, fromTail: boolean): void;
  heldMessage(id: string): Message | null;
  placesHeld(message: Message): boolean;
  settleReplays(read: unknown, answer: unknown): void;
  liveFollowers(): { main: boolean; side: boolean };
  refreshAfterLiveMutation(): void;
  dispose(): void;
}

/** Taking live frames over `deps`. */
export function createLiveStream(deps: LiveStreamDependencies): LiveStream {
  const channelStore = deps.channelStore;
  const selectedChannelId = deps.selectedChannelId;
  const providerFacts = deps.providerFacts;
  const loadDiscord = deps.loadDiscord;
  const refreshQuietly = deps.refreshQuietly;
  const setStatus = deps.setStatus;
  const abandonedReads = { noteLiveEvent: (channel: unknown, fromTail: boolean) => deps.noteLiveEvent(channel, fromTail) };
  const VibeTalkContract = { is: (name: string, value: unknown) => deps.contractIs(name, value) };
  const watchAll = { attached: () => deps.watchAllAttached() };
  const voiceCall = () => ({ canSendText: () => deps.voiceCanSendText(), reportUnrelayed: (messages: unknown[], reason: string) => deps.reportUnrelayed(messages, reason), relayToAgent: (message: Message, selfPosted: boolean, replayed?: boolean, arrival?: unknown) => deps.relayToAgent(message, selfPosted, replayed, arrival) });
  const relayMode = deps.relayMode;
  const refreshServerSettings = deps.refreshServerSettings;
  const onSettingsFrame = deps.onSettingsFrame;
  const el = deps.el;
  const idsOf = deps.idsOf;
  const renderOutgoingMessages = deps.renderOutgoingMessages;
  const screens = deps.screens;
  const followsNewest = deps.followsNewest;
  const noteOutgoingEcho = deps.noteOutgoingEcho;
  const settleFromServer = deps.settleFromServer;
  const noteSelfAuthor = deps.noteSelfAuthor;
  const loadState = deps.loadState;
  const renderChannelFreshness = deps.renderChannelFreshness;
  const channelReads = deps.channelReads;
  const columns = deps.columns;
  const queueDiscordLoad = deps.queueDiscordLoad;
  const noteStartsRead = deps.noteStartsRead;
  const hiddenStream = deps.hiddenStream;
  const followReplyStore = deps.followReplyStore;
  const followPlacement = deps.followPlacement;
  const inSideThread = deps.inSideThread;
  const inColumn = deps.inColumn;
  const liveEvents = deps.liveEvents;
  const ownPosts = deps.ownPosts;
  const saveChannelScope = deps.saveChannelScope;
  const entryLandingHolds = deps.entryLandingHolds;
  const sideShowing = deps.sideShowing;
  const listState = () => ({ set newestId(id: string) { deps.listStateNewestId(id); } });
  const scrollToNewest = deps.scrollToNewest;
  const hiddenEmpty = deps.hiddenEmpty;
  const setJumpNewest = deps.setJumpNewest;
  const renderScrollTools = deps.renderScrollTools;
  const requestVisibleSummaries = deps.requestVisibleSummaries;
  const mutationRefresh = deps.mutationRefresh;
  const foldLiveMessage = deps.foldLiveMessage;
  const appendChannelRow = deps.appendChannelRow;
  const appendToColumns = deps.appendToColumns;
  const renderOutgoingEverywhere = deps.renderOutgoingEverywhere;
  const threadOf = deps.threadOf;
  const loadCanon = deps.loadCanon;
  const replayLedger = deps.replayLedger;
  const buildCopyOn = deps.buildCopyOn;
  const liveFollow = deps.liveFollow;
  const noteRemoved = deps.noteRemoved;
  let disposed = false;
  void disposed;

function onStreamFrame(frame: LiveFrame): void {
  if (frame.event === "reset") {
    // The server is saying this subscriber fell further behind than its replay tail, so there IS a
    // gap and it cannot be filled from the stream. Re-read the channel rather than carrying on
    // short: a missing message the page does not know about is the one failure mode a live feed
    // must not have. The cursor is dropped with it (`liveFollow`) — resuming from an id on the far
    // side of a gap would ask the server to replay what it has already said it cannot.
    // `#337 voice-relay-silent`. The re-read puts the channel right; whether a call heard what fell
    // into the gap cannot be known, so it is said — unless the stream of every channel, which
    // carries this channel too, is what the call is hearing from.
    if (!watchAll.attached() && voiceCall().canSendText() && relayMode() !== "off") voiceCall().reportUnrelayed([], "gap");
    setStatus("the live feed fell behind — re-reading the channel.");
    refreshQuietly(() => loadDiscord({ keepPosition: true, reason: "reset" }))();
    // `#457 settings-server-side`. The same gap could hide a settings frame: re-read.
    refreshServerSettings().catch(() => {});
    return;
  }
  // `#457 settings-server-side`. A settings change from another device, carried on this
  // stream between messages: it is about no channel and carries no cursor of its own.
  if (frame.event === "settings") {
    onSettingsFrame(frame);
    return;
  }
  if (!frame.data) {
    return;
  }
  let payload = null;
  try {
    payload = JSON.parse(frame.data);
  } catch (_error) {
    return; // a frame this page cannot read is not a reason to tear the stream down.
  }
  // `remote-ops-audit` W7. Whatever it is about, an event for a channel says the service is answering
  // for it (`noteLiveEvent`), unless it is the replay tail's.
  if (payload && typeof payload === "object" &&
      ["message", "message_update", "message_delete"].includes(frame.event)) {
    const about = payload.message && typeof payload.message === "object" ? payload.message.channel_id : payload.channel_id;
    abandonedReads.noteLiveEvent(about, payload.from_tail === true);
  }
  if (frame.event === "message_update" || frame.event === "message_delete") {
    // `#308 live-reply-in-main`. An edit is a copy too: one that names the thread of a message the
    // store holds without one places it at once, whatever the read below brings.
    if (providerFacts.threading() && frame.event === "message_update" && VibeTalkContract.is("LiveMessageEvent", payload) &&
        placesHeld(payload.message)) {
      placeFromEdit(payload.message);
    }
    // ...and a removal says the message is gone upstream, whatever the read below brings: no answer
    // sent before it may put the message back.
    if (providerFacts.threading() && frame.event === "message_delete" && VibeTalkContract.is("LiveDeleteEvent", payload)) {
      noteLiveRemoval(payload.channel_id, payload.message_id,
        typeof payload.observed_ms === "number" ? payload.observed_ms : null);
    }
    // An edit or removal out of the replay tail is on every page read begun after the attach, so
    // it waits for such a read rather than making its own. The page's copy of the message says
    // nothing about which edit it has seen, so no page can answer for it sooner.
    // `#43 replay-burst-double-read`.
    if (providerFacts.threading() && payload && payload.from_tail === true) {
      const message = payload.message || {};
      const id = frame.event === "message_update" ? message.id : payload.message_id;
      const channel = frame.event === "message_update" ? message.channel_id : payload.channel_id;
      awaitReplayRead(`${frame.event}:${id}`, String(channel), replayLedger.attachedAfter());
      return;
    }
    // A fetched row may combine several messages and the update may change whether they still
    // combine. Re-reading the authoritative page is safer than trying to patch that derived DOM
    // structure in place, and `keepPosition` preserves where the reader was.
    refreshAfterLiveMutation();
    return;
  }
  if (frame.event !== "message") {
    return;
  }
  // Checked against the contract before any of it is believed. A malformed event cannot be
  // rendered, but it still says the channel changed, so the answer is the authoritative re-read an
  // edit gets — never silently dropping what may be a real message.
  if (!VibeTalkContract.is("LiveMessageEvent", payload)) {
    refreshAfterLiveMutation();
    return;
  }
  receiveLiveMessage(payload.message, payload.self_posted, payload.replayed, payload.from_tail);
  // `#468 send-dedupe`. This server's copy of a post it made for one of this page's keys.
  if (payload.self_posted && typeof payload.client_key === "string") {
    noteOutgoingEcho(payload.client_key);
    settleFromServer(payload.client_key);
  }
}

// Whether a re-read for live edits is owed or running, and the loop that waits out a read in flight and
// reads again for edits arriving during it: web/src/core/sync/coalesce.ts (`#624 voice-page-modularity`).

function receiveLiveMessage(message: Message, selfPosted: boolean, replayed: boolean, fromTail: boolean): void {
  // `#337 voice-relay-silent`. How it came, for whether a call that may have missed it hears it.
  const arrival = { fromTail: fromTail === true, resumed: liveFollow.resumed() };
  // `#466 reply-promotes-thread`. Not a read's observation, which retires a sent receipt at once: the
  // read the stream's copy asks for may be answered from older state. The copy goes into the store,
  // which keeps the page's own post against such a read (`ownPostStands`), and the receipt goes after
  // its showing (`sentReceiptRetires`, `#500 sent-receipts-pile-up`).
  // The other free way to learn which account is ours: the server marks what IT posted, so the
  // author of a self-posted message is this bridge by construction. Done before the channel guard
  // below, because the fact is true regardless of which channel the reader happens to be looking
  // at. `#85 voice-desktop-review`.
  if (selfPosted) {
    noteSelfAuthor(message.author_id);
  }
  if (String(message.channel_id) !== String(selectedChannelId())) {
    renderOutgoingEverywhere();
    return;
  }
  // A live message for this channel is proof the list is current as of now. Only a list already
  // current moves its stamp: saved or failed rows stay labelled until a read replaces them.
  if (!replayed && loadState().freshness === "fresh" && loadState().freshAt) {
    loadState().freshAt = Date.now();
    renderChannelFreshness();
  }
  if (!providerFacts.threading() && channelReads.busy()) {
    // This event is newer evidence than a read already on the wire. Invalidate that snapshot
    // even when the event's ID is already visible from a POST acknowledgement; otherwise it
    // could erase the row after this event retired its durable send receipt.
    ++loadState().generation;
    if (!channelReads.queued(columns.main)) queueDiscordLoad({ keepPosition: true, readAs: "live" });
  }
  if (providerFacts.threading()) {
    // A replayed copy is from when it was published, so the store's copy is at least as new — its
    // reply count, its edits — and is the one kept. Asked before the row is folded in, which would
    // make every message look already held. A held message is already in the view drawn from the
    // store, or deliberately not when the to-do filter hides it, so it is not appended again.
    const held = replayed ? heldMessage(message.id) : null;
    // `#308 live-reply-in-main`. A copy naming the thread of a message the store holds without one
    // places it, replayed or not: it leaves Main for its thread now, not at the next read that can say.
    // A replayed copy gives the store its thread record and nothing else: the rest of it is older.
    const places = placesHeld(message);
    // `#549 own-read-stored`. One of his, new here, is drawn as it starts: read, when he said so.
    if (!replayed) noteStartsRead([message]);
    if (document.hidden) {
      // `#226 page-energy-profile`. Into the store, for coming back to draw and read behind; no row
      // and no read for a page nobody can see. A call's agent is told as ever.
      if (!held || places) {
        if (held) takeThreadRecord(message);
        else foldLiveMessage(message, true, replayed);
        hiddenStream.noteMessage();
        followReplyStore();
      }
      renderOutgoingEverywhere();
      voiceCall().relayToAgent(message, selfPosted, replayed, arrival);
      return;
    }
    // `#185 reply-new-thread`. An open Reply follows the store as the rows do, before the read below
    // and whether or not it lands: a reply in the thread it offers to start turns the row's tag into
    // Thread(1) at once, and the offer has to go with it.
    if (!held) {
      // `#280 scroll-stability`. Whether each column's reader is on its newest line is asked BEFORE
      // the row goes in, which is what changes the answer — as the channel without threads below
      // asks it, and as a send does (`#229`). Asked after, by the read below, a reader on the newest
      // line was a row short of it by then and was never followed: the message arrived out of sight
      // under them, and the jump offered to take them to it.
      const pinned = liveFollowers();
      appendToColumns(message, true, replayed);
      followLiveArrival(pinned);
      followReplyStore();
    } else if (places) {
      takeThreadRecord(message);
    }
    if (places) followPlacement();
    renderOutgoingEverywhere();
    voiceCall().relayToAgent(message, selfPosted, replayed, arrival);
    // The server owns thread membership, counts and activity ordering. Re-read the active
    // context instead of dropping a reply from another thread into the visible conversation.
    // `#229 desktop-two-column`: a reply in the thread on the right is read as that thread, which
    // brings its root's count with it; anything else as Main. One read, never one per column.
    if (!replayed) {
      const column = inSideThread(heldMessage(String(message.id)) || message) ? columns.side : columns.main;
      refreshQuietly(() => inColumn(column, () => loadDiscord({ keepPosition: true, reason: "live" })))();
    } else if (!held) {
      awaitReplayRead(String(message.id), String(message.channel_id),
        fromTail ? replayLedger.attachedAfter() : replayLedger.started());
    }
    return;
  }
  const list = el("discord-log");
  const id = String(message.id);
  // Against EVERY id on screen: a message already combined into a row is on screen, and a dedupe
  // that only looked at row identities would add a second copy of it.
  const already = [...list.children].some((li) => idsOf(li).includes(id));
  if (already) {
    renderOutgoingMessages();
    return;
  }
  // The SAME constructor the fetched rows go through, so the untrusted-content boundary is
  // identical on both paths. A live feed is not a reason to relax element construction.
  const area = el("scroll-area");
  // Not while a filter change holds a message in place: nothing but the reader moves it. `#243`.
  const wasAtNewest = screens.view() === "discord" && followsNewest(area);
  // `#549 own-read-stored`. One of his is drawn as it starts: read, when he said so.
  if (!replayed) noteStartsRead([message]);
  appendChannelRow(message, true);
  listState().newestId = id;
  if (wasAtNewest) {
    scrollToNewest();
  } else if (!isNoise(message) && !hiddenEmpty(message)) {
    // Somewhere else in the history, or looking at the call. Offer the jump; never take it.
    // Never for a placeholder the server reads automatically (`#196 auto-read-noise`): an offer to
    // go and look at "Working…" is the interruption the owner asked to be rid of.
    setJumpNewest(true, "discord", 1);
  }
  renderScrollTools();
  // A row that arrived because the SERVER said so is a row like any other: if it is long and the
  // reader is looking at it, it gets a summary. `#49 cached-summaries`.
  requestVisibleSummaries();
  voiceCall().relayToAgent(message, selfPosted, replayed, arrival);
}

/**
 * Which columns' readers follow a message arriving now: each one on the newest line of its list, on
 * the main screen in the channel's view. Not one a filter change holds in place (`followsNewest`),
 * nor one just landed on a thread's first unread who has not moved since, whom the landing outranks
 * (`entryLandingHolds`). Asked BEFORE the message goes in. `#280 scroll-stability`.
 *
 * @returns {{main: boolean, side: boolean}}
 */

function refreshAfterLiveMutation(): void {
  // `#226 page-energy-profile`. Not for a hidden page: the read that coming back makes answers it.
  if (providerFacts.threading() && document.hidden) {
    hiddenStream.noteMutation();
    return;
  }
  mutationRefresh.request();
}

/**
 * A message has arrived. Put it on screen, then decide whether the agent hears about it.
 *
 * De-duplicated against the rows that are actually rendered rather than against a set this file
 * keeps: `loadDiscord` replaces every child of the log, so a private set would go stale exactly
 * when it mattered — after a refresh, which is the moment a replayed message is most likely to
 * arrive twice.
 */

function heldMessage(id: string): Message | null {
  const channel = String(selectedChannelId());
  if (channelStore.current().channel !== channel) loadCanon(channel);
  return channelStore.current().messages.find((held) => String(held.id) === String(id)) || null;
}

/**
 * Whether `message`, a copy from the stream, names the thread of a message the selected channel's
 * store holds without one: the copy that places it. `#308 live-reply-in-main`.
 */

function placesHeld(message: Message): boolean {
  if (!threadOf(message) || String(message.channel_id) !== String(selectedChannelId())) return false;
  const held = heldMessage(message.id);
  return held !== null && !threadOf(held);
}

/**
 * An edit from the stream whose copy names the thread of a message the store holds without one: that
 * record is the store's from now on, and the message moves into its thread (`followPlacement`). Only
 * the record. What the message says is still the read's to bring, as for every edit. `#308`.
 *
 * @param {any} message
 */

function placeFromEdit(message: Message): void {
  takeThreadRecord(message);
  if (document.hidden) hiddenStream.noteMessage();
  else followPlacement();
}

/**
 * The thread record of `message`, a copy from the stream that places a message the store holds
 * (`placesHeld`), into the store's copy, which keeps everything else it has: an edit's copy says
 * nothing of which edits the store has seen, and a replayed copy is from when the message was
 * published. The message is placed. `#308 live-reply-in-main`.
 *
 * @param {any} message
 */

function takeThreadRecord(message: Message): void {
  const id = String(message.id);
  channelStore.current().messages = channelStore.current().messages.map((held) => {
    if (String(held.id) !== id) return held;
    const placed = { ...held, thread: message.thread };
    // As new as the store's copy, the record aside (`copyBuiltOn`, `#310 stale-live-read-flicker`).
    buildCopyOn(placed, held);
    return placed;
  });
  channelStore.current().unplaced.delete(id);
}

/**
 * A removal from the stream, of message `id` in `channel`: the message is gone upstream. The store
 * keeps it until the read the removal asks for lands, as for every removal; but it is no longer left
 * unplaced (`forgetUnplaced`), so no answer of All sent before the removal puts it back as a thread
 * reply (`placeFromPage`), and a placing read out as it arrived is not folded (`landPlacingRead`).
 * `#308 live-reply-in-main`.
 *
 * @param {string} channel
 * @param {string} id
 * @param {number | null} [observedMs] when the server observed the removal (`removalsObserved`)
 */

function noteLiveRemoval(channel: string, id: string, observedMs: number | null = null): void {
  liveEvents.noteRemoval(String(channel));
  // `#466 reply-promotes-thread`. Heard of in any channel: a post of this page's removed upstream is no
  // longer kept against reads that do not carry it (`ownPostStands`).
  // `#500 sent-receipts-pile-up`: nor on the device, whichever of its windows sent it.
  ownPosts.removed(String(id));
  if (channelStore.current().channel !== String(channel)) return;
  noteRemoved(channelStore.current(), String(id), observedMs);
  // `#315 removed-reply-target`. A reply open on it says so now, not when the read behind this lands,
  // and so does one after a reload before that read has landed.
  followReplyStore();
  saveChannelScope();
}

/**
 * One read merges a replayed event the store cannot answer for, and the rest of its burst.
 *
 * The server had an event from the replay tail before the stream attached, so any read begun after
 * the attach has seen it, even a reply in a thread the view does not show. The adapter's catch-up
 * is replayed too but may arrive after every read the page has made, so it is only known to be
 * seen by a read begun after it arrived. `arrivedAfter` is the read count at that point. When such
 * a read has landed there is nothing to do. Otherwise the event waits under `key` for the next read
 * to land, and `settleReplays` decides then; with none on the wire and the channel on screen, it
 * starts one. The voice pane starts none: entering the channel reads it, and that read answers for
 * everything waiting. A LIVE frame is not asked about: it is newer than any read on the wire, so it
 * always re-reads, as `refreshAfterLiveMutation` explains.
 *
 * @param {string} key the id of a new message, which a landed page holding it answers for; an
 *   edit or removal is keyed by its event and id, which no page's message id can match, because
 *   a page's copy says nothing about which edit it has seen
 * @param {string} channel
 * @param {number} arrivedAfter
 */

function awaitReplayRead(key: string, channel: string, arrivedAfter: unknown): void {
  // `#203 incremental-refresh`. An edit or removal is answered only by a read that carries them:
  // a newest page, or a complete delta. A delta of additions says nothing about either.
  if (replayLedger.awaitRead(key, channel, arrivedAfter)) return;
  // `#226 page-energy-profile`. A hidden page leaves it waiting for the read coming back makes.
  if (!channelReads.busy() && screens.view() === "discord" && !document.hidden) {
    refreshQuietly(() => loadDiscord({ keepPosition: true, reason: "replay" }))();
  }
}

/**
 * Timeline read number `read` has landed with `page`, or with null if it failed: forget the
 * replayed events it answers for, and queue one more read if any arrived while it was out and its
 * page did not have them. Asked of the page, not the store, which the frame itself has already put
 * the message in. A read that started after an event arrived answers for it even when its page
 * lacks it — a reply in another thread is not on the main view's page — so no event can cost more
 * than that one read. A read that failed answers for nothing and queues nothing: the event waits
 * for the next read, the poll's or the one entering the channel makes, so a server that keeps
 * failing is not asked again for every frame.
 *
 * @param {number} read
 * @param {{messages: VibeTalk.Message[], threads: VibeTalk.ThreadSummary[],
 *          delta: {complete: boolean} | null} | null} page what the read's landed pages carried,
 *   and — when every one of them was a delta — whether all of those were complete
 */

function settleReplays(read: unknown, page: unknown): void {
  replayLedger.settle(read, page, String(selectedChannelId()));
  // The stream's replay is answered by a read of the left column's view. `#229 desktop-two-column`.
  if (page && replayLedger.waiting() && screens.view() === "discord" && !channelReads.queued(columns.main)) {
    inColumn(columns.main, () => queueDiscordLoad({ keepPosition: true, reason: "replay" }));
  }
}

// `canSendText` and `sendClientEvent` used to be defined a second time HERE, shadowing the pair
// near `startCapture`. They are declared once now, where the long note about the send path already
// lived; the relay below calls exactly the same function it was already getting.

/**
 * The name the channel picker shows for a snowflake, or the snowflake itself for one no longer on
 * the list (an outbox entry may outlive its channel). From the list, not the picker's children:
 * those are service groups once there are several. `#355 provider-grouped-channels`.
 */

function liveFollowers() {
  const follows = () => screens.screen() === "main" && screens.view() === "discord" &&
    !entryLandingHolds() && followsNewest(el("scroll-area"));
  return {
    main: inColumn(columns.main, follows),
    side: sideShowing() && inColumn(columns.side, follows),
  };
}

/**
 * Keep each reader `liveFollowers` found on the newest line on it, now that the message is in. The
 * rest are not moved at all; the read behind the message offers them the jump (`settleAfterRead`).
 *
 * @param {{main: boolean, side: boolean}} pinned
 */

function followLiveArrival(pinned: { main: boolean; side: boolean }): void {
  if (pinned.main) inColumn(columns.main, scrollToNewest);
  if (pinned.side && sideShowing()) inColumn(columns.side, scrollToNewest);
}

/**
 * The store's copy of a message in the selected channel, or null.
 *
 * `#43 replay-burst-double-read`. A replayed message the store holds needs no read. The replayed
 * copy is the message as first published, and every later change to it reaches the store as an
 * edit event or a read, so the store's copy is at least as new. The stream replays its tail in a
 * burst on every reload, so re-reading for each frame cost a warm reload two identical reads of the
 * newest page — one of them on the voice pane, where nothing refreshes a channel nobody is looking
 * at.
 */

  return {
    onStreamFrame, receiveLiveMessage, heldMessage, placesHeld, settleReplays, liveFollowers, refreshAfterLiveMutation,
    dispose() { disposed = true; },
  };
}

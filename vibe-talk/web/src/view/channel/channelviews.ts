// Channel views, threads and the channel composer (`#220 view-switch-instant`, `#221
// read-modes`, `#222 unread-replies`, `#229 desktop-two-column`, `#252 thread-button-state`,
// `#281 view-keys`, `#304 back-gesture-in-app`, `#306 done-stale-across-views`, `#333
// page-live-reads-remainder`, `#396 thread-color-memory`, `#427 server-thread-hues`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel for the plan's
// step 17): where the channel stands, the record of a thread, which view is shown and how
// the switch between them is drawn without a blank list, and what a held view missed. The
// store, the views' rows and the reads are handed in; the words are the page's.

import type { Message, ThreadSummary } from "../../contract/index.ts";
import { createThreadHues } from "./hues.ts";
import type { ThreadHues } from "./hues.ts";
import type { EntryState } from "./threadentry.ts";

/** The selection of the channel on screen, as the views read and change it. */
export interface ViewSelection {
  view: string;
  threadId: string | null;
  thread: ThreadSummary | null;
  origin: string | null;
}

/** A message's thread record, as held (`heldThreadRecord`). */
export interface ThreadRecordLike {
  id: string;
  root_message_id?: string;
  is_root?: boolean;
  reply_count?: number | null;
  reply_count_exact?: boolean | null;
}

/** Where the reader of the left column is (`readerPlace`). */
export interface ReaderPlaceResult {
  pinned: boolean;
  seen: readonly { ids: readonly string[]; top: number }[];
  card: number | null;
  cardSeen: boolean;
}

/** A view held in this page (`channelContexts`). */
export interface HeldViewContext {
  messages: Message[];
  threads: ThreadSummary[];
  selected: ThreadSummary | null;
  rows: HTMLElement[];
  cards: HTMLElement[];
  seam: HTMLElement[];
  more: boolean;
  cursor: unknown;
  newest: string | null;
  position: unknown;
  top: number;
  freshness: string;
  freshAt: number;
  dismissed: Set<string>;
  readMode: string;
}

/** What the channel views are handed by the page. */
export interface ChannelViewsDependencies {
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  columnSide(): boolean;
  channelCanon(): { channel: string; threads: ThreadSummary[]; messages: Message[]; views: Map<string, { live?: boolean; at?: number; dense?: number; top?: number; fromAll?: boolean; landedHidden?: boolean; notice?: string; cursor?: unknown; newest?: unknown; fullAt?: number }>; dismissed: Set<string> };
  view(): string;
  threading(): boolean;
  entryState(): EntryState;
  selection(): ViewSelection;
  readMode(): string;
  transcriptShown(): unknown;
  selectedChannelId(): unknown;
  heldMessage(id: string): Message | null;
  providerOfChannel(channel: string): { replies_in_thread?: boolean } | null;
  providersSize(): number;
  repliesInThreadCapability(): boolean;
  threadOf(message: unknown): string | null;
  rowMessages(row: HTMLElement): Message[];
  renderSideColumn(): void;
  renderThreadSelect(): void;
  renderThreadHeading(): void;
  followReplyStore(): void;
  markListView(): void;
  paintAllThreadFocus(): void;
  pinsView(): { only: boolean };
  renderOutgoingMessages(): void;
  syncCanonDismissed(): void;
  timelineRows(): { messages: Message[]; threads: ThreadSummary[] };
  olderPages(): { moreAbove: boolean; cursor: unknown };
  captureScroll(): unknown;
  loadState(): { freshAt: number; freshness: string; generation: number };
  doneState(): { archived: Set<string> };
  idsOf(row: Element): string[];
  shownRowFor(row: HTMLElement | undefined): HTMLElement | null;
  atBottom(area: HTMLElement): boolean;
  noteDeepLinkReaderMove(): void;
  putDockedReplyAway(): void;
  replyDocked(): boolean;
  replyColumn(): unknown;
  activeColumn(): unknown;
  putRepliesBack(): void;
  dropReplyArrowTap(): void;
  rememberChannelDraft(): void;
  setPinnedOnly(only: boolean, kept: boolean): void;
  viewOnScreen(): { view: string; thread: string | null; summary: unknown; origin: string | null } | null;
  saveUiState(): void;
  scheduleBackSync(): void;
  voiceCall(): { renderControls(): void };
  threadOutrunsStore(threadId: string | null, summary: ThreadSummary | null): boolean;
  heldViewRows(): Message[];
  drawProjection(): boolean;
  drawHeldRows(rows: readonly Message[], freshAt: number, freshness: string): boolean;
  setChannelFreshness(state: string): void;
  saveChannelScope(): void;
  walkCursorOwed(): boolean;
  maybeLoadOlder(): void;
  allReachesThread(canon: unknown, threadId: string | null, summary: ThreadSummary | null): boolean;
  storeLacksReplies(threadId: string | null, summary: ThreadSummary | null): boolean;
  projectView(): Message[] | ThreadSummary[] | null;
  channelReadPosition(area: HTMLElement): object;
  readFilterKeeps(): (message: Message) => boolean;
  settleAfterRead(messages: readonly Message[], options: unknown): void;
  hiddenEmpty(message: Message): boolean;
  hidingRead(): boolean;
  renderChannelLoading(busy: boolean, quiet?: boolean): void;
  restoreChannelComposer(): void;
  renderOlderControl(): void;
  restoreScroll(position: unknown): void;
  renderCachedTimeline(): void;
  syncUnreadReplies(): void;
  renderChannelFreshness(): void;
  payThreadEntry(): void;
  rowsMissDone(): boolean;
  scrollToNewest(): void;
  renderJumpNewest(): void;
  threadCards(threads: readonly ThreadSummary[]): HTMLElement[];
  loadDiscordTimeline(options: unknown): Promise<void>;
  api(path: string, options: unknown): Promise<unknown>;
  deviceStorage(): unknown;
  tokenScope(): string;
  channelRecord(id: string): unknown;
  timeOfRecord(record: unknown): number;
  threadOfMessage(message: unknown): string | null;
  layoutShown(): { columns(): boolean; threadList(): boolean; restoring(): boolean };
  stopReading(): void;
  setReadingMode(on: boolean): void;
  readRunsForget(key: string): void;
  viewKey(view?: string, threadId?: string | null): string;
  rememberView(view: { view: string; thread: string | null; summary: unknown; origin: string | null }): void;
  forgetChosenView(test: (channel: string) => boolean): void;
  settleUiState(): void;
  payReadCursor(): void;
  readsBusy(): boolean;
  readContext(): string;
  channelContexts(): Map<string, HeldViewContext>;
  listState(): { newestId: string | null };
  replyDocked(): boolean;
  replyColumn(): unknown;
  activeColumn(): unknown;
}

/** The channel views, as the page calls them. */
export interface ChannelViews {
  readonly threadHues: ThreadHues;
  channelContextKey(): string;
  heldThreadRecord(message: Message, stored?: unknown): ThreadRecordLike | null;
  channelRepliesInThread(channel: string): boolean;
  threadReplyCount(id: string, message: Message | null): number | null;
  threadReplies(id: string, message: Message | null): { count: number | null; exact: boolean | undefined };
  threadForMessageId(id: string): string | null;
  withThreadQuery(path: string, threadId: string | null): string;
  restoreThreadHues(channel: string): void;
  noteServerThreadHuesIn(messages: readonly Message[], summaries: readonly ThreadSummary[], channel: string, provisional: boolean): void;
  seedServerThreadHues(channel: string): unknown;
  threadHue(id: string): string;
  threadCount(count: number | null, exact: boolean | undefined): string;
  renderChannelNavigation(): void;
  renderColumnLists(): void;
  rememberChannelContext(): void;
  readerPlace(): ReaderPlaceResult;
  channelCard(): HTMLElement | null;
  keepReaderPlace(place: ReaderPlaceResult): boolean;
  readBehindSwitch(): Promise<void>;
  changeChannelView(view: string, threadId?: string | null, summary?: ThreadSummary | null, origin?: string | null, before?: ReaderPlaceResult | null): Promise<void>;
  inheritAllCover(view: string, threadId?: string | null, summary?: ThreadSummary | null): boolean;
  catchUpHeldView(words?: boolean): boolean;
  catchUpFromAll(): void;
}

/** Make the channel views from the page's store, rows and reads. */
export function createChannelViews(deps: ChannelViewsDependencies): ChannelViews {
  // --- channel views, threads and the channel composer ------------------------------------------

  function channelContextKey(): string {
    const key = [deps.selectedChannelId(), deps.selection().view, deps.selection().threadId];
    // `#229 desktop-two-column`. The right column's views are its own: a thread shown there and the
    // same thread shown in one column are two lists of rows, never one held context moved between them.
    return JSON.stringify(deps.columnSide() ? [...key, "side"] : key);
  }


  /**
   * The thread record the page holds for a message: the store's copy's, the row's own, or — for a
   * thread root whose copy arrived without one — the summary of the thread it starts. The store
   * first: a row keeps the objects it was drawn from, and the store may have learnt more about them
   * since, a root's reply count above all. `stored` is the store's copy, for a caller that has looked
   * it up already (`replyLinks`, which asks after every message in the list).
   */
  function heldThreadRecord(message: Message, stored: unknown = deps.heldMessage(String(message.id))): ThreadRecordLike | null {
    const id = String(message.id);
    for (const copy of [stored, message]) {
      if (deps.threadOf(copy)) return (copy as Message).thread as ThreadRecordLike;
    }
    const summary = deps.channelCanon().threads.find((held) => held.root && String(held.root.id) === id);
    return summary ? {
      id: String(summary.id), root_message_id: id, is_root: true,
      reply_count: summary.reply_count, reply_count_exact: summary.reply_count_exact,
    } : null;
  }


  /**
   * Whether a reply to a main-channel message of `channel` is posted in that message's thread, as
   * its provider's server says (`replies_in_thread`): Slack, which has no reply pointer, so its
   * server threads every reply. `#252 thread-button-state`. With several providers each channel
   * answers for its own, as the send service's posts-once does.
   */
  function channelRepliesInThread(channel: string): boolean {
    if (deps.providersSize()) {
      const provider = deps.providerOfChannel(channel);
      return Boolean(provider && provider.replies_in_thread === true);
    }
    return deps.repliesInThreadCapability();
  }

  /** How many replies a thread has, from its summary or the message's own thread record. */
  function threadReplyCount(id: string, message: Message | null): number | null {
    return threadReplies(id, message).count;
  }

  /** `threadReplyCount`, with whether that count is exact. Null when neither record says. */
  function threadReplies(id: string, message: Message | null): { count: number | null; exact: boolean | undefined } {
    const summary = deps.channelCanon().threads.find((held) => String(held.id) === String(id));
    if (summary && typeof summary.reply_count === "number") {
      return { count: summary.reply_count, exact: summary.reply_count_exact };
    }
    const own = message && message.thread;
    if (own && typeof own.reply_count === "number") return { count: own.reply_count, exact: own.reply_count_exact };
    return { count: null, exact: false };
  }

  function threadForMessageId(id: string): string | null {
    for (const row of [...deps.el("discord-log").children, ...deps.el("pinned-log").children] as HTMLElement[]) {
      const found = deps.rowMessages(row).find((message) => String(message.id) === String(id));
      if (found) return deps.threadOf(found);
    }
    return deps.selection().threadId;
  }

  function withThreadQuery(path: string, threadId: string | null): string {
    return threadId ? `${path}${path.includes("?") ? "&" : "?"}thread_id=${encodeURIComponent(threadId)}` : path;
  }

  // `#396 thread-color-memory`. A thread keeps its colour on this device across a reload and a trip to
  // another channel and back. The colours are handed out in the order threads are first drawn, so
  // held only in memory they came back different every time a reload — or a channel change, which
  // starts the channel afresh — drew the threads in another order. Each channel's colours are now
  // kept in localStorage and put back before its first thread is drawn; a thread seen for the first
  // time still gets the widest arc left, measured against the ones put back.
  //
  // Bounded, the least recently drawn going first: every colour remembered is an arc a new thread
  // cannot have, so a channel that remembered every thread it ever had would crowd its new ones
  // into slivers. A browser that refuses storage colours threads exactly as before, for the session.
  //
  // `#427 server-thread-hues`. The server now decides each thread's colour, once, as it first
  // learns of the thread, and every message's thread record and every thread summary carries it
  // (`hue`). This page uses the server's colour whenever one arrives — the same colour on every
  // device — and the algorithm (`ThreadHues.hue`), `#396`'s memory and `#414`'s spread, is now only the
  // fallback for a thread the server has not coloured: an older server, a read-token page meeting
  // a thread nobody with the write token has read yet, a deployment with no store. An assigned
  // thread is never recoloured here: a server hue replaces a fallback one the moment it arrives,
  // and after that nothing on this page moves it.
  //
  // The colours of the channel on screen, the server's colours, the device's bounded memory of every
  // channel's colours, a colour handed out for a thread the server has not coloured, and the one offer per
  // load of the remembered colours: one owner in web/src/view/channel/hues.ts (`#624 voice-page-modularity`).
  const threadHues = createThreadHues({
    storage: deps.deviceStorage() as never,
    now: () => Date.now(),
    channelRecord: (id) => deps.channelRecord(id) as never,
    selected: () => String(deps.selectedChannelId()),
    writeToken: () => deps.tokenScope() === "write",
    held: () => deps.channelCanon() as never,
    timeOf: (record) => deps.timeOfRecord(record),
    threadOf: (message) => deps.threadOfMessage(message),
    post: (channel, hues) => deps.api(`/api/v1/channels/${encodeURIComponent(channel)}/thread-hues`,
      { method: "POST", body: { hues } }),
  });

  /** Put back `channel`'s remembered colours, after storing what was drawn of the last one. */
  function restoreThreadHues(channel: string): void {
    threadHues.restore(channel);
  }

  /** Take the server's colours off a batch of messages and thread summaries, roots included. */
  function noteServerThreadHuesIn(messages: readonly Message[], summaries: readonly ThreadSummary[], channel: string, provisional: boolean): void {
    threadHues.noteServerIn(messages, summaries, channel, provisional);
  }

  /**
   * Offer this browser's remembered colours for `channel` to the server, once — the migration that lets
   * the colours drawn today survive `#427`. Write token only. Issued at sign-in and on a channel change,
   * before either flow sends a read or opens the stream — deliberately NOT awaited inside a read, whose
   * counting and column bookkeeping are sequenced to the breath a read is sent in (`apiDecoded`,
   * `abandoned.number`) and broke under a wait inserted there. Never fails its caller.
   */
  function seedServerThreadHues(channel: string): unknown {
    return threadHues.seed(channel);
  }

  /**
   * The colour thread `id` is drawn in (`ThreadHues.hue`): a hue in degrees, which the rows hand straight to
   * `style.setProperty("--thread-hue", ...)` as they always have.
   *
   * @returns {any}
   */
  function threadHue(id: string): string {
    return String(threadHues.hue(id));
  }

  function threadCount(count: number | null, exact: boolean | undefined): string {
    if (typeof count !== "number") return "Replies";
    return `${exact === false ? "about " : ""}${count} ${count === 1 ? "reply" : "replies"}`;
  }

  function renderChannelNavigation(): void {
    // `#229 desktop-two-column`. The right column names its thread in a heading of its own; what is
    // this pass's there is which of its lists is up and where its box posts.
    if (deps.columnSide()) {
      renderColumnLists();
      deps.renderSideColumn();
      return;
    }
    const inChannel = deps.view() === "discord";
    // `gchat-thread-selector`. Choosing Main, All, or a thread is the bar's thread picker now, so the
    // tabs that used to sit over the list stay hidden; the heading still names an open thread.
    deps.el("channel-navigation").hidden = !inChannel || deps.selection().view !== "thread";
    deps.el("channel-view-tabs").hidden = true;
    deps.renderThreadSelect();
    deps.el("thread-heading").hidden = deps.selection().view !== "thread";
    for (const view of ["main", "threads", "flat"]) {
      deps.el(`channel-view-${view}`).setAttribute("aria-pressed", deps.selection().view === view ? "true" : "false");
    }
    deps.renderThreadHeading();
    // A reply open in the stream says where it goes from the same store, so a read that places the
    // message being answered says so under its box (`followReplyStore`).
    deps.followReplyStore();
    renderColumnLists();
  }

  /** Which of the column's lists is up, and where its box posts. */
  function renderColumnLists(): void {
    deps.markListView();
    // `#299 all-thread-strip`. A view put back from rows this page held is drawn with no pass over them:
    // All comes back to the thread selected in it, and any other view to none.
    deps.paintAllThreadFocus();
    // `#206 pin-message`. The Pinned filter's list stands in for whichever of these is up.
    deps.el("thread-list").hidden = deps.selection().view !== "threads" || deps.pinsView().only;
    deps.el("discord-log").hidden = deps.selection().view === "threads" || deps.pinsView().only;
    deps.el("pinned-log").hidden = !deps.pinsView().only;
    // `#216 compose-hint`. Where the message goes, said in the box rather than in a heading line above
    // it, and the same words as the box's name for a screen reader.
    const destination = deps.selection().threadId ? "Reply in this thread" : "Message the main channel";
    deps.el("channel-compose-text").setAttribute("aria-label", destination);
    (deps.el("channel-compose-text") as HTMLTextAreaElement).placeholder = `${destination}…`;
    deps.renderOutgoingMessages();
  }



  function rememberChannelContext(): void {
    deps.syncCanonDismissed();
    deps.channelContexts().set(channelContextKey(), {
      messages: deps.timelineRows().messages,
      threads: deps.timelineRows().threads,
      selected: deps.selection().thread,
      rows: [...deps.el("discord-log").children] as HTMLElement[],
      cards: [...deps.el("thread-list").children] as HTMLElement[],
      seam: [...deps.el("channel-summary").children] as HTMLElement[],
      more: deps.olderPages().moreAbove,
      cursor: deps.olderPages().cursor,
      newest: deps.listState().newestId,
      position: deps.captureScroll(),
      top: deps.el("scroll-area").scrollTop,
      freshness: deps.loadState().freshness,
      freshAt: deps.loadState().freshAt,
      dismissed: deps.doneState().archived,
      // `#221 read-modes`. The read mode its rows were drawn under, so a mode chosen elsewhere since
      // redraws them on the way back rather than showing what the old mode left.
      readMode: deps.readMode(),
    });
  }

  /**
   * The channel rows on screen, top to bottom — each one's message ids and the height its row stands
   * at — and whether the reader is on the newest line: the place a switch between Main and All keeps.
   * `#220 view-switch-instant`.
   *
   * `#269 approval-card-inline` x `#280 scroll-stability`. And, where the screen shows no row at all but
   * the send card at the foot of the channel's list, the height the card stands at (`card`, else null):
   * that reader's place. A long draft opened by More is taller than a phone's list — in Chromium 907px in
   * a list 739px tall — and a reader 40px up its foot, Send on screen and nothing else, was read as
   * nowhere: Cmd+3 and Esc brought them back 995px above the foot, the card 919px lower and Send off
   * screen, and one 200px up was taken to the foot. Only there: a reader who can see a row is on that row,
   * as before, card or no card.
   *
   * ...and whether the card was on screen at all (`cardSeen`), rows or none: a reader near enough the foot
   * to see it, for a switch that finds none of their rows in the view it goes to (`changeChannelView`).
   */
  function readerPlace(): ReaderPlaceResult {
    const area = deps.el("scroll-area");
    const top = area.getBoundingClientRect().top;
    const bottom = top + (area.clientHeight || 0);
    const listed = deps.view() === "discord" && deps.selection().view !== "threads" && !deps.pinsView().only
      ? [...deps.el("discord-log").children] : [];
    const seen = listed
      .map((row) => ({ row, box: row.getBoundingClientRect() }))
      .filter(({ box }) => box.bottom > top && box.top < bottom)
      .map(({ row, box }) => ({ ids: deps.idsOf(row), top: box.top }));
    const card = seen.length === 0 ? channelCard() : null;
    const box = card ? card.getBoundingClientRect() : null;
    const footCard = channelCard();
    const footBox = footCard ? footCard.getBoundingClientRect() : null;
    return {
      pinned: deps.view() === "discord" && deps.atBottom(area),
      seen,
      card: box && box.bottom > top && box.top < bottom ? box.top : null,
      cardSeen: Boolean(footBox && footBox.bottom > top && footBox.top < bottom),
    };
  }

  /**
   * The send card where it is the next message of the channel's list in the column being worked on, else
   * null. `#269 approval-card-inline`. Only ever the left column's: the card never ends the right one.
   */
  function channelCard(): HTMLElement | null {
    const card = deps.leftEl("post-confirm");
    return !deps.columnSide() && deps.view() === "discord" && !card.hidden && card.getAttribute("data-place") === "channel"
      ? card : null;
  }

  /**
   * Put the reader on the first message of `place` that the list now on screen also shows, at the
   * height it stood at — or on the newest line, when that is where they were. False, moving nothing,
   * when this list shows none of the messages they could see.
   *
   * ...or, where their screen showed only the send card, on the card, at the height it stood at, where it
   * ends this list too (`readerPlace`). `#269 approval-card-inline` x `#280 scroll-stability`.
   *
   * ...or, where that card has gone since, Send tapped or the draft settled elsewhere, on the newest line,
   * the place the card left at the foot, as it is for a reader who saw only the card as it went
   * (`cardAloneOnScreen`). The card goes from the dock under the thread list too, the reader's place read
   * as the list came up: in Chromium at 393x852, the long draft opened by More, Cmd+3 from 40 or 60px up
   * Main, the draft settled elsewhere or sent from the dock, and then Esc or Cmd+1 left them 182 or 202px
   * above the foot, and Cmd+2 353px above All's foot, offered the jump and no longer following the list.
   */
  function keepReaderPlace(place: ReaderPlaceResult): boolean {
    if (place.pinned) {
      deps.scrollToNewest();
      return true;
    }
    const rows = [...deps.el("discord-log").children] as HTMLElement[];
    for (const seen of place.seen) {
      // A message inside a collapsed run of read messages is on screen as the run's line. `#221 read-modes`.
      const row = deps.shownRowFor(rows.find((candidate) => deps.idsOf(candidate).some((id) => seen.ids.includes(id))));
      if (!row) continue;
      deps.el("scroll-area").scrollTop += row.getBoundingClientRect().top - seen.top;
      deps.renderJumpNewest();
      return true;
    }
    const card = typeof place.card === "number" ? channelCard() : null;
    if (card) {
      deps.el("scroll-area").scrollTop += card.getBoundingClientRect().top - (place.card as number);
      deps.renderJumpNewest();
      return true;
    }
    if (typeof place.card === "number" && deps.transcriptShown() === null) {
      deps.scrollToNewest();
      return true;
    }
    return false;
  }

  /**
   * Read the view a switch drew from the rows the store held, behind them: merged into the rows on
   * screen at the reader's place, with no loading line over them, and no "new" count for rows that
   * were only not drawn before. A failure is said by the freshness pill over the rows it leaves up.
   * `#220 view-switch-instant`.
   */
  async function readBehindSwitch(): Promise<void> {
    try {
      await deps.loadDiscordTimeline({ keepPosition: true, quiet: true, ownAct: true, reason: "enter" });
    } catch (error) {
      if (!(error && (error as { shownInPill?: boolean }).shownInPill)) throw error;
    }
  }

  /**
   * Show another view of the selected channel.
   *
   * `#220 view-switch-instant`. NEVER A BLANK LIST WHILE THERE IS SOMETHING TO DRAW. The owner,
   * 2026-10-08: "ALL view has loaded all the messages so switching to MAIN channel only should
   * basically just hide thread replies. But instead I saw it flash to a blank screen and do the
   * message fetching/loading animation. ... I don't want to fetch already cached data and I don't want
   * to WAIT in the UI if I don't have to." Main had never been read, so no page covered it, and the
   * switch emptied the list and read Main afresh — although every row of it was on the page in All.
   * So, in order: a view held in this page is put back; a view the store covers — read, saved on this
   * device, or Main or a thread taken from All's cover (`inheritAllCover`) — is projected; a view it
   * does not cover is drawn from the rows the store holds for it, and read behind them; and only a
   * view with nothing at all to draw is emptied and read with the loading line. Main and All keep the
   * reader on the message they were reading.
   *
   * `#269 approval-card-inline` x `#280 scroll-stability`. `before`: where the reader of the left column
   * was before a change of layout that brings this switch, read before the layout drew that column at
   * another width (`leftColumnPlace`). Read here, after it, the place is not the one the reader had.
   */
  async function changeChannelView(view: string, threadId: string | null = null, summary: ThreadSummary | null = null, origin: string | null = null, before: ReaderPlaceResult | null = null): Promise<void> {
    deps.noteDeepLinkReaderMove();
    // `#277 reply-in-column`. The list a reply open in this column answers a message of is going.
    if (deps.replyDocked() && deps.activeColumn() === deps.replyColumn()) deps.putDockedReplyAway();
    // Before the view goes: where the reader is, and how current what they are reading is — what a
    // view drawn from held rows is as current as. Where a change of layout brings the switch, where the
    // reader of the left column was before it (`before`, `leftColumnPlace`).
    const place = before && !deps.columnSide() ? before : readerPlace();
    const from = deps.selection().view;
    const came = { freshAt: deps.loadState().freshAt, freshness: deps.loadState().freshness };
    // `#206 pin-message`. Choosing a view is asking to see it, and the Pinned filter shows the same
    // pins in every view — so it steps aside, as it must for a pin opened in its thread. Placed by the
    // switch's own rules, not as a filter change (`#243 filter-keeps-selection`).
    deps.setPinnedOnly(false, false);
    // `#215 reply-coalesce`. Gathered replies are an arrangement of the list on screen, and go with
    // it — back into time order BEFORE the rows are kept below, so that coming back to this view
    // finds them in the order every other read of it draws. A tap on an arrow still waiting to jump
    // goes too: the message it would find is looked for in the list on screen when it fires.
    deps.putRepliesBack();
    deps.dropReplyArrowTap();
    deps.rememberChannelDraft();
    rememberChannelContext();
    // Where Back goes from a thread: the view it was opened from, or the one the opener says — the thread
    // list, for a thread opened from its cards (`#281 view-keys`), whose left column was Main behind it.
    if (view === "thread" && (origin || deps.selection().view !== "thread")) deps.selection().origin = origin || deps.selection().view;
    deps.selection().view = view;
    deps.selection().threadId = view === "thread" ? threadId : null;
    deps.selection().thread = view === "thread" ? summary : null;
    // `#221 read-modes`. Entering a thread owes the reader its first unread message, unfolded, once
    // its rows are drawn — from what this page holds, or when its read lands (`payThreadEntry`). The
    // thread is laid out afresh, so runs of read messages opened on an earlier visit collapse again.
    deps.entryState().threadOwed = view === "thread" ? channelContextKey() : null;
    deps.entryState().threadAwaitsRead = false;
    // ...and a landing made on an earlier visit is spent: a view this page held comes back with the
    // rows it was made on, at the scroll position it left, and must not hold the reader there again.
    deps.entryState().landing = null;
    if (view === "thread") deps.readRunsForget(deps.viewKey());
    ++deps.loadState().generation;
    deps.stopReading();
    deps.setReadingMode(false);
    const held = deps.channelContexts().get(channelContextKey());
    deps.timelineRows().messages = held ? held.messages : [];
    deps.timelineRows().threads = held ? held.threads : [];
    deps.selection().thread = summary || (held && held.selected) || null;
    // `#189 restore-ui-state`. The reader's own choice: it outranks the default from here on, and it
    // is what a reopen comes back to — and, `#205 channel-view-memory`, what this channel opens in
    // whenever the reader comes back to it. All too: what was picked, not whatever the default is.
    // `#229 desktop-two-column`. Not in two columns, though: the left column is Main there because of
    // the layout, not by the reader's choice, and the right column's thread is not a view the channel
    // opens in. Leaving two columns goes back to the choice made in one.
    // `#281 view-keys`: nor behind the thread list, whose left column is Main for the same reason.
    if (!deps.layoutShown().columns() && !deps.layoutShown().threadList() && !deps.columnSide() && !deps.layoutShown().restoring()) {
      const chosen = deps.viewOnScreen();
      if (chosen) deps.rememberView(chosen);
      // The Threads list is not a view a channel opens in: picked, it leaves the channel to its default.
      else deps.forgetChosenView((channel) => channel === String(deps.selectedChannelId()));
    }
    deps.settleUiState();
    deps.saveUiState();
    // `#304 back-gesture-in-app`. A thread entered has an entry to go back through; Main and All have none.
    deps.scheduleBackSync();
    // Main and All are one conversation, so a switch between them — or into one from a thread no place
    // of theirs is held for — keeps the reader on the message they were reading, where both show it.
    // A thread opens at its newest, and Back returns to the place its view was left at, as before.
    const keepPlace = (view === "main" || view === "flat") &&
      (from === "main" || from === "flat" || (from === "thread" && !held));
    const sameStore = deps.threading() && deps.channelCanon().channel === String(deps.selectedChannelId());
    // `#222 unread-replies`. A thread whose root reports replies the store has not brought — the tap on
    // "1 new" — is drawn from what is held as any other, and read behind it however it is drawn: from
    // rows held since an earlier visit, or a cover saved on the device, neither of which has the reply
    // the reader came for. Its landing waits for that read (`entryState().readOwed`).
    const outrun = view === "thread" && sameStore && deps.threadOutrunsStore(threadId, summary);
    deps.entryState().readOwed = outrun ? channelContextKey() : null;
    const inherited = !held && inheritAllCover(view, threadId, summary);
    const covered = !held && sameStore && deps.channelCanon().views.has(deps.viewKey());
    const rows = held || covered ? [] : deps.heldViewRows();
    deps.olderPages().moreAbove = held ? held.more : false;
    deps.olderPages().cursor = held ? held.cursor : null;
    deps.payReadCursor();
    deps.listState().newestId = held ? held.newest : null;
    // The rows on screen stay until the new view's replace them, unless there is nothing to draw.
    if (held || (!covered && rows.length === 0)) {
      deps.el("discord-log").replaceChildren(...(held ? held.rows : []));
      deps.el("thread-list").replaceChildren(...(held ? held.cards : []));
      deps.el("channel-summary").replaceChildren(...(held ? held.seam : []));
    }
    deps.el("timeline-notice").hidden = true;
    deps.loadState().freshAt = held ? held.freshAt : 0;
    deps.loadState().freshness = held ? held.freshness : "fresh";
    if (held) deps.doneState().archived = held.dismissed;
    // A read still on the wire is for the view the reader left: no loading line over this one.
    deps.renderChannelLoading(deps.readsBusy(), true);
    deps.restoreChannelComposer();
    deps.renderOlderControl();
    deps.voiceCall().renderControls();
    if (held) {
      deps.el("scroll-area").scrollTop = held.top;
      deps.restoreScroll(held.position);
      // `#269 approval-card-inline` x `#280 scroll-stability`. A reader who could see the send card at the
      // foot, and no message this view shows — replies near All's foot, which Main lacks — is at its foot,
      // the card in reach, as in a view not held, rather than where the view was last left: in Chromium at
      // a desk, Ctrl+4 from 40px up All, a long draft opened by More, left Main 326px above its foot, where
      // an earlier visit had left it, Send off screen.
      if (keepPlace && !keepReaderPlace(place) && place.cardSeen) deps.scrollToNewest();
      // Drawn from the store, where what it shows changed since: then nothing below draws it again.
      const caughtUp = catchUpHeldView();
      // What was read, dealt with or answered elsewhere since the held rows were drawn: in a thread
      // just left, above all, whose replies Main's chips count (`#222 unread-replies`). A message marked
      // Done, or put back, in another view meanwhile is in this view's copy already, not yet on its
      // rows (`#306 done-stale-across-views`).
      if (!caughtUp && (held.readMode !== deps.readMode() || deps.rowsMissDone())) deps.renderCachedTimeline();
      else deps.syncUnreadReplies();
      deps.renderChannelFreshness();
      // A thread read behind its held rows for replies they lack is entered at its newest, where those
      // will come, unless the landing finds an unread message among the rows held.
      if (outrun) deps.scrollToNewest();
      deps.payThreadEntry();
      // Switching back to a view already fetched in this page is a local
      // presentation change. The live stream and periodic poll refresh the
      // active view; a tab switch itself must not become another network wait.
      // ...but a view drawn from held rows whose read never landed still owes that read — queued
      // behind a read of another view still on the wire; one of this view lands here by itself.
      // ...and so does a thread whose root reports replies the held rows lack (`#222 unread-replies`).
      if (outrun || (sameStore && view !== "threads" && !deps.channelCanon().views.has(deps.viewKey()) &&
          deps.readContext() !== channelContextKey())) {
        await readBehindSwitch();
      }
      return;
    }
    // Not drawn in this page yet, but covered by the channel's store: project it, with no request.
    // The stream and the poll keep it current from here.
    if (covered && deps.drawProjection()) {
      // Saved rows drawn while the device is offline say so, as the view the reader left did.
      if (!(deps.channelCanon().views.get(deps.viewKey())?.live) && came.freshness === "offline") deps.setChannelFreshness("offline");
      deps.renderChannelFreshness();
      if (!(keepPlace && keepReaderPlace(place))) deps.scrollToNewest();
      // A thread lands on its first unread, from the rows just projected. `#221 read-modes`.
      deps.payThreadEntry();
      // Main is a view of its own from here, in the snapshot too. A thread drawn from All's cover is
      // saved as All's, as it always was.
      if (inherited && view === "main") deps.saveChannelScope();
      // Main drawn from All with the reader at its top — All's window held few roots, or they are
      // reading the oldest of them: arriving there asks for what is above, as scrolling there does.
      if (deps.walkCursorOwed()) deps.maybeLoadOlder();
      // A thread whose saved cover lacks replies its root reports is read behind it. `#222 unread-replies`.
      if (outrun) await readBehindSwitch();
      return;
    }
    // Covered by nothing, but the store holds some of it: up at once, and read behind it.
    if (deps.drawHeldRows(rows, came.freshAt, came.freshness)) {
      if (!(keepPlace && keepReaderPlace(place))) deps.scrollToNewest();
      // A thread lands on its first unread among those rows now, and the read behind them keeps it
      // there, or brings the root to the head above it when both fit (`keepEntryLanding`). `#221
      // read-modes`.
      deps.payThreadEntry();
      await readBehindSwitch();
      return;
    }
    // Nothing to draw: the one case the loading line is for.
    deps.el("discord-log").replaceChildren();
    deps.el("thread-list").replaceChildren();
    deps.el("channel-summary").replaceChildren();
    if (deps.readsBusy()) deps.renderChannelLoading(true);
    deps.scrollToNewest();
    await deps.loadDiscordTimeline({ reason: "enter" });
  }

  /**
   * A THREAD IS A SUBSET OF ALL. Opened while All is covered and the thread is not, it inherits All's
   * cover and is drawn as a filter of the messages already held: no loading screen for a narrowing of
   * what is on the page — on opening it, and on reopening the page in it.
   *
   * ...when All reaches back to where the thread starts. `#189 restore-ui-state` made All the view a
   * channel opens in, so All is nearly always covered, and an old thread — what the thread list's
   * older pages are for — began before All's newest page: inherited, it drew empty, with no cursor to walk back on
   * until the poll read it. That thread is read for itself instead.
   *
   * ...and when nothing says the thread has outgrown what is held. `#222 unread-replies`: a root's
   * count, as Main last read it, can be ahead of All's read, by replies nothing has brought — Slack's
   * stream brings a reply a poll late, and none in a thread rooted before the newest page of history
   * (`#307 slack-reply-live`). All does not answer for that thread, so it is read too
   * (`threadOutrunsStore`). Nor for one whose older replies All's page did not bring — Slack's brings
   * the newest fifty of each thread it reads upstream (and once read only ten threads a page): drawn
   * from All, it showed its root and not all of them, until the poll (`storeLacksReplies`).
   *
   * SO IS MAIN, and always from where All starts: every message posted to the channel itself, thread
   * roots included, is a row of All. `#220 view-switch-instant`. Main is covered wherever All is, and
   * drawn by hiding All's thread replies — on switching to it and on reopening the page in it — with
   * no read. The roots keep their reply counts: their thread records, and the summaries, are the
   * store's. Not All's notice, which can be about All alone ("the newest 50 of each"): Main's
   * own read says Main's. And a Main the store covers already is as current as All when All's newest
   * page is the later read, since that page answered for every Main row in it. Either way Main has no
   * cursor of its own to walk back on, and `fromAll` says so: its first step back is its own read
   * (`walkCursorOwed`). Main's own page replaces the cover, and the mark goes with it.
   *
   * True when it gave the view a cover or freshened one, which the snapshot then has to be told.
   */
  function inheritAllCover(view: string, threadId: string | null = null, summary: ThreadSummary | null = null): boolean {
    if (!deps.threading() || deps.channelCanon().channel !== String(deps.selectedChannelId())) return false;
    const all = deps.channelCanon().views.get(deps.viewKey("flat"));
    if (!all || (view !== "main" && view !== "thread")) return false;
    const key = deps.viewKey(view, threadId);
    const own = deps.channelCanon().views.get(key);
    // Not All's forward cursor, which is bound to All: the view's first refresh reads in full.
    // `#203 incremental-refresh`.
    if (view === "main" && !own) {
      const { landedHidden: _hidden, ...cover } = all;
      deps.channelCanon().views.set(key, { ...cover, notice: "", cursor: null, newest: null, fullAt: 0, fromAll: true });
    } else if (view === "main" && own && all.live && (!own.live || (all.at as number) > (own.at as number))) {
      own.live = true;
      own.at = all.at;
      own.fromAll = true;
      // `#578 search-status-clarity`. All's newest read speaks for Main's rows too, and so does a gap it
      // found: below it, Main's rows walked back to earlier are scattered as All's are.
      if (Number.isFinite(all.dense)) own.dense = Number.isFinite(own.dense) ? Math.max(own.dense as number, all.dense as number) : all.dense;
      if (Number.isFinite(all.top)) own.top = Number.isFinite(own.top) ? Math.max(own.top as number, all.top as number) : all.top;
    } else if (view === "thread" && !own && deps.allReachesThread(deps.channelCanon(), threadId, summary) && !deps.storeLacksReplies(threadId, summary)) {
      deps.channelCanon().views.set(key, { ...all, cursor: null, newest: null, fullAt: 0 });
    } else {
      return false;
    }
    return true;
  }

  /**
   * Whether `held` and `arriving` say the same, field for field, whatever order their fields are in.
   * `sameMessage` compares JSON, which depends on that order, and a copy put together here — the
   * stream's, given the thread record, the reactions or the edit mark the store's copy had
   * (`foldLiveMessage`) — has those fields last. `#333 page-live-reads-remainder`.
   *
   * @param {any} held
   * @param {any} arriving
   * @returns {boolean}
   */
  function sameWords(held: unknown, arriving: unknown): boolean {
    if (held === arriving) return true;
    if (typeof held !== "object" || typeof arriving !== "object" || held === null || arriving === null) return false;
    if (Array.isArray(held) || Array.isArray(arriving)) {
      return Array.isArray(held) && Array.isArray(arriving) && held.length === arriving.length &&
        held.every((one, at) => sameWords(one, arriving[at]));
    }
    const fields = (item: Record<string, unknown>): string[] => Object.keys(item).filter((key) => item[key] !== undefined);
    const mine = fields(held as Record<string, unknown>);
    return mine.length === fields(arriving as Record<string, unknown>).length &&
      mine.every((key) => (arriving as Record<string, unknown>)[key] !== undefined && sameWords((held as Record<string, unknown>)[key], (arriving as Record<string, unknown>)[key]));
  }

  /**
   * A held view missed what the stream delivered while it was hidden; the store did not. True when
   * that changed the rows on screen.
   *
   * ...or what another view's read brought. `#333 page-live-reads-remainder`. The rows are compared
   * word for word, not only by which messages they are (`sameWords`): a read of one view that brings
   * an edited copy of a message another held view shows — an agent's placeholder edited into its
   * answer, read in its thread — left that view drawing the old words when it was shown again, or in
   * the other column, until its own next read, up to a poll later. `#306 done-stale-across-views`
   * closed the same gap for Done. A copy that says the same, in another object or with its fields in
   * another order, draws nothing. The cards of the thread list are compared by id alone, as before:
   * a count or a time that changed on one card would rebuild them all, and its own read draws them.
   *
   * `words` false compares ids alone, as before: for a read overtaken by a read of the same view
   * queued behind it, which is what draws that view (`channelReads.queued`).
   */
  function catchUpHeldView(words = true): boolean {
    const projected = deps.threading() ? deps.projectView() : null;
    if (!projected) return false;
    const cover = deps.channelCanon().views.get(deps.viewKey());
    if (cover && cover.landedHidden) {
      // A page read for this view arrived while it was hidden: it is as current as that read.
      delete cover.landedHidden;
      deps.loadState().freshAt = cover.at as number;
      deps.loadState().freshness = "fresh";
    }
    const threads = deps.selection().view === "threads";
    const held = threads ? deps.timelineRows().threads : deps.timelineRows().messages;
    const ids = (items: readonly { id: unknown }[]): string => items.map((item) => String(item.id)).join("\n");
    const unchanged = ids(projected) === ids(held) &&
      (!words || threads || projected.every((item, at) => item === held[at] || sameWords(held[at], item)));
    if (unchanged) return false;
    if (threads) {
      deps.timelineRows().threads = projected as ThreadSummary[];
      deps.el("thread-list").replaceChildren(...deps.threadCards(projected as ThreadSummary[]));
    } else {
      deps.timelineRows().messages = projected as Message[];
      deps.renderCachedTimeline();
    }
    return true;
  }

  /**
   * A page read for All landed while the reader is in Main, or in a thread drawn from All. `#220
   * view-switch-instant`. All's newest page, or a delta continuing it, answers for every row of Main
   * from All's floor up, and for a thread All reaches: so the view on screen is caught up from the
   * store now, as a refresh of its own would be — the newest line followed, or the reader's place kept
   * and what arrived counted — and Main is as current as that read. Without this, Main drawn from All
   * went on showing what All held before, under a pill saying Live, until its own poll. Only a view
   * the store covers: one being read for itself is drawn by that read.
   */
  function catchUpFromAll(): void {
    if (!deps.threading() || (deps.selection().view !== "main" && deps.selection().view !== "thread") ||
        deps.channelCanon().channel !== String(deps.selectedChannelId()) || !deps.channelCanon().views.has(deps.viewKey())) return;
    if (inheritAllCover(deps.selection().view, deps.selection().threadId, deps.selection().thread)) {
      deps.loadState().freshAt = deps.channelCanon().views.get(deps.viewKey())?.at as number;
      deps.setChannelFreshness("fresh");
      // Saved, now live: older history is known to exist, and the step back to it is Main's read.
      if (deps.walkCursorOwed()) {
        deps.olderPages().moreAbove = true;
        deps.renderOlderControl();
      }
    }
    const area = deps.el("scroll-area");
    const position = deps.channelReadPosition(area);
    if (catchUpHeldView()) {
      const keeps = deps.readFilterKeeps();
      deps.settleAfterRead(deps.timelineRows().messages.filter((message) => !deps.hiddenEmpty(message) && (!deps.hidingRead() || keeps(message))),
        { keepPosition: true, area, ...position });
    }
    deps.renderChannelFreshness();
  }



  return {
    threadHues, channelContextKey, heldThreadRecord, channelRepliesInThread, threadReplyCount,
    threadReplies, threadForMessageId, withThreadQuery, restoreThreadHues, noteServerThreadHuesIn,
    seedServerThreadHues, threadHue, threadCount, renderChannelNavigation, renderColumnLists,
    rememberChannelContext, readerPlace, channelCard, keepReaderPlace, readBehindSwitch,
    changeChannelView, inheritAllCover, catchUpHeldView, catchUpFromAll,
  };
}

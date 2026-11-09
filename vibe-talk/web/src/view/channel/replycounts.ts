// Reply counts and the way down to them: the Thread button before there is a thread, unread
// replies on Main, and how many replies by pointer a message has with the circle that takes the
// reader to them (`#624 voice-page-modularity`, moved from the legacy script). Words, order and
// timing are as they were in the script. What the store holds of every thread's replies stays
// core/channels' (`replyTallyOf`).

import { allReachesThread, emptyTally, exactReplyCount, replyTallyOf, tallyRecord, threadOf, unseenRepliesIn } from "../../core/channels/index.ts";
import type { ChannelCanon, ReplyTally } from "../../core/channels/index.ts";
import { countOf, replyMarkerName } from "../../present/rows/index.ts";
import { childByClass } from "../../view/dom/index.ts";
import type { Message, ThreadSummary } from "../../contract/index.ts";
import { QUOTE_REPLY_TITLE } from "./quoting.ts";

type ReplyButton = HTMLElement & { replyCount?: number; quoteFor?: string; quoteShort?: boolean; quoteWidth?: number | string };
type Row = HTMLElement & { replyButton?: ReplyButton | null; replyMarker?: HTMLElement | null; replyArrowButton?: HTMLElement | null };

/** What the reply counts ask of the page. */
export interface ReplyCountsDependencies {
  answeredElsewhere(target: string): boolean;
  cardSplit(choice: unknown): unknown;
  el(id: string): HTMLElement & { disabled?: boolean };
  focusMinimally(box: unknown): void;
  gatherCount(id: string, message: Message, held: number): { count: number | null; exact: boolean };
  guardQuietly(fn: () => unknown): () => void;
  hiddenEmpty(message: Message): boolean;
  hidingRead(): boolean;
  idsOf(row: unknown): string[];
  inColumn(column: unknown, act: () => void): void;
  inThisColumn(fn: (event: unknown) => void): (event: unknown) => void;
  inView(message: Message, view: string, threadId: unknown): boolean;
  knownChannel(id: unknown): { writable: boolean } | null | undefined;
  listState(): { outgoingKey: string };
  openReadRun(row: Row): void;
  openThread(id: string): unknown;
  openThreadChoice(): unknown;
  openThreadRootId(): string | null;
  outgoingReplyKey(): string;
  paintThreadStrip(row: Row, id: string | null): void;
  pendingInto(id: string): number;
  quoteLabelFits(button: unknown, plain: string, full: string): boolean;
  renderScrollTools(): void;
  replyArrows(): { countsDrawn: boolean };
  revealRow(row: Row): void;
  rootInward(choice: unknown): boolean;
  rowHolding(id: string): Row | null;
  rowMessages(row: unknown): Message[];
  rowShown(row: Row): boolean;
  seekMessage(target: string, land: () => boolean, elsewhere: () => boolean): Promise<void>;
  selectedChannelId(): unknown;
  selection(): { view: string; threadId: unknown };
  sentFromThisPage(message: Message): boolean;
  setSearchOpen(open: boolean): void;
  setStatus(text: string): void;
  shownNode(node: unknown): boolean;
  stillToDo(message: Message, done?: Set<string>): boolean;
  threadCount(count: number | null, exact: boolean): string;
  threadHue(id: string): string;
  threadReplyCount(id: string, message: unknown): number | null;
  timeOf(message: unknown, field: string): number;
  viewKey(view: string, threadId?: unknown): string;
  channelStore(): { current(): ChannelCanon };
  providerFacts: { threading(): boolean };
  columns(): { main: unknown; side: unknown };
  layoutShown(): { columns(): boolean };
  outbox(): { values(): Iterable<{ channel: string; replyTo: string | null; parts: { reply_to?: unknown; id?: unknown; thread?: { is_root?: unknown; root_message_id?: unknown } | null }[]; state: string; id: string; postedCount: number; text: string; createdAt: number }> };
  outboxDispatch(): { retryPending(id: string): boolean };
  quoteOwner(): { held(): { button: unknown } | null };
}

/** The reply counts: tallies, chips, labels and the way down. */
export interface ReplyCounts {
  REPLY_BUTTON_TITLE: string;
  unansweredRoot(message: unknown, reported: number | null, held: number, exact: number | null): boolean;
  paintThreadBadge(badge: HTMLElement, id: string, replies: number, unanswered: boolean): void;
  startThread(id: string): unknown;
  unansweredThread(message: unknown, record: { id?: unknown }): boolean;
  heldUnansweredRoot(id: unknown): Message | null;
  replyTally(): ReplyTally;
  rootThreadOf(message: Message, tally: ReplyTally): string | null;
  unseenReplies(id: unknown, message: Message | null, tally: ReplyTally, summary?: unknown): number;
  threadOutrunsStore(id: unknown, summary?: unknown): boolean;
  storeLacksReplies(id: unknown, summary?: unknown): boolean;
  repliesPartlyHeld(id: unknown, message: Message | null, held: number, tally: ReplyTally): boolean;
  rootReplyState(message: Message, tally: ReplyTally): { id: string | null; unread: number; unseen: number };
  holdsUnreadReplies(message: Message, tally: ReplyTally): boolean;
  readFilterKeeps(thread?: unknown): (message: Message) => boolean;
  unpackFilter: { empty(message: Message): boolean; hiding(): boolean; keeps(choice: unknown): (message: Message) => boolean; toDo(message: Message, done: unknown): boolean };
  drawThreadChips(row: Row, tally: ReplyTally, inMain: boolean, inAll?: boolean): void;
  heldReplies(rows: readonly Row[]): { replies: Map<string, Map<string, number>>; held: Map<string, Message> };
  labelReplyButton(button: ReplyButton | null | undefined, count: number): void;
  drawReplyLabel(button: ReplyButton): void;
  renderReplyCounts(): void;
  nextReplyTarget(marker: HTMLElement): number;
  labelReplyMarker(marker: HTMLElement): void;
  replyMarker(row: Row): unknown;
  jumpToReply(target: string, at: number, of: number): Promise<void>;
  landOnReply(target: string, at: number, of: number): boolean;
}

/** The reply counts over `deps`. */
export function createReplyCounts(deps: ReplyCountsDependencies): ReplyCounts {
  // --- the Thread button before there is a thread ---------------------------------------------------
  //
  // `#252 thread-button-state`. The owner, 2026-10-12: "Thread button doesn't deserve a color UNLESS
  // there is a thread. Right now I send a brand new message in the main chat and it immediately has a
  // colored "(Thread)" button in the upper right even though it has zero responses. I think it should
  // be a functional button for CREATING a new thread from vibe-talk... but it should be greyed and have
  // no color yet until there is an actual thread created (at least one response)."
  //
  // Google Chat names a thread for every message, so every new message there heads a thread nobody has
  // answered, and its tag was drawn in a colour of its own, as a thread's is. Such a tag is grey now,
  // as Reply beside it is, says plain "Thread", and STARTS the thread: the thread opens as the tag
  // opens any thread — on the right, in two columns — with the message alone in it and its message box
  // focused, and what is sent from there is the thread's first reply. No hue is spent on it: the
  // colours tell threads apart, and a channel where every message heads a thread would otherwise give
  // every message one. The first reply — the reader's own, once acknowledged, or anyone's, brought by a
  // read or the stream — makes it a thread, and the next pass over the rows draws the coloured
  // Thread(1) in its place (`drawThreadChips`).
  //
  // WHERE A THREAD CAN BE STARTED. Only where the provider has named the thread the message heads, by
  // a count that says nobody has answered it: a count of zero, not a missing one, since a root whose
  // provider gives no count may well have replies, and keeps the coloured tag it had. Google Chat
  // names one for every message, and so does this server's Slack backend: Slack lets any message on
  // the channel head a thread, whose id is the message's ts, though Slack itself writes the thread
  // down only at its first reply (src/slack/client.rs). Discord's threads are channels of their own,
  // which this server does not create: a message that heads none has no tag. A Discord thread
  // somebody made and nobody has written in yet is grey like Google Chat's, and its first reply goes
  // into it. And not on a channel that is read-only, where nothing can be posted to start one: a
  // control that cannot act is absent.

  /**
   * Whether `message` heads a thread nobody has answered yet: it is the thread's root, its count
   * (`reported`, the summary's or its own) says zero, and nothing says more — not the replies the store
   * holds, `held`, nor the highest exact count, `exact`.
   */
  function unansweredRoot(message: unknown, reported: number | null, held: number, exact: number | null) {
    const record = message ? (message as { thread?: { is_root?: unknown } | null }).thread : null;
    return Boolean(record) && record!.is_root === true && reported === 0 && held === 0 && !exact;
  }

  /**
   * Paint a row's thread tag for its state: grey "Thread", which starts the thread (`startThread`),
   * for a thread nobody has answered — absent on a channel that is read-only — and otherwise the
   * thread's coloured Thread(N), which opens it. Written only where it changes: the pass over the rows
   * paints every tag again.
   */
  function paintThreadBadge(badge: HTMLElement, id: string, replies: number, unanswered: boolean) {
    const state = unanswered ? "start" : "open";
    const text = !unanswered && replies ? `Thread(${replies})` : "Thread";
    const title = unanswered ? "Start a thread on this message" : "Open this thread";
    const label = unanswered ? title : replies ? `Open this thread, ${deps.threadCount(replies, true)}` : null;
    const hidden = unanswered && deps.knownChannel(deps.selectedChannelId())?.writable === false;
    if (badge.getAttribute("data-thread-state") !== state) {
      badge.setAttribute("data-thread-state", state);
      // The colour is the thread's, and there is no thread yet; it is given when there is.
      if (unanswered) badge.style.removeProperty("--thread-hue");
      else badge.style.setProperty("--thread-hue", deps.threadHue(id));
    }
    if (badge.textContent !== text) badge.textContent = text;
    if (badge.getAttribute("title") !== title) badge.setAttribute("title", title);
    if (label === null) badge.removeAttribute("aria-label");
    else if (badge.getAttribute("aria-label") !== label) badge.setAttribute("aria-label", label);
    if (badge.hidden !== hidden) badge.hidden = hidden;
  }

  /**
   * The grey Thread tag was pressed: open thread `id`, which nobody has answered yet, as its tag opens
   * any thread — on the right, in two columns — and put the focus in its message box, ready for its
   * first reply. Nothing is posted until that reply is sent, and that send is what starts the thread.
   */
  function startThread(id: string) {
    const opened = deps.openThread(id);
    deps.inColumn(deps.layoutShown().columns() ? deps.columns().side : deps.columns().main, () => {
      const box = deps.el("channel-compose-text");
      // The box is at the foot of the thread, and writing in it is going there: as far as shows it.
      if (deps.shownNode(box) && !box.disabled) deps.focusMinimally(box);
    });
    return opened;
  }

  /**
   * Whether `message`, whose thread record is `record`, heads a thread nobody has answered: the one
   * decision the grey Thread tag paints (`drawThreadChips`) and Reply's "Start a new thread" box is
   * offered by (`replyRoute`), so the two cannot disagree about the same message. Its count says zero —
   * a summary's before the record's — the store holds no reply in it, and no exact count says more:
   * a live reply the store holds makes it a thread at once, on the tag and on Reply alike — on a Reply
   * screen already open as well, which asks again with every redraw (`followReplyStore`).
   * `#185 reply-new-thread`, `#252 thread-button-state`.
   *
   * @param {object} message
   * @param {object} record its thread record, the store's copy's before the row's (`heldThreadRecord`)
   */
  function unansweredThread(message: Message, record: { id: unknown }) {
    const id = String(record.id);
    const tally = replyTally();
    const entry = tally.threads.get(id);
    return unansweredRoot({ thread: record }, deps.threadReplyCount(id, /** @type {any} */ ({ thread: record })), entry ? entry.held : 0,
      exactReplyCount(id, message, tally));
  }

  /**
   * The root of thread `id` when the store holds it and nobody has answered it — no reply held, and a
   * count of zero (`unansweredRoot`) — or null. Such a thread is its root alone, and is drawn from it
   * at once, with no read to wait for (`heldViewRows`).
   */
  function heldUnansweredRoot(id: string) {
    const key = String(id);
    const thread = deps.channelStore().current().messages.filter((message: Message) => threadOf(message) === key);
    const root = thread.find((message: Message) => message.thread!.is_root === true);
    if (!root || thread.length !== 1) return null;
    const tally = replyTally();
    return unansweredRoot(root, deps.threadReplyCount(key, root), 0, exactReplyCount(key, root, tally)) ? root : null;
  }

  // --- unread replies, on Main ----------------------------------------------------------------------
  //
  // `#222 unread-replies`. The owner, 2026-10-08: replying marks a message read, and his own messages
  // count as read automatically, so in Main his last messages — each answered by an agent in a thread —
  // vanished under Hide read although the agents' replies were unread. He treats the channel as an
  // inbox. So on Main a root whose thread holds unread replies says how many, in a chip beside its N
  // replies — "12 replies  3 unread", his "N replies, M unread" — and the read filters keep that root:
  // Hide read leaves it in the list and Fold read never folds it into a run. The root itself is
  // still drawn as read; the chip is drawn at full strength over it (web/voice.css), so the cue is not
  // faded with the root it is on.
  //
  // A CHIP OF ITS OWN, and a tap on it enters the thread, which lands on its first unread message
  // (`#221 read-modes`): the count of what is waiting is the way to the first of it. Not more words on
  // N replies, which in Main opens All to gather the replies — and under Hide read All leaves the read
  // root out, with them. Nor on Thread(N), the way in that every row has: in the meta line of a phone
  // "Thread(12) · 3 unread" wrapped the row's controls onto a second line, the ⋯ at its start,
  // measured at 412px on exactly the rows this is for — the owner's own messages, inset from the
  // edge. The line N replies is on has the room. Nothing is drawn when nothing is unread.
  //
  // UNREAD is `stillToDo`'s rule, the one both read modes use: not Done, not the reader's own words,
  // not noise. FROM THE REPLIES THE STORE HOLDS, with the store's read state, because Main draws no
  // reply and its rows' `dismissed` says nothing about one. The replies come from All's pages — Main is
  // drawn from All's cover (`#220 view-switch-instant`) — the thread's own, and the stream. Their read
  // state comes from those pages (`foldReadState`), from Done here (`markHeldDone`), and from Main's own
  // reads, which carry every mark from their oldest row up (`reply_dismissals`): a reply dealt with or
  // put back on another device is known at Main's next read, without reading All or the thread.
  // Counting on the server would read every thread on Main's page from the chat service on every read
  // of Main; the store already holds what is recent, and the one read this adds is bounded: All's newest
  // page, behind Main, when Main's counts outrun the store, at most every few minutes (`readAllForCounts`).
  //
  // WHAT IS NOT HELD BUT IS REPORTED IS NEW. The store holds a reply only once a read of All or of its
  // thread, or the stream, brought it — and while the reader sits on Main only Main is read. Slack's
  // stream carries a reply only a poll after it was posted, and only in a thread rooted on the newest
  // page of history (`#307 slack-reply-live`); the Google Chat bridge's carries one without saying
  // which thread it is in. What Main's own read does carry is each root's count, so the page knows a
  // reply exists before it holds it. A shortfall of the held replies against an exact count is one of
  // two things: replies older than anything read of the thread, which the reader has had every chance
  // to see, or replies posted since the last read that brought the thread's newest, which nothing has
  // brought. `noteReplyGaps` keeps the first by thread, whenever a read says it (All's page carrying
  // the root, the thread's own read, a step back), so the rest is the second (`unseenReplies`): those
  // may be unread, and the root says so — "2+ unread" beside what is known, "1 new" when nothing held
  // is unread — and is kept by both read filters until a read of the thread or of All brings them and
  // the count is exact again. A thread no read has said anything of — Main opened cold, before All or
  // the thread was ever read — counts its whole shortfall, which errs towards keeping a root that is
  // read over hiding one that is not, the failure this is for; and All is read behind Main for it.
  //
  // What is older than anything read and not held is not counted either way: a thread holding no
  // unread reply it knows of, and no new one, says nothing, and its root is read like any other. One
  // that does hold one says it is a floor, "3+ unread".
  //
  // ONLY ON MAIN. In All and in a thread the replies are rows of the list, and speak for themselves.

  /**
   * What the store holds of every thread's replies (web/src/core/channels `ReplyTally`).
   *
   * @typedef {import("./src/core/channels/index.ts").ReplyTally} ReplyTally
   */

  /**
   * What the store holds of every thread's replies (web/src/core/channels `replyTallyOf`), unread by
   * `stillToDo` over the store's read state: nothing for a store that is not the channel on screen, or
   * a service with no threads. One walk over the store, for a pass over the rows that asks after every
   * thread on screen.
   *
   * @returns {ReplyTally}
   */
  function replyTally() {
    if (!deps.providerFacts.threading() || deps.channelStore().current().channel !== String(deps.selectedChannelId())) return emptyTally();
    return replyTallyOf(deps.channelStore().current(), deps.stillToDo);
  }

  // `tallyRecord` is web/src/core/channels's (`#624 voice-page-modularity`).

  /** The thread `message` heads, when it is a root Main shows and the channel is not that thread, or null. */
  function rootThreadOf(message: Message, tally: ReplyTally) {
    const record = tallyRecord(message, tally);
    return record && record.is_root === true && String(record.id) !== String(deps.channelStore().current().scope)
      ? String(record.id) : null;
  }

  // `exactReplyCount` is web/src/core/channels's (`#624 voice-page-modularity`).

  /**
   * How many of thread `id`'s replies were posted since the last read that brought its newest, in the
   * selected channel's store (web/src/core/channels `unseenRepliesIn`).
   */
  function unseenReplies(id: string, message: Message | null, tally: ReplyTally, summary: ThreadSummary | null = null) {
    return unseenRepliesIn(deps.channelStore().current(), id, message, tally, summary);
  }

  /**
   * Whether thread `id` has replies the store has not brought (`unseenReplies`). Then All's cover,
   * however far back it reaches, does not answer for the thread — those replies came after All's read
   * — and entering it reads it, behind what is held: a tap on "1 new" that drew the thread from All
   * would show everything but the reply it was tapped for.
   */
  function threadOutrunsStore(id: string, summary: ThreadSummary | null = null) {
    const key = String(id);
    const root = deps.channelStore().current().messages.find((message: Message) => threadOf(message) === key && message.thread!.is_root === true);
    return unseenReplies(key, root || null, replyTally(), summary) > 0;
  }

  /**
   * Whether the store holds fewer of thread `id`'s replies than its exact count reports, new or old
   * (`threadOutrunsStore` is the new ones only). Then All's cover does not hold the thread whole,
   * however far back it reaches, and entering the thread reads it. `#222 unread-replies`.
   */
  function storeLacksReplies(id: string, summary: ThreadSummary | null = null) {
    const key = String(id);
    const root = deps.channelStore().current().messages.find((message: Message) => threadOf(message) === key && message.thread!.is_root === true);
    const tally = replyTally();
    const count = exactReplyCount(key, root || null, tally, summary);
    const entry = tally.threads.get(key);
    return count !== null && count > (entry ? entry.held : 0);
  }

  /**
   * Whether thread `id` may have replies the store does not hold, holding `held`: then what is counted
   * of it is a floor. By an exact count when there is one; otherwise unless the thread's own read or
   * All's reaches back to where it starts.
   */
  function repliesPartlyHeld(id: string, message: Message, held: number, tally: ReplyTally) {
    const count = exactReplyCount(id, message, tally);
    if (count !== null) return count > held;
    const own = deps.channelStore().current().views.get(deps.viewKey("thread", id));
    if (own && !own.more) return false;
    return !allReachesThread(deps.channelStore().current(), id, null);
  }

  /**
   * What a root on Main says of its thread's replies, by `tally`: the unread ones held, and the ones
   * reported but not held (`unseenReplies`). Nothing outside Main, where the replies are rows of the
   * list, and nothing for a message that heads no thread.
   */
  function rootReplyState(message: Message, tally: ReplyTally) {
    const id = deps.selection().view === "main" ? rootThreadOf(message, tally) : null;
    if (id === null) return { id: null, unread: 0, unseen: 0 };
    const entry = tally.threads.get(id);
    return { id, unread: entry ? entry.unread : 0, unseen: unseenReplies(id, message, tally) };
  }

  /** Whether `message` is a root on Main whose thread holds an unread reply, or may (`rootReplyState`). */
  function holdsUnreadReplies(message: Message, tally: ReplyTally) {
    const { unread, unseen } = rootReplyState(message, tally);
    return unread > 0 || unseen > 0;
  }

  /**
   * What the read filters keep: an unread message, and on Main a root whose thread holds an unread
   * reply, or may, as well (`holdsUnreadReplies`). Hide read draws exactly these; Fold read folds
   * every row but these (`applyReadRuns`, by the attribute the pass over the rows sets from the same
   * rule). The store is walked only when a read row is asked after: Show read and Fold read ask
   * after none.
   *
   * `thread` is a thread whose messages are drawn, as a thread's: the open thread of the column by
   * default, or one unpacked under its card (`unpackBlock`, `#407 unpack-row-parity`), which keeps what
   * the thread would.
   *
   * @param {import("./src/present/rows/index.ts").ThreadChoice | null} [thread]
   * @returns {(message: any) => boolean}
   */
  function readFilterKeeps(thread: { id: unknown; root?: { id: unknown } | null } | null = deps.selection().view === "thread" ? deps.openThreadChoice() as { id: unknown; root?: { id: unknown } | null } | null : null) {
    const counting = deps.providerFacts.threading() && deps.selection().view === "main" && thread === null;
    // `#405 root-under-summary`. Under a summarized card the root is kept however read it is: it is
    // the first child the card's words are about, and Hide read hiding it would leave the summary
    // heading a thread whose first message cannot be seen. A raw card already says the root.
    // `#522 thread-card-max-lines`: and under a split card, whose continuation is the root's row.
    const keptRoot = thread && (deps.rootInward(thread) || deps.cardSplit(thread))
      ? (deps.selection().view === "thread" && thread.id === String(deps.selection().threadId) ? deps.openThreadRootId()
        : thread.root ? String(thread.root.id) : null) : null;
    let tally: ReplyTally | null = null;
    return (message: Message) => {
      if (deps.stillToDo(message) || deps.sentFromThisPage(message)) return true;
      if (keptRoot !== null && String(message.id) === keptRoot) return true;
      if (!counting) return false;
      if (tally === null) tally = replyTally();
      return holdsUnreadReplies(message, tally);
    };
  }

  /**
   * The thread chip and N replies on one row, from `tally`, with counts never below the replies held;
   * and on a root in Main's list the count of its unread replies, in a chip of its own beside N replies.
   * Called by every pass over the rows, which follows every redraw, so the counts follow arrivals, reads
   * and Done wherever they happen. Written only when they change: this runs with every pass.
   */

  /**
   * The read-state facts unpacking filters by (web/src/core/channels `UnpackFilter`,
   * `#624 voice-page-modularity`), for `unpackMessages`/`unpackReadSignature`/`readLook`.
   *
   * @type {import("./src/core/channels/index.ts").UnpackFilter}
   */
  const unpackFilter = {
    empty: (message: Message) => deps.hiddenEmpty(message),
    hiding: () => deps.hidingRead(),
    keeps: (choice: unknown) => readFilterKeeps(choice as { id: unknown; root?: { id: unknown } | null }),
    toDo: (message: Message, done: Set<string>) => deps.stillToDo(message, done),
  };
  function drawThreadChips(row: Row, tally: ReplyTally, inMain: boolean, inAll: boolean = false) {
    const message = deps.rowMessages(row)[0];
    if (!message) return;
    const own = threadOf(message);
    const badge = own ? childByClass(row, "thread-badge") : null;
    if (own && badge) {
      const entry = tally.threads.get(own) || { held: 0, unread: 0 };
      const reported = deps.threadReplyCount(own, message);
      const exact = exactReplyCount(own, message, tally);
      // `#466 reply-promotes-thread`: with the sends in the outbox going into it.
      const pending = deps.pendingInto(own);
      const replies = Math.max(typeof reported === "number" ? reported : 0, entry.held, exact || 0) + pending;
      // `#252 thread-button-state`. Grey while nobody has answered; the coloured Thread(N) from the
      // first reply on, which is this pass, the next time the rows are passed over after it arrives.
      paintThreadBadge(badge, own, replies, pending === 0 && unansweredRoot(message, reported, entry.held, exact));
      const gather = childByClass(row, "thread-replies");
      if (gather && message.thread!.is_root) {
        const { count, exact } = deps.gatherCount(own, message, entry.held);
        const words = deps.threadCount(count, exact);
        if (gather.textContent !== words) gather.textContent = words;
      }
    }
    // `#299 all-thread-strip`. In All, the strip in its thread's colour, by the rule the tag just drew:
    // a thread somebody has answered, whose tag is its coloured Thread(N). Nobody's thread, no strip.
    const striped = inAll && own !== null && badge !== null && !badge.hidden &&
      badge.getAttribute("data-thread-state") === "open";
    deps.paintThreadStrip(row, striped ? own : null);
    // The thread the root heads by the store's record of it (`rootReplyState`), as the read filters
    // decide by — not only by the row's own copy, which may have none: a message drawn before its first
    // reply, whose thread a summary from the Threads list knows of. Hide read keeps that root, and it
    // shows why. `#222 unread-replies`.
    const state = inMain ? rootReplyState(message, tally) : null;
    const id = state && state.id !== null && (own === null || state.id === own) ? state.id : null;
    const [unread, unseen] = id !== null && state ? [state.unread, state.unseen] : [0, 0];
    let chip = childByClass(row, "thread-unread");
    if (id === null || (unread === 0 && unseen === 0)) {
      if (row.hasAttribute("data-unread-replies")) row.removeAttribute("data-unread-replies");
      if (chip && !chip.hidden) chip.hidden = true;
      return;
    }
    row.setAttribute("data-unread-replies", String(unread || unseen));
    if (!chip) {
      chip = document.createElement("button");
      chip.className = "thread-unread";
      chip.setAttribute("type", "button");
      chip.setAttribute("title", "Open this thread at its first unread reply");
      chip.addEventListener("click", deps.inThisColumn(() => deps.guardQuietly(() => deps.openThread(id))()));
      // After N replies, the last thing on the row, so the two read as one line: "12 replies  3 unread".
      row.append(chip);
    }
    // What is known first: the unread replies held, a floor when there may be more. Only when none is
    // held, the replies reported but not yet brought, "1 new": not known to be unread, but nothing has
    // said they are read either. In words for a screen reader: "3+" is a glyph, read "three plus".
    const entry = tally.threads.get(id);
    const floor = repliesPartlyHeld(id, message, entry ? entry.held : 0, tally);
    const replyWord = (count: number) => (count === 1 ? "reply" : "replies");
    const [words, label] = unread > 0
      ? [`${unread}${floor ? "+" : ""} unread`, `${floor ? "at least " : ""}${unread} unread ${replyWord(unread)}`]
      : [`${unseen} new`, `${unseen} new ${replyWord(unseen)} not loaded yet`];
    if (chip.textContent !== words) {
      chip.textContent = words;
      chip.setAttribute("aria-label", `Open this thread at its first unread reply, ${label}`);
    }
    if (chip.hidden) chip.hidden = false;
  }

  // A thread's strip in All, and selecting a thread there, are web/src/view/channel's now

  // --- how many replies a message has, and the way down to them ----------------------------------
  //
  // `#228 reply-visible`. The owner, 2026-10-09: "When I press Reply on a message and write a reply it
  // gets marked as read. Fine. But there's still a button that says Reply ... I would like the button
  // to change to say Reply (1), Reply (2) if there are already replies so that it is clear that it is
  // already replied to." And: "a blue circle about the size of our arrow head centered on the lower
  // left corner of the message box ... the other end of the disconnected arrow pointing up/left from
  // the reply lower in the view. If we click it we should jump to the child, just as clicking the
  // arrow jumps to the parent."
  //
  // WHAT IS COUNTED. A reply by POINTER — a message whose `reply_to` names this one: Discord's
  // replies, and Google Chat's quote-replies, which arrive as the same pointer — that the page holds:
  // every message in the channel's store, which every view reads into (Main, All, each thread, the
  // stream), and every row of the list, each counted once, by id. The owner's own replies count, which
  // is the point. A combined row counts the replies to any message it stands for, as `data-replied`
  // dims it for any, but not one of its own messages answering another of its own. A thread's replies
  // that name no message are not counted: Thread(N) and N replies already say how many the thread
  // has, and Reply (N) beside them would be a second number for another kind of reply, a centimetre
  // away. One in a thread that does name this message is a reply by pointer like any other.
  //
  // SLACK HAS NO POINTER. Its only reply is a thread reply, and its Reply posts into the message's
  // thread, which the thread chips count. Since `#934 voice-page-reply-parent-arrow` the server names
  // what a reply it posted answers there too (its own record, and our metadata in Slack), so the reply
  // draws its arrow; but a reply naming the ROOT of the thread it is in is that thread's reply, which the
  // thread chips already count, and is not counted again as Reply (N), on any provider. One naming
  // another message of its thread is a reply by pointer like any other.
  //
  // THE OWNER'S OWN SEND COUNTS AT ONCE, from the outbox, the moment Send is pressed — where the
  // channel's replies are known to carry the pointer: a provider without threads, whose only reply is
  // one, or a channel in which the page already holds a message naming another. Nothing else tells the
  // page before the server answers whether a send will carry it; on Slack it will not, and counting it
  // there would say (1) for the second the send took and then go back to nothing. Once the server has
  // acknowledged a send, its answer says: a first part that came back naming the message is that
  // reply, and the same reply as the row the acknowledgement draws, so it is counted once. A send
  // waiting to go again on its own (a retry hold, `retryPending`) is still being sent, and still counts. One that
  // failed for good posted nothing; one whose delivery is unconfirmed, and that nothing is retrying,
  // is a reply only once a read brings it.
  //
  // THE CIRCLE is a button on the row's lower-left corner, the size of the disconnected arrow's head
  // and in the arrow's blue, placed by web/voice.css against the row itself: no rectangle is read to
  // draw it, and it adds nothing to the one layout the arrows take (`measureReplyArrows`). It is on a
  // row with a reply the reader can be taken to that is NOT joined to the row already:
  //
  //   * A REPLY DIRECTLY BELOW, whose arrow is the one that reaches up and meets this row, is joined
  //     to it: that arrow's head meets this row at its foot, or at its side just above the foot, a
  //     few pixels from where the circle would be, and two marks there for one connection read as two
  //     things. Photographed both ways at 412 and 1280 pixels: on a desk the head and a circle all but
  //     touched. A row whose only replies are those has no circle; one with another reply anywhere
  //     else has — the owner's "other end of the disconnected arrow".
  //   * BOTH, a reply directly below and another further away: the circle is drawn, for the other,
  //     and it and the joined reply's arrow step aside for each other at the corner they share
  //     (`data-reply-hub`, `data-reply-lift`, web/voice.css), so neither is drawn over the other and
  //     a press on either is that one's.
  //   * GATHERED (`#215 reply-coalesce`), the conversation is the stack under its parent, joined by
  //     the bridge: neither the parent nor a reply in the stack has a circle.
  //   * One the reader CAN be taken to: on a row of this list, shown, filtered out by the search or
  //     collapsed into a run of read messages; or, from Main or a thread, a message another view
  //     shows, which All draws — unless Hide read leaves it out there as well. A circle whose every tap
  //     answered "Hide read is hiding that message" would be a control that looks broken. The button
  //     still counts that reply: it was written.
  //
  // A TAP goes to the reply the way a tap on an arrow goes to what it answers (`seekMessage`): All is
  // opened for a reply this view does not show, a collapsed run is opened and a search that hides it
  // closed, and the reply lands at the head of the list, lit. With several, the first tap goes to the
  // earliest reply NOT joined to the row — the one the circle is there for, rather than the one already
  // in view under it — and each tap after that to the next in time order, round to the first after the
  // last. Which comes next is kept by the row's message (`replyMarkerVisited`), so a redraw does not
  // start it again, and the circle's name says where the next tap goes.
  //
  // NOT IN THE PINNED LIST, which is drawn without the reply gutter (`#206 pin-message`): there Reply
  // is "Reply", and there is no circle.

  /** Reply's tooltip on a message nobody has answered. No channel data, for the reason in `discordNode`. */
  const REPLY_BUTTON_TITLE = "Reply to this message";

  /** By the id of a row's first message, the reply its circle last took the reader to. */
  const replyMarkerVisited = new Map();

  /** What each circle can take the reader to: the replies' ids in time order, and which are joined. */
  const replyMarkerTargets = new WeakMap();

  // Whether any Reply on the list says a count, or any row has a circle: a pass then has something to undo. Kept per column: `deps.replyArrows().countsDrawn` (`freshReplyArrows`, `#624 voice-page-modularity`).

  /**
   * The replies by pointer the page holds of the channel on screen, as `replies`: by the id of the
   * message each answers, each reply's id to its time in milliseconds (NaN when it has none). And as
   * `held`, every message the page holds of it, by id. See the head of this section.
   *
   * @param {any[]} rows the channel list's rows
   * @returns {{replies: Map<string, Map<string, number>>, held: Map<string, any>}}
   */
  function heldReplies(rows: readonly Row[]) {
    const viewing = String(deps.selectedChannelId());
    /** @type {Map<string, any>} */
    const held = new Map();
    if (deps.providerFacts.threading() && deps.channelStore().current().channel === viewing) {
      for (const message of deps.channelStore().current().messages) held.set(String(message.id), message);
    }
    for (const row of rows) {
      for (const message of deps.rowMessages(row)) {
        if (!held.has(String(message.id))) held.set(String(message.id), message);
      }
    }
    /** @type {Map<string, Map<string, number>>} */
    const replies = new Map();
    const add = (parent: string, child: string, at: number) => {
      if (!parent || parent === child) return;
      if (!replies.has(parent)) replies.set(parent, new Map());
      replies.get(parent).set(child, at);
    };
    // ...and what each says, so that a send the stream has already brought back, ahead of the
    // server's answer to it, is not counted a second time from the deps.outbox().
    const said = new Set();
    // `#934 voice-page-reply-parent-arrow`. A reply naming the root of the thread it is in: the thread's chips count it.
    const answersOwnRoot = (message: { reply_to?: unknown; thread?: { is_root?: unknown; root_message_id?: unknown } | null } | null | undefined) => Boolean(message && message.reply_to && message.thread &&
      message.thread!.is_root !== true && String(message.thread.root_message_id || "") === String(message.reply_to));
    for (const [id, message] of held) {
      if (!message.reply_to || answersOwnRoot(message)) continue;
      add(String(message.reply_to), id, deps.timeOf(message, "timestamp"));
      said.add(`${message.reply_to}\n${String(message.content || "").trim()}`);
    }
    const pointers = !deps.providerFacts.threading() || replies.size > 0;
    // Counted before the server answers only where the send stays a reply by pointer: with threads, a reply
    // to a message in no thread, or to a thread's root, may become that thread's reply (Slack's Reply does),
    // which is not counted; it is counted, where it is one, once the server's answer says so.
    const staysPointer = (id: string) => {
      if (!deps.providerFacts.threading()) return true;
      const target = held.get(String(id));
      return Boolean(target && target.thread && target.thread.is_root !== true);
    };
    for (const entry of deps.outbox().values()) {
      if (entry.channel !== viewing || !entry.replyTo) continue;
      const first = entry.parts[0];
      if (first) {
        if (first.reply_to && !answersOwnRoot(first)) add(String(first.reply_to), String(first.id), deps.timeOf(first, "timestamp"));
      } else if ((entry.state === "sending" || deps.outboxDispatch().retryPending(entry.id)) && entry.postedCount === 0 &&
          pointers && staysPointer(entry.replyTo) && !said.has(`${entry.replyTo}\n${String(entry.text).trim()}`)) {
        add(String(entry.replyTo), `outgoing:${entry.id}`, entry.createdAt);
      }
    }
    return { replies, held };
  }


  // `outgoingReplyKey` as it stood when the Reply counts were last drawn. Kept per column: `deps.listState().outgoingKey` (`freshListState`, `#624 voice-page-modularity`).

  /** Say on a row's Reply how many replies it has: "Reply (2)", and in words to a screen reader. */
  function labelReplyButton(button: ReplyButton | null | undefined, count: number) {
    if (!button) return;
    button.replyCount = count;
    drawReplyLabel(button);
  }

  /**
   * A row's Reply as it stands: how many replies it has (`labelReplyButton`), and whether a press now
   * quotes the text selected in its message, as "Quote reply" (`#267 quote-reply`). Writes only what
   * changed.
   *
   * `#267 quote-reply`. QUOTE REPLY NEVER MOVES THE MESSAGE'S WORDS. It is wider than Reply, and where
   * the header line has no room left for that it would wrap, and the words under it would drop a line
   * at the very moment the reader starts selecting them, under their finger on a phone. So the full
   * label is tried once, as the selection begins (`quoteLabelFits`), and kept only where nothing on the
   * header line moved; anywhere else the button keeps the width it had as Reply and reads "Quote", in
   * the same accent, its name still "Quote reply".
   */
  function drawReplyLabel(button: ReplyButton) {
    const count = Number(button.replyCount) || 0;
    const held = deps.quoteOwner().held();
    const quoting = held !== null && held.button === button;
    const counted = (word: string) => (count > 0 ? `${word} (${count})` : word);
    const plain = count > 0 ? `Reply (${count})` : "Reply";
    let short = false;
    if (quoting) {
      const full = counted("Quote reply");
      if (button.quoteFor !== full) {
        button.quoteFor = full;
        button.quoteShort = !deps.quoteLabelFits(button, plain, full);
      }
      short = button.quoteShort === true;
    } else {
      button.quoteFor = undefined;
    }
    const word = quoting ? "Quote reply" : "Reply";
    const text = quoting ? counted(short ? "Quote" : word) : plain;
    if (button.textContent !== text) button.textContent = text;
    const state = quoting ? (short ? "short" : "true") : null;
    if (button.getAttribute("data-quote") !== state) {
      if (state === null) button.removeAttribute("data-quote");
      else button.setAttribute("data-quote", state);
    }
    const width = short ? String(button.quoteWidth || "") : "";
    if ((button.style.width || "") !== width) button.style.width = width;
    const lead = quoting ? QUOTE_REPLY_TITLE : REPLY_BUTTON_TITLE;
    if (count > 0) {
      const name = `${word}, ${countOf(count)} already`;
      if (button.getAttribute("aria-label") !== name) button.setAttribute("aria-label", name);
      const title = `${lead}. It has ${countOf(count)} already.`;
      if (button.getAttribute("title") !== title) button.setAttribute("title", title);
      return;
    }
    // Named by its words, except where they are cut to "Quote": its name is still all of it.
    if (short) {
      if (button.getAttribute("aria-label") !== word) button.setAttribute("aria-label", word);
    } else if (button.hasAttribute("aria-label")) {
      button.removeAttribute("aria-label");
    }
    if (button.getAttribute("title") !== lead) button.setAttribute("title", lead);
  }

  /**
   * Every row's Reply count, and its circle, from the replies the page holds and the arrows just
   * decided. Run by `renderReplyLinks`, which every pass over the rows and every filter ends with,
   * and by `renderOutgoingMessages`, for a send of the owner's. Writes only what changed, and reads
   * no layout. See the head of this section.
   */
  function renderReplyCounts() {
    deps.listState().outgoingKey = deps.outgoingReplyKey();
    const rows = [...deps.el("discord-log").children] as Row[];
    const { replies, held } = heldReplies(rows);
    if (replies.size === 0 && !deps.replyArrows().countsDrawn) return;
    /** @type {Map<string, any>} */
    const drawn = new Map();
    for (const row of rows) {
      for (const id of deps.idsOf(row)) drawn.set(id, row);
    }
    // From Main or a thread, a reply this view does not show is one All does, unless Hide read leaves
    // it out there too: by the store's read state, as the unread counts go (`replyTally`).
    const elsewhere = deps.providerFacts.threading() && deps.selection().view !== "flat";
    const reachable = (id: string) => {
      if (drawn.has(id)) return true;
      const message = held.get(id);
      return elsewhere && Boolean(message) && !deps.inView(message, deps.selection().view, deps.selection().threadId) &&
        !(deps.hidingRead() && !deps.stillToDo(message, deps.channelStore().current().dismissed));
    };
    // Drawn directly under the row it answers, at the head of its own row, with the arrow that meets it.
    const joined = (id: string) => {
      const row = drawn.get(id);
      const arrow = row ? row.replyArrowButton : null;
      return Boolean(arrow) && deps.rowShown(row) && deps.idsOf(row)[0] === id &&
        row.getAttribute("data-read-run") !== "head" && arrow.getAttribute("data-reply-style") === "adjacent";
    };
    let shown = false;
    // The replies joined to a row with a circle, whose arrows arrive at that row's corner too.
    const hubs = new Set();
    for (const row of rows) {
      const ids = deps.idsOf(row);
      const own = new Set(ids);
      /** @type {Map<string, number>} */
      const answers = new Map();
      for (const id of ids) {
        for (const [child, at] of replies.get(id) || []) {
          if (!own.has(child)) answers.set(child, at);
        }
      }
      labelReplyButton(row.replyButton, answers.size);
      if (answers.size > 0) shown = true;
      const gathered = row.hasAttribute("data-stack-under") || row.getAttribute("data-coalesce") === "parent";
      const order = (at: number) => (Number.isFinite(at) ? at : Infinity);
      const targets = gathered ? [] : [...answers].filter(([child]) => reachable(child))
        .sort((a, b) => order(a[1]) - order(b[1])).map(([child]) => child);
      const together = new Set(targets.filter(joined));
      let marker = row.replyMarker || null;
      if (targets.length > together.size) {
        if (!marker) marker = replyMarker(row);
        replyMarkerTargets.set(marker, { parent: ids[0], ids: targets, joined: together });
        labelReplyMarker(marker);
        if (row.getAttribute("data-reply-marker") !== "true") row.setAttribute("data-reply-marker", "true");
        for (const child of together) hubs.add(child);
        shown = true;
      } else if (marker) {
        row.removeChild(marker);
        row.replyMarker = null;
        row.removeAttribute("data-reply-marker");
      }
      // Lifted only beside a joined reply's arrow, as `measureReplyArrows` decides.
      if (!(targets.length > together.size && together.size > 0) && row.hasAttribute("data-reply-lift")) {
        row.removeAttribute("data-reply-lift");
      }
    }
    // Each joined arrow knows whether a circle shares its corner, and steps aside for it (web/voice.css).
    for (const row of rows) {
      const arrow = row.replyArrowButton;
      if (!arrow) continue;
      const hub = hubs.has(deps.idsOf(row)[0]);
      if (hub !== arrow.hasAttribute("data-reply-hub")) {
        if (hub) arrow.setAttribute("data-reply-hub", "true");
        else arrow.removeAttribute("data-reply-hub");
      }
    }
    deps.replyArrows().countsDrawn = shown;
  }

  /** Which of a circle's replies the next tap goes to, as an index into them. */
  function nextReplyTarget(marker: HTMLElement) {
    const held = replyMarkerTargets.get(marker);
    if (!held || held.ids.length === 0) return -1;
    const last = replyMarkerVisited.get(held.parent);
    const at = last === undefined ? -1 : held.ids.indexOf(last);
    if (at >= 0) return (at + 1) % held.ids.length;
    const first = held.ids.findIndex((id: string) => !held.joined.has(id));
    return first < 0 ? 0 : first;
  }

  /** Name a circle for where its next tap goes, and how many replies there are to go to. */
  function labelReplyMarker(marker: HTMLElement) {
    const held = replyMarkerTargets.get(marker);
    const next = nextReplyTarget(marker);
    if (!held || next < 0) return;
    const name = replyMarkerName(next + 1, held.ids.length);
    if (marker.getAttribute("aria-label") !== name) marker.setAttribute("aria-label", name);
    const title = held.ids.length === 1 ? name : `${name}. Each tap goes on to the next.`;
    if (marker.getAttribute("title") !== title) marker.setAttribute("title", title);
  }

  /**
   * The circle, at the end of `row`, drawn by web/voice.css on its lower-left corner. A button, so the
   * keyboard reaches it after the row's own controls, with a ring when it does.
   */
  function replyMarker(row: Row) {
    const marker = document.createElement("button");
    marker.className = "reply-marker";
    marker.setAttribute("type", "button");
    marker.addEventListener("click", deps.inThisColumn((event) => {
      // The row's own tap would fold it or read it aloud: this tap is the circle's.
      if (event && typeof (event as { stopPropagation?: unknown }).stopPropagation === "function") (event as { stopPropagation(): void }).stopPropagation();
      const held = replyMarkerTargets.get(marker);
      const next = nextReplyTarget(marker);
      if (!held || next < 0) return;
      const target = held.ids[next];
      replyMarkerVisited.set(held.parent, target);
      labelReplyMarker(marker);
      deps.guardQuietly(() => jumpToReply(target, next + 1, held.ids.length))();
    }));
    row.append(marker);
    row.replyMarker = marker;
    return marker;
  }

  /** A tap on a circle: the walk the arrow's jump makes, down to reply `target`, the `at`th of `of`. */
  async function jumpToReply(target: string, at: number, of: number) {
    await deps.seekMessage(target, () => landOnReply(target, at, of),
      () => deps.providerFacts.threading() && deps.selection().view !== "flat" && deps.answeredElsewhere(target));
  }

  /** Take the reader to reply `target` if a row on screen holds it, as `landOnAnswered` does; else false. */
  function landOnReply(target: string, at: number, of: number) {
    const row = deps.rowHolding(target);
    if (!row) return false;
    if (String(row.className).split(/\s+/).includes("search-hidden")) deps.setSearchOpen(false);
    deps.openReadRun(row);
    deps.revealRow(row);
    deps.renderScrollTools();
    deps.setStatus(of === 1 ? "Here is the reply to it." : `Here is reply ${at} of ${of} to it.`);
    return true;
  }


  return {
    REPLY_BUTTON_TITLE, unansweredRoot, paintThreadBadge, startThread, unansweredThread,
    heldUnansweredRoot, replyTally, rootThreadOf, unseenReplies, threadOutrunsStore,
    storeLacksReplies, repliesPartlyHeld, rootReplyState, holdsUnreadReplies, readFilterKeeps,
    unpackFilter, drawThreadChips, heldReplies, labelReplyButton, drawReplyLabel,
    renderReplyCounts, nextReplyTarget, labelReplyMarker, replyMarker, jumpToReply, landOnReply,
  };
}

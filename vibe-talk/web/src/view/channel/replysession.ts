// The reply session: a reply open in the stream, from where it goes to putting it away
// (`#410 inline-reply-editor`, `#185 reply-new-thread`, `#1052 post-to-main`, `#846
// draft-chip-honest`, `#267 quote-reply`, `#314 reply-focus-flake`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel for the plan's step 17 move 4): the route, the
// drafts and their chip, the quoting glue, opening, closing and sending, the box docked in the
// stream, and the offer that follows the store. The stores, the columns, the channel's facts and
// the send itself are handed in; `queueOutgoingMessage`'s signature and request shape are the
// boundary the send service meets. The words, the order and the timing are the page's as they were.

import type { Message } from "../../contract/index.ts";
import { createReplyState } from "./replystate.ts";
import type { ReplyRouting, ReplyState } from "./replystate.ts";

/** A column, as the reply session reads it. */
export interface ReplyColumnLike {
  readonly name: string;
  readonly mode?: string;
  readonly composer: { readonly key: string };
  readonly [key: string]: unknown;
}

/** A message, as the reply session reads it: the contract's, with its thread read loosely. */
export type ReplyMessageLike = Message & { readonly thread?: { is_root?: boolean } | null };

/** The reply's draft store, as the session reads it (core/outbox owns it). */
export interface ReplyDraftStoreLike {
  get(id: string): string | undefined;
  set(id: string, text: string): void;
  delete(id: string): void;
  has(id: string): boolean;
  ids(): string[];
  startsThread(id: string): boolean;
  setStartsThread(id: string, on: boolean): void;
  save(): boolean;
  load(): void;
}

/** What the reply session asks of the page. */
export interface ReplySessionDependencies {
  el(id: string): HTMLElement & { checked: boolean; value: string; setSelectionRange(start: number, end: number): void };
  leftEl(id: string): HTMLElement & { checked: boolean; value: string; setSelectionRange(start: number, end: number): void };
  mainColumn: ReplyColumnLike;
  columns: { main: ReplyColumnLike; side: ReplyColumnLike };
  columnWork: { active(): ReplyColumnLike; side(): boolean };
  inColumn<T>(column: ReplyColumnLike, act: () => T): T;
  eachColumn(act: (column: ReplyColumnLike) => void): void;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  screens: { screen(): string; view(): string };
  selection(): { view: string; threadId?: string | null; thread?: unknown };
  selectedChannelId(): string;
  channelStore: { current(): { scope?: string; threads?: Map<string, { id?: unknown }>; unplaced: Map<string, unknown>; messages: ReplyMessageLike[] } };
  providerFacts: { threading(): boolean };
  threadOf(message: ReplyMessageLike): string | null;
  heldThreadRecord(message: ReplyMessageLike): { id?: unknown; is_root?: boolean } | null;
  unansweredThread(message: ReplyMessageLike, record: { id?: unknown } | null): boolean;
  channelRepliesInThread(channel: string): boolean;
  removedUpstream(id?: unknown): boolean;
  threadReplies(thread: string, message: ReplyMessageLike): { count: number };
  openThreadRootId(): string | null;
  knownChannel(id?: string | null): { writable?: boolean } | null;
  setStatus(text: string): void;
  setTextIfChanged(node: HTMLElement, text: string): void;
  setHiddenIfChanged(node: HTMLElement, hidden: boolean): void;
  replyDrafts: ReplyDraftStoreLike;
  draftLedger: { has(kind: string, key: string): boolean; touch(kind: string, key: string, channel: string): void; drop(kind: string, key: string): void; save(): void };
  draftQueries: { draftsHere(): { kind: string; key: string; channel?: string }[]; draftHeld(draft: { kind: string; key: string }): boolean; draftLive(draft: { kind: string; key: string }): unknown };
  draftRound: { next(drafts: { kind: string; key: string }[], channel: string, offScreen: (draft: { kind: string; key: string }) => boolean): { kind: string; key: string } | null };
  replySight: { keep(seen: boolean): void; crossed(now: boolean): boolean };
  quoteOwner(): {
    quickDraftMessage(key: string): string;
    quickReplyOpenKey(): string | null;
    openQuickDraft(key: string, row: HTMLElement): void;
    openSentenceReply(row: HTMLElement, id: string, text: string, box: { top: number; bottom: number; left: number; right: number } | null): void;
    quickReplyOpen(): boolean;
    closeQuickReply(): void;
  };
  dictation: { stop(): void; quietKeyboard(): void; start(): void; showKeyboard(): void };
  sideColumnUp(): boolean;
  queueOutgoingMessage(entry: Record<string, unknown>, column?: ReplyColumnLike | null): Promise<unknown>;
  replyMarksRead(target: ReplyMessageLike, text: string): string | null;
  quoteBlock(quote: string): string;
  fitGrowingBox(box: HTMLElement): void;
  scheduleBackSync(): void;
  markCardBesideReply(): void;
  gatherState(): { gathered: { byReply?: boolean; parent?: string; column?: ReplyColumnLike; context?: string; thread?: string | null } | null };
  gatheredParent(): unknown;
  putRepliesBack(): void;
  rowsChosen(): unknown;
  renderChannelRows(): void;
  renderScrollTools(): void;
  renderOutgoingEverywhere(): void;
  renderOutgoingMessages(): void;
  outbox: { storageOkay(): boolean; values(): Iterable<{ inPlace?: unknown }> };
  openThread(id: string, opts?: unknown): void;
  closeThread(): void;
  jumpToMessageLink(ref: unknown): Promise<unknown>;
  focusAtEnd(box: HTMLElement): void;
  focusList(pane: unknown): void;
  followFoot(area: HTMLElement): void;
  atBottom(area: HTMLElement): boolean;
  restoreScroll(mark: unknown): void;
  captureScroll(): unknown;
  shownNode(node: Element): boolean;
  sideShowing(): boolean;
  pinsView(): { only: boolean };
  channelContexts: { values(): Iterable<{ rows: Iterable<HTMLElement> }> };
  rowMessages(li: Element): ReplyMessageLike[];
  idsOf(row: Element): string[];
  channelPane: unknown;
  sidePane: unknown;
  rowShown(row: HTMLElement): boolean;
  paneRowFor(pane: unknown, id: string): HTMLElement | null;
  nodeWithin(area: HTMLElement, node: Element | null): boolean;
  paneColumn(pane: unknown): unknown;
  layoutShown: { columns(): boolean };
  gatherReport(id: string): { gathered: number };
  channelContextKey(): string;
  focusLost(): boolean;
  isComposer(node: Element | null): boolean;
  repliesTo(id: string, messages: ReplyMessageLike[]): Set<string>;
  rowIn(list: HTMLElement, id: string): HTMLElement | null;
}

/** The reply session, as the page calls it. */
export interface ReplySession {
  readonly replyState: ReplyState<ReplyColumnLike, ReplyMessageLike>;
  replyRoute(message: ReplyMessageLike, bound?: boolean): ReplyRouting;
  replyBoundForThread(): boolean;
  rememberDraft(): void;
  draftMarker(messages: ReplyMessageLike[], li: Element): HTMLButtonElement | null;
  renderDraftMarkers(): void;
  noteDraft(kind: string, key: string, channel: string, written: boolean): void;
  inSight(node: HTMLElement, area: HTMLElement | null): boolean;
  renderDraftChip(): void;
  openSentenceReply(id: string, text: string, box: { top: number; bottom: number; left: number; right: number } | null): void;
  pressDraftChip(): Promise<void>;
  cancelQuickReply(): void;
  answerQuickReplyDiscard(discard: boolean): void;
  quickReplyToStream(row: HTMLElement, message: ReplyMessageLike, text: string): void;
  openReply(messages: ReplyMessageLike[], from?: HTMLElement | null, quote?: string, focus?: boolean): void;
  renderReplyDestination(): void;
  closeReply(how?: { restore?: boolean; refocus?: boolean }): void;
  cancelReply(): void;
  answerReplyDiscard(discard: boolean): void;
  sendReply(): Promise<void>;
  replyPane(): { scroller(): HTMLElement; head(): number; foot(): number };
  replyKeysHere(): boolean;
  placeInlineReply(): void;
  placeRowsInPlaceAgain(): void;
  fitInlineRoom(): void;
  reopenPendingReply(): void;
  noteReplyFocusLeaving(event: FocusEvent): void;
  putDockedReplyAway(): void;
  replyingTo(message: ReplyMessageLike): boolean;
  markReplying(row: HTMLElement, on: boolean): void;
  followReplyStore(): void;
  anchorUnder(list: HTMLElement, id: string, message?: ReplyMessageLike | null): HTMLElement | null;
  readonly replySight: { keep(seen: boolean): void; crossed(now: boolean): boolean };
}

/** Make the reply session from the page's stores, columns and acts, handed in as they stand. */
export function createReplySession(deps: ReplySessionDependencies): ReplySession {
  const replyState = createReplyState<ReplyColumnLike, ReplyMessageLike>(deps.mainColumn);
  /** The note under a reply bound for a message's thread that has been answered since (`replyRoute`). */
  const REPLY_INTO_ANSWERED_THREAD =
    "Posts in this message's thread, as you chose. It has a reply already, so yours will not be the first.";

  /** The note under a reply to a message removed upstream, which Send refuses (`replyRoute`). `#315`. */
  const REPLY_TO_REMOVED =
    "This message was removed from the channel, so a reply to it cannot be posted. Nothing will be sent.";

  /**
   * Where a reply to `message` goes, from the view on screen. `#185 reply-new-thread`.
   *
   * WHERE THE MESSAGE IS, not where the reader happens to be looking from. Inside a thread, into
   * that thread. A thread reply seen anywhere else — All, above all — into ITS thread: on
   * 2026-10-04 a reply from All went to the main channel because the message's live copy carried no
   * thread record. A message on the main channel, thread root or not, onto the main channel as a
   * reply to it, which the provider shows as a quote or a reply link (Slack has neither, and its
   * server side threads such a reply instead). A channel registered as one conversation keeps every
   * reply in that conversation, as it always did.
   *
   * `branch` is the thread a reply could START instead: only for a main-channel message the provider
   * has already named an empty thread for, which is what makes "start a new thread here" something
   * the server can do today (every Google Chat message is the root of such a thread). A root whose
   * thread already has replies has nothing new to branch. `unknown` is a message whose thread no
   * read has stated, which the send settles before it posts — see `placeReplyTarget`.
   *
   * `into` is that same thread once somebody has answered it, for a reply the reader had already
   * bound for it (`bound`: the box ticked, or a draft kept with its tick): the box goes, as it would
   * for the thread on opening, and the reply still goes where the reader sent it — into the thread,
   * as a reply in it, saying so — not to the main channel without a word. `#185 reply-new-thread`.
   *
   * `removed` is a message removed upstream (`noteRemoved`), which the store may hold until the read
   * behind the removal lands: no reply to it goes anywhere, and Send says why (`sendReply`). It is
   * `unknown` too, so a reply queued for it before the removal came is refused where it is placed
   * (`placeReplyTarget`). `#315 removed-reply-target`: its bare copy used to read as a message on the
   * main channel, and a reply to it went there.
   *
   * @param {object} message
   * @param {boolean} [bound] the reader has chosen to start this message's thread with the reply
   */
  function replyRoute(message: ReplyMessageLike, bound = false): ReplyRouting {
    /** @type {{ thread: string | null, branch: string | null, into: string | null, unknown: boolean, removed: boolean, note: string }} */
    const route = { thread: null, branch: null, into: null, unknown: false, removed: false, note: "" };
    if (!deps.providerFacts.threading()) return { ...route, thread: deps.threadOf(message) };
    if (deps.removedUpstream(message.id)) return { ...route, unknown: true, removed: true, note: REPLY_TO_REMOVED };
    if (deps.selection().view === "thread" && deps.selection().threadId) {
      return { ...route, thread: String(deps.selection().threadId), note: "Posts in this thread." };
    }
    const record = deps.heldThreadRecord(message);
    const scope = deps.channelStore.current().scope;
    if (scope) return { ...route, thread: record && String(record.id) === scope ? scope : null };
    if (record && record.is_root !== true) {
      return { ...route, thread: String(record.id), note: "Posts in the thread this message belongs to." };
    }
    // Only a thread nobody has answered, by the rule the grey Thread tag goes by (`deps.unansweredThread`):
    // ticking the box on a thread that has replies after all would post into it while saying it starts
    // one. And not where a reply IS a thread reply (Slack, `deps.channelRepliesInThread`): there the reply
    // starts the message's thread already, and a box whose two states post the same thing would say
    // one of them goes to Main.
    if (record) {
      if (deps.channelRepliesInThread(deps.selectedChannelId())) return route;
      if (deps.unansweredThread(message, record)) return { ...route, branch: String(record.id) };
      return bound ? { ...route, into: String(record.id), note: REPLY_INTO_ANSWERED_THREAD } : route;
    }
    if (!deps.channelStore.current().unplaced.has(String(message.id))) return route;
    return {
      ...route, unknown: true,
      note: "This message came in live and the channel has not been read since, so whether it is " +
        "in a thread is not known yet. Send finds out first, then posts your reply where it is.",
    };
  }

  const REPLY_DRAFT_REFUSED =
    "This browser refused to store the draft, so it will be lost if you reload (private " +
    "browsing does this). It is still here until then.";

  /**
   * Whether the reply on screen is bound for its message's own thread by the reader's choice: the box
   * ticked, or a ticked reply whose thread has been answered since (`replyRoute`'s `into`).
   * `#185 reply-new-thread`.
   */
  function replyBoundForThread(): boolean {
    return Boolean(replyState.routing.into) || Boolean(replyState.routing.branch && deps.el("reply-branch").checked);
  }

  function rememberDraft(): void {
    if (!replyState.target) {
      return;
    }
    // By the id as a string, the key a reload reads them back under and `draftMarker` asks by.
    const id = String(replyState.target.id);
    const had = deps.replyDrafts.has(id);
    const before = deps.replyDrafts.get(id) || "";
    const text = deps.el("reply-text").value;
    if (text.trim()) {
      deps.replyDrafts.set(id, text);
    } else {
      deps.replyDrafts.delete(id);
    }
    deps.replyDrafts.setStartsThread(id, Boolean(text.trim()) && replyBoundForThread());
    // `#819 voice-page-quoted-reply-and-draft-chip`. Found again by the Draft chip; no newer for being unchanged.
    if ((deps.replyDrafts.get(id) || "") !== before) noteDraft("reply", id, String(replyState.channelId || deps.selectedChannelId() || ""), deps.replyDrafts.has(id));
    // `#193 compose-draft-loss`. The row's Draft comes and goes with the draft.
    if (deps.replyDrafts.has(id) !== had) renderDraftMarkers();
    // ...and SAY SO when the browser refuses. The read-back was here from the start and its answer
    // was thrown away, which made it a comment: a reader in private browsing was told a draft was
    // kept by the fact that nothing said otherwise, and lost it on reload. Same idiom as the reading
    // width and the microphone settings, which both report the refusal where the control is.
    //
    // On failure only. This line also carries "Not posted: …", and typing after a failed send is not
    // a reason to take the reason away.
    if (!deps.replyDrafts.save()) {
      deps.el("reply-state").textContent = REPLY_DRAFT_REFUSED;
    }
  }

  // --- a reply still being written, on its message's row ------------------------------------------
  //
  // `#193 compose-draft-loss`. A draft is kept per message, but nothing on the list said which message
  // had one: after a reload, or a sign-in that sent the reader away and back, they had to remember which
  // message they were answering. So a row with a saved reply draft carries Draft — a pencil and the word,
  // in the accent, beside Reply — which opens Reply with the draft, as Reply itself does. It goes when
  // the draft is sent or emptied. One button on every row, hidden where there is no draft, so marking a
  // row never has to find a place among its controls.

  /**
   * A row's Draft. `messages` are the row's, and the draft is its first constituent's, the one Reply
   * answers.
   *
   * Named "Draft reply" to a screen reader, which hears the button without the row beside it, and
   * never with the draft's text: the row already says what the message is.
   *
   * @param {Array<object>} messages
   * @param {HTMLLIElement} li
   */
  function draftMarker(messages: ReplyMessageLike[], li: Element): HTMLButtonElement | null {
    const row = li as HTMLElement & { draftMarker?: HTMLButtonElement | null };
    const marker = document.createElement("button");
    marker.className = "draft-marker";
    marker.setAttribute("type", "button");
    marker.setAttribute("aria-label", "Draft reply");
    marker.setAttribute("title", "You started a reply to this message and have not sent it. Open it in Reply.");
    marker.textContent = "Draft";
    marker.hidden = !deps.replyDrafts.has(String((messages[0] as ReplyMessageLike).id));
    marker.addEventListener("click", deps.inThisColumn(() => openReply(messages, li)));
    row.draftMarker = marker;
    return marker;
  }

  /**
   * Every row's Draft, after a draft was started, sent or emptied: the rows on screen in each column,
   * and the rows a view keeps for when it is shown again.
   */
  function renderDraftMarkers(): void {
    /** @type {(HTMLElement & { draftMarker?: HTMLButtonElement | null })[]} */
    const rows = [];
    deps.eachColumn(() => rows.push(.../** @type {HTMLLIElement[]} */ ([...deps.el("discord-log").children, ...deps.el("pinned-log").children])));
    for (const held of deps.channelContexts.values()) rows.push(...(held.rows || []));
    for (const row of rows) {
      const first = deps.rowMessages(row)[0];
      const marker = (row as { draftMarker?: HTMLButtonElement | null }).draftMarker;
      if (!marker || !first) continue;
      const hidden = !deps.replyDrafts.has(String(first.id));
      if (marker.hidden !== hidden) marker.hidden = hidden;
    }
  }

  // --- every draft, found again: the Draft chip ----------------------------------------------------
  //
  // `#819 voice-page-quoted-reply-and-draft-chip`. The owner lost a dictated reply: written in a quick reply,
  // closed, and nowhere to be seen. A draft kept where nothing on the screen shows it is a draft lost. So a
  // chip over the list, in the accent, says Draft whenever the channel on screen holds one anywhere: the main
  // box or a thread's, a reply in the stream, a quick reply. A tap goes to the newest and opens it, focused;
  // with several, each tap goes to the next, newest first, round to the newest again. The drafts are their
  // own stores' (`channelDrafts`, `deps.replyDrafts`, `quickDrafts`); where and when each was written is the
  // ledger's (`deps.draftLedger`), and a draft written before the ledger kept its place is found too, oldest.

  /** Which draft the chip's next tap goes to: the one after the last it went to (web/src/core/outbox `createDraftRound`). */

  /**
   * A draft was written (or emptied): kept in the ledger with its channel and the time, or let go, and the chip
   * drawn again.
   *
   * @param {import("./src/core/outbox/index.ts").DraftKind} kind
   * @param {string} key
   * @param {string} channel
   * @param {boolean} written
   */
  function noteDraft(kind: string, key: string, channel: string, written: boolean): void {
    const had = deps.draftLedger.has(kind, key);
    if (written && channel) deps.draftLedger.touch(kind, key, channel);
    else deps.draftLedger.drop(kind, key);
    deps.draftLedger.save();
    // Drawn again where a draft came or went, or was emptied (one the ledger never knew of counts too); a
    // keystroke in one already counted changes nothing shown.
    if (!written || had !== deps.draftLedger.has(kind, key)) renderDraftChip();
  }




  /**
   * Whether `node` is shown, drawn, and — given the list `area` it scrolls in — has any of itself inside what
   * that list shows. Where the page cannot measure, shown is on screen.
   *
   * @param {HTMLElement} node
   * @param {HTMLElement | null} area
   */
  function inSight(node: HTMLElement, area: HTMLElement | null): boolean {
    if (!deps.shownNode(node)) return false;
    if (typeof node.getClientRects === "function" && node.getClientRects().length === 0) return false;
    if (!area || typeof node.getBoundingClientRect !== "function" || typeof area.getBoundingClientRect !== "function") return true;
    const box = node.getBoundingClientRect();
    const frame = area.getBoundingClientRect();
    return box.bottom > frame.top && box.top < frame.bottom;
  }

  /**
   * `#846 draft-chip-honest`. Whether `draft`'s own box is on screen: the box at a column's foot that holds it,
   * its reply open in the stream and scrolled into view, or its quick reply open. The owner: "Our draft button
   * pops up even when the composer is on the screen. It should just be for jumping to it when off screen".
   *
   * @param {import("./src/core/outbox/index.ts").DraftEntry} draft
   */
  function draftOnScreen(draft: { kind: string; key: string }): boolean {
    if (draft.kind === "composer") {
      return [deps.columns.main, ...(deps.sideShowing() ? [deps.columns.side] : [])].some((column) => column.composer.key === draft.key &&
        deps.inColumn(column, () => inSight(deps.el("channel-compose-text"), null)));
    }
    if (draft.kind === "reply") {
      return Boolean(replyState.docked && replyState.target && String(replyState.target.id) === draft.key) &&
        inSight(deps.el("reply-text"), replyPane().scroller());
    }
    return deps.quoteOwner().quickReplyOpenKey() === draft.key;
  }

  /** The channel on screen's drafts whose box is not on screen, newest first: what the chip jumps to. */
  function draftsOffScreen(): { kind: string; key: string }[] {
    return deps.draftQueries.draftsHere().filter((draft) => !draftOnScreen(draft));
  }

  /**
   * Whether the reply open in the stream was in sight when the chip was last drawn: a scroll redraws the chip
   * only when that changes (`draftChipFrame`). Kept by every drawing, so it is never stale. One owner in
   * web/src/view/shell/draftsight.ts (`#624 voice-page-modularity`).
   *
   * @type {import("./src/view/shell/index.ts").ReplySight}
   */

  /**
   * The chip: up while the channel on screen holds a draft whose box is off screen, saying how many where
   * there are several (`#846 draft-chip-honest`).
   */
  function renderDraftChip(): void {
    deps.replySight.keep(Boolean(replyState.docked) && inSight(deps.el("reply-text"), replyPane().scroller()));
    const chip = deps.leftEl("draft-chip");
    const count = draftsOffScreen().length;
    const hidden = count === 0;
    if (chip.hidden !== hidden) chip.hidden = hidden;
    deps.setTextIfChanged(deps.leftEl("draft-chip-label"), count > 1 ? `Drafts (${count})` : "Draft");
    const title = count > 1
      ? `${count} replies or messages you started here and have not sent. Tap for the newest; tap again for the next.`
      : "A reply or message you started here and have not sent. Tap to open it.";
    if (chip.getAttribute("title") !== title) chip.setAttribute("title", title);
  }

  /**
   * The row drawing message `id` in a list on screen, and its column: the left column's first, then the
   * right's while it shows a thread or the cards.
   *
   * @param {string} id
   * @returns {{row: HTMLLIElement, column: ColumnFields} | null}
   */
  function draftRow(id: string): { column: ReplyColumnLike; row: HTMLElement } | null {
    for (const column of [deps.columns.main, ...(deps.sideColumnUp() ? [deps.columns.side] : [])]) {
      const row = deps.inColumn(column, () => /** @type {HTMLLIElement | undefined} */ (
        [...deps.el("discord-log").children].find((li) => !((li as HTMLElement).hidden) &&
          deps.rowMessages(li).some((message) => String(message.id) === id))));
      if (row) return { row: row as unknown as HTMLElement, column };
    }
    return null;
  }

  /**
   * Go to `draft` and open it, focused: its box, its reply under its message, or its quick reply on its
   * message; a message not drawn is jumped to first, as a link to it is.
   *
   * @param {import("./src/core/outbox/index.ts").DraftEntry} draft
   */
  async function openDraft(draft: { kind: string; key: string; channel?: string }): Promise<void> {
    if (draft.kind === "composer") {
      let thread = null;
      try {
        thread = JSON.parse(draft.key)[1] || null;
      } catch (_error) {
        thread = null;
      }
      const shows = () => [deps.columns.main, ...(deps.sideShowing() ? [deps.columns.side] : [])].find((column) => column.composer.key === draft.key) || null;
      let column = shows();
      if (!column && thread) {
        await deps.openThread(String(thread));
        column = shows();
      } else if (!column) {
        await deps.closeThread();
        column = shows();
      }
      // The box at the foot stands aside while a reply is open in the stream: that reply put down, its draft kept.
      if (column && replyState.docked) closeReply({ restore: false, refocus: false });
      if (column) deps.inColumn(column, () => deps.focusAtEnd(deps.el("channel-compose-text")));
      return;
    }
    const id = draft.kind === "reply" ? draft.key : deps.quoteOwner().quickDraftMessage(draft.key);
    let found = draftRow(id);
    if (!found) {
      await deps.jumpToMessageLink({ service: "", channel: draft.channel, message: id, thread: null });
      found = draftRow(id);
    }
    if (!found) {
      deps.setStatus("That draft's message is not in this channel's list now; it is kept until it is.");
      return;
    }
    const { row, column } = found;
    deps.inColumn(column, () => {
      row.scrollIntoView({ block: "center" });
      if (draft.kind === "reply") {
        // To the draft's own message first, where the row draws it among others: Reply answers the first.
        const messages = deps.rowMessages(row);
        const own = messages.filter((message) => String(message.id) === id);
        openReply([...own, ...messages.filter((message) => String(message.id) !== id)], row);
      }
      else deps.quoteOwner().openQuickDraft(draft.key, row);
    });
  }

  /**
   * `#821 voice-page-read-aloud-react`. React, from a paused read: the quick reply on the words it held, on the
   * row drawing their message, focused, its draft kept and found as any quick reply's; and the microphone
   * started in the box where the browser can hear (the press is the gesture it needs), else the line says to
   * use the keyboard's. The read stays paused behind it: carrying on is the reader's own tap.
   *
   * @param {string} id
   * @param {string} text
   * @param {{top: number, bottom: number, left: number, right: number} | null} box
   */
  function openSentenceReply(id: string, text: string, box: { top: number; bottom: number; left: number; right: number } | null): void {
    const found = draftRow(id);
    if (!found) {
      deps.setStatus("That message is not on screen to reply to.");
      return;
    }
    // `#829 voice-page-dictation-polish`: the keyboard kept down from before the box takes the focus. A reply
    // opened while one listens (React again) stops that listening first, whose end would give the keyboard back.
    deps.dictation.stop();
    deps.dictation.quietKeyboard();
    deps.inColumn(found.column, () => deps.quoteOwner().openSentenceReply(found.row, id, text, box));
    if (!deps.el("quick-reply").hidden) deps.dictation.start();
    else deps.dictation.showKeyboard();
  }

  /**
   * `#821 voice-page-read-aloud-react`. Speaking into the quick reply (web/src/view/composer `createDictation`):
   * what it settles on is kept as the draft, as typing is.
   */

  /** A tap on the chip: the newest draft, or the one after the last it went to. */
  async function pressDraftChip(): Promise<void> {
    // A quick reply open is put away, its words kept as its draft (not put down into the stream as a tap
    // elsewhere would): the chip goes on from it to the next.
    if (deps.quoteOwner().quickReplyOpen()) deps.quoteOwner().closeQuickReply();
    // `#846 draft-chip-honest`. Only the drafts off screen, in their round among them all: the one just opened,
    // on screen now, is passed over, and the next tap goes on from it rather than back to the newest.
    const next = deps.draftRound.next(deps.draftQueries.draftsHere(), String(deps.selectedChannelId() || ""), (draft) => !draftOnScreen(draft));
    if (!next) {
      renderDraftChip();
      return;
    }
    await openDraft(next);
  }

  // --- quoting part of a message in the reply to it ------------------------------------------------
  //
  // `#267 quote-reply`, `#726 vt-selection-chip`, `#727 vt-selection-quick-reply`. The deps.selection Quote
  // reply would quote, the Quoted reply chip under it, and the quick reply the chip opens are
  // web/src/view/channel's (`createQuoting`, with QUOTE_HOLD_MS, QUOTE_REPLY_TITLE and the chip's and box's
  // measures; `#624 voice-page-modularity`), whose head says what each does and why. Where a reply posts,
  // the outbox and the row's Reply label are here, handed to it.

  /**
   * `#846 draft-chip-honest`. Cancel on the quick reply: with words in it, "Discard this draft?" first; with
   * none, it closes at once. Esc and a tap elsewhere keep the words, as before.
   */
  function cancelQuickReply(): void {
    if (!deps.el("quick-reply-text").value.trim()) {
      deps.quoteOwner().closeQuickReply();
      return;
    }
    deps.setHiddenIfChanged(deps.leftEl("quick-reply-discard"), false);
    deps.leftEl("quick-reply-discard-no").focus({ preventScroll: true });
  }

  /**
   * The quick reply's answer to "Discard this draft?": Yes empties it and closes it, its draft gone from the
   * device and the Draft chip; No leaves it open, focused.
   *
   * @param {boolean} discard
   */
  function answerQuickReplyDiscard(discard: boolean): void {
    deps.setHiddenIfChanged(deps.leftEl("quick-reply-discard"), true);
    if (!discard) {
      if (deps.quoteOwner().quickReplyOpen()) deps.leftEl("quick-reply-text").focus({ preventScroll: true });
      return;
    }
    deps.el("quick-reply-text").value = "";
    deps.quoteOwner().closeQuickReply();
  }

  /**
   * `#727 vt-selection-quick-reply`. A quick reply put down by a tap outside it with words in it: they, under
   * their quote, become the message's reply draft, ahead of any draft it had (a blank line between), and its
   * reply opens in the stream under it, shown and open, as Reply opens it, in the column drawing it. Without
   * taking the focus: the tap was going somewhere, and the reader sees the box without being put in it.
   *
   * @param {HTMLElement} row the row the box was opened on
   * @param {any} message
   * @param {string} text the quote and what was written
   */
  function quickReplyToStream(row: HTMLElement, message: ReplyMessageLike, text: string): void {
    const id = String(message.id);
    const prior = deps.replyDrafts.get(id) || "";
    deps.replyDrafts.set(id, prior.trim() ? `${text}\n\n${prior}` : text);
    deps.replyDrafts.save();
    noteDraft("reply", id, String(deps.selectedChannelId() || ""), true);
    renderDraftMarkers();
    // The row the box was opened on, unless a redraw has replaced it since: then the one drawing the message.
    const drawn = row.parentNode && deps.rowShown(row) ? row : deps.paneRowFor(deps.channelPane, id) || deps.paneRowFor(deps.sidePane, id);
    const column = drawn && listPane(drawn) === deps.sidePane ? deps.columns.side : deps.columns.main;
    deps.inColumn(column, () => openReply(drawn ? deps.rowMessages(drawn) : [message], drawn, "", false));
  }


  /**
   * Open Reply on one specific row: its box in the stream of the column Reply was pressed in, under the
   * message and its replies (`dockReply`, `#410 inline-reply-editor`). There is no reply screen.
   *
   * THE ANSWER GOES TO THE FIRST CONSTITUENT, and the line beside "Your reply" says which id that is.
   *
   * `#247 reply-continuity`. `from` is the row Reply was pressed on, when it was pressed on one: it stays
   * where it is on the screen, and the box opens under it and its replies.
   *
   * `#267 quote-reply`. `quote` is the text selected in the message when Reply was Quote reply: the box
   * opens with it quoted, over any draft, and focused with the caret under it. `focus`: the box takes
   * the focus, as it does whenever the reader opens it; a reply the page reopens by itself
   * (`reopenPendingReply`) leaves the keys where they are.
   *
   * @param {Array<object>} messages
   * @param {HTMLElement | null} [from]
   * @param {string} [quote]
   * @param {boolean} [focus]
   */
  function openReply(messages: ReplyMessageLike[], from: Element | null = null, quote = "", focus = true): void {
    // `#407 unpack-row-parity`. A row unpacked under the cards acts in the right column, which shows no
    // thread to answer in or come back to: its reply is the left column's, drawn in the cards' list.
    if (deps.columnWork.side() && deps.columns.side.mode !== "thread") {
      deps.inColumn(deps.columns.main, () => openReply(messages, from, quote, focus));
      return;
    }
    // Only on the channel's lists, where every Reply is: with none on screen the draft waits for its row.
    if (deps.screens.screen() !== "main" || deps.screens.view() !== "discord") return;
    replyState.pending = null;
    // `#410 inline-reply-editor`. A reply already open in the stream is put down first, its words kept as
    // Cancel keeps them and what it gathered put back, and the box moves to this message: the row pressed
    // stays where it was on the screen while the first box leaves from above it.
    if (replyState.docked) {
      const list = from && from.parentNode ? /** @type {HTMLElement} */ (from.parentNode) : null;
      const id = from ? deps.idsOf(from)[0] : undefined;
      const top = list && from && deps.shownNode(from) ? from.getBoundingClientRect().top : NaN;
      closeReply({ restore: false, refocus: false });
      if (list && id !== undefined) {
        holdRowAt(listPane(list as HTMLElement) as { scroller(): HTMLElement }, list as HTMLElement, id, top);
        from = deps.rowIn(list as HTMLElement, id) || from;
      }
    }
    const message = messages[0] as ReplyMessageLike;
    replyState.target = message;
    replyState.channelId = deps.selectedChannelId();
    // `#185 reply-new-thread`. Offered only where a new thread is possible, and never carried over
    // from the last reply: the default is to answer where the message is. Only this message's own kept
    // draft brings its tick back (`deps.replyDrafts.startsThread`) — or, where its thread has been answered since,
    // the thread it was bound for (`replyRoute`'s `into`).
    // `#419 threads-view-main-messages`. A main-channel message among the Threads view's cards is answered to
    // make it a thread: bound for its new thread where the service starts one on the reply's say-so.
    const starts = Boolean(from && from.hasAttribute("data-main-row"));
    const bound = (deps.replyDrafts.has(String(message.id)) && deps.replyDrafts.startsThread(String(message.id))) || starts;
    replyState.routing = replyRoute(message, bound);
    deps.el("reply-branch").checked = Boolean(replyState.routing.branch) && bound;
    deps.el("reply-branch-row").hidden = !replyState.routing.branch;
    // `#1052 post-to-main`: offered only where this reply would join a thread, and never ticked
    // on open — no draft brings it back either, so the destination is chosen each time.
    deps.el("reply-to-main").checked = false;
    deps.el("reply-to-main-row").hidden = !(replyState.routing.thread || replyState.routing.into);
    replyState.column = deps.columnWork.active();
    deps.el("reply-target-meta").textContent = `to ${
      message.author_is_bot ? `${message.author} (bot)` : String(message.author)
    } · id ${message.id}`;
    const draft = deps.replyDrafts.get(String(message.id)) || "";
    // The quote over the draft, once: a draft that already begins with this very quote keeps one.
    const block = quote ? deps.quoteBlock(quote) : "";
    const opened = block && !draft.startsWith(block) ? `${block}${draft}` : draft;
    deps.el("reply-text").value = opened;
    replyState.quote = block ? { opened, draft } : null;
    deps.el("reply-state").textContent = "";
    // Where it posts is said first: the line it takes is part of the box's height, and the room left
    // over the box is what the row answered is kept in view in.
    renderReplyDestination();
    deps.setHiddenIfChanged(deps.el("reply-discard"), true);
    dockReply(from);
    deps.fitGrowingBox(deps.el("reply-text"));
    // `#846 draft-chip-honest`. Its draft is on screen now, and the box at the foot stands aside for it.
    renderDraftChip();
    if (!focus) return;
    // The box came up to be written in: it takes the focus, the caret under a quote or after a draft.
    // Without moving either list, which has rules of its own about where the reader is.
    const box = deps.el("reply-text");
    box.focus({ preventScroll: true });
    const caret = block ? block.length : opened.length;
    if (typeof box.setSelectionRange === "function") box.setSelectionRange(caret, caret);
  }

  /**
   * Say where Send will post, in the words of the choice the reader has made. Silent for a plain
   * main-channel reply, which is what Reply has always done and needs no note.
   */
  function renderReplyDestination(): void {
    const note = deps.el("reply-destination");
    note.textContent = !deps.el("reply-to-main-row").hidden && deps.el("reply-to-main").checked
      ? "Posts in the main channel as a new thread, as a reply to this message."
      : replyState.routing.branch
        ? deps.el("reply-branch").checked
          ? "Posts in a new thread on this message, as its first reply."
          : "Posts in the main channel as a reply to this message."
        : replyState.routing.note;
    note.hidden = note.textContent === "";
  }

  /**
   * Put the reply down, its words kept as a draft: Cancel, Back and Escape.
   *
   * `#410 inline-reply-editor`. The box leaves the stream with nothing moving (`restore`, see
   * `leaveDockedReply`), and the keys go to its list where they were in the box (`refocus`). The page
   * putting a reply away because the column changed under it — another channel, another thread, the
   * call's view — asks for neither: the reader is going elsewhere.
   *
   * @param {{restore?: boolean, refocus?: boolean}} [how]
   */
  function closeReply(how: { restore?: boolean; refocus?: boolean } = {}): void {
    const { restore = true, refocus = true } = how;
    // `#267 quote-reply`. A quote put in the box and left as it was is not a reply begun: the draft
    // that was there before it is what is kept.
    if (replyState.quote && deps.el("reply-text").value === replyState.quote.opened) deps.el("reply-text").value = replyState.quote.draft;
    replyState.quote = null;
    rememberDraft();
    const id = replyState.target ? String(replyState.target.id) : null;
    replyState.target = null;
    deps.setHiddenIfChanged(deps.el("reply-discard"), true);
    if (replyState.docked && id !== null) leaveDockedReply(id, { restore, refocus });
    // `#846 draft-chip-honest`. The draft is off screen now, and the box at the foot back.
    renderDraftChip();
  }

  /**
   * `#846 draft-chip-honest`. Cancel: put the reply away. With words it would keep as a draft, it asks first,
   * "Discard this draft?", under the box (`answerReplyDiscard`); with none, it goes at once. Esc and Back keep
   * the words (`closeReply`). The owner: Cancel asks, "and Yes really deletes it".
   */
  function cancelReply(): void {
    const text = deps.el("reply-text").value;
    // What putting it away would keep: a quote put in and left as it was keeps the draft from before it.
    const kept = replyState.quote && text === replyState.quote.opened ? replyState.quote.draft : text;
    if (!replyState.target || !kept.trim()) {
      closeReply();
      return;
    }
    deps.setHiddenIfChanged(deps.el("reply-discard"), false);
    // Into view as it takes the focus: under the box, it may open below the list's foot.
    deps.el("reply-discard-no").focus();
  }

  /**
   * The answer to "Discard this draft?": Yes empties the box and puts the reply away, so the draft is gone from
   * the page and the device (`rememberDraft`), its row's Draft mark and the Draft chip's count with it; No
   * leaves the box open as it was, focused.
   *
   * @param {boolean} discard
   */
  function answerReplyDiscard(discard: boolean): void {
    deps.setHiddenIfChanged(deps.el("reply-discard"), true);
    if (!discard) {
      if (replyState.target) deps.el("reply-text").focus({ preventScroll: true });
      return;
    }
    replyState.quote = null;
    deps.el("reply-text").value = "";
    closeReply();
  }

  /** Hand the captured target and text to the same durable deps.outbox as the normal composer. */
  async function sendReply(): Promise<void> {
    if (!replyState.target) return;
    const text = deps.el("reply-text").value;
    if (!text.trim()) {
      deps.el("reply-state").textContent = "Nothing to send yet — write something first.";
      return;
    }
    const channel = replyState.channelId;
    const target = replyState.target;
    if (deps.knownChannel(channel)?.writable === false) {
      deps.el("reply-state").textContent = "This channel is read-only.";
      return;
    }
    const route = replyState.routing;
    // `#315 removed-reply-target`. A reply to a message removed upstream goes nowhere: the words stay in
    // the box, and nothing is queued to hold up what the reader writes next in the channel.
    if (route.removed) {
      deps.el("reply-state").textContent = "Not sent: the message you are answering was removed from the channel.";
      return;
    }
    rememberDraft();
    const branch = route.branch && deps.el("reply-branch").checked ? route.branch : null;
    // `#1052 post-to-main`: ticked where offered, the reply lands in the main channel as a new
    // thread answering this message, so it draws there, not in place under it here.
    const toMain = !deps.el("reply-to-main-row").hidden && deps.el("reply-to-main").checked;
    const completion = deps.queueOutgoingMessage({
      channel, threadId: toMain ? null : branch || route.into || route.thread, replyTo: String(target.id), text,
      ...(toMain ? { postToMain: true } : {}),
      // `#467 inline-send-in-place`. Sent from the box in the stream: its row is drawn where that box was,
      // under the message it answers, until the reply itself is drawn (`placeOutgoingInPlace`).
      ...(replyState.docked && !toMain ? { inPlace: String(target.id) } : {}),
      // `#500 sent-receipts-pile-up`. And in which list: the Threads view's cards, where Main may draw the
      // same message on the left.
      ...(replyState.docked && !toMain && replyState.list === deps.leftEl("side-cards") ? { inCards: true } : {}),
      // `#300 reply-marks-read`. A targeted reply marks the message it answers, wherever it goes, unless
      // its quote covers too little of it (`#728 vt-quote-read-threshold`).
      marksRead: deps.replyMarksRead(target, text),
      // Not where to post yet: the send reads the channel first. See `placeReplyTarget`.
      ...(route.unknown ? { placement: "unknown" } : {}),
    }, replyState.docked ? replyState.column : null);
    deps.replyDrafts.delete(String(target.id));
    deps.replyDrafts.setStartsThread(String(target.id), false);
    if (deps.outbox.storageOkay()) deps.replyDrafts.save();
    noteDraft("reply", String(target.id), String(replyState.channelId || ""), false);
    renderDraftMarkers();
    replyState.quote = null;
    deps.el("reply-text").value = "";
    deps.el("reply-state").textContent = "";
    // Do not call closeReply: its draft save would overwrite the fallback retained after a storage
    // failure. The captured request owns its text now; a newer reply draft must never be touched by
    // its eventual completion.
    replyState.target = null;
    // `#410 inline-reply-editor`. Sent from the stream, nothing moves but the box, which goes: the message
    // answered turns Done where it stands (`markAnsweredDone`), which says the reply went, and the reply
    // joins what Reply gathered under it as it lands. A reply that does not go says so where the reader
    // is, the outbox row that offers Retry being down at the column's foot (`reportColumnReply`).
    if (replyState.docked) {
      leaveDockedReply(String(target.id), { sent: true });
      // `#467 inline-send-in-place`. Its row takes the place the box has just left.
      deps.renderOutgoingEverywhere();
    }
    await completion;
  }

  const columnPane = (column: ReplyColumnLike) => (column === deps.columns.side ? deps.sidePane : deps.channelPane);


  /** The lists Reply opens its box in, by their ids. */
  const REPLY_LISTS = ["discord-log", "side-log", "pinned-log", "side-pinned-log", "side-cards"];

  /** The keys' list the reply open in the stream is in: the right column's, over its thread or its cards. */
  function replyPane(): { scroller(): HTMLElement; head(): number; foot(): number } {
    return listPane(deps.leftEl("reply-compose")) as { scroller(): HTMLElement; head(): number; foot(): number };
  }

  /** The keys' list `node` is in: the right column's where it is inside its scroller, else the left's. */
  function listPane(node: HTMLElement): unknown {
    return deps.nodeWithin(deps.leftEl("side-scroll"), node) ? deps.sidePane : deps.channelPane;
  }

  /**
   * Whether the browser draws a box under its row by anchor positioning, which is what puts the reply's box
   * in the middle of the stream. Without it (or with no row drawn to put it under) the box is drawn at the
   * list's foot, where the column's own box was, and the list is not moved to show it: moving the reader
   * to the foot of the list is the jump this does without. Asked of the stylesheet's own feature query, as
   * `FIELD_SIZING` asks; the page reads no user agent.
   */
  let anchorsKnown: boolean | undefined;
  function anchorsSupported(): boolean {
    if (anchorsKnown === undefined) {
      anchorsKnown = Boolean(typeof window === "object" && window.CSS && typeof window.CSS.supports === "function" &&
        window.CSS.supports("anchor-name", "--a"));
    }
    return anchorsKnown;
  }

  /**
   * Whether the column the keys are working in holds the reply open in the stream: the column it was
   * opened in, or the one its box is drawn in, which differ for a row unpacked under the Threads view's
   * cards (`#407 unpack-row-parity`: its reply is the left column's, drawn in the right one's list).
   */
  function replyKeysHere(): boolean {
    return deps.columnWork.active() === replyState.column || (replyState.docked && deps.columnWork.active() === deps.paneColumn(replyPane()));
  }

  /** The list Reply was pressed on: the row's, where it is on one of the channel's lists; else the column's. */
  function replyListOf(from: Element | null): HTMLElement {
    const list = from && from.parentNode;
    if (list && REPLY_LISTS.includes(String((list as HTMLElement).id || ""))) return list as HTMLElement;
    return deps.el(deps.pinsView().only ? "pinned-log" : "discord-log");
  }

  /** The place in the stream for a reply opened on `list`: #side-reply in the right column, else #column-reply. */
  function replyMountFor(list: HTMLElement): HTMLElement {
    return deps.nodeWithin(deps.leftEl("side-column"), list) ? deps.leftEl("side-reply") : deps.leftEl("column-reply");
  }

  /** Whether `message` is the root of the thread the list on screen is: every row after it is its thread. */
  function rootOfOpenThread(message: ReplyMessageLike): boolean {
    if (deps.selection().view !== "thread" || !deps.selection().threadId) return false;
    const id = String(message.id);
    return id === deps.openThreadRootId() || id === String(deps.selection().threadId) ||
      (deps.threadOf(message) === String(deps.selection().threadId) && (message.thread as { is_root?: boolean } | null)?.is_root === true);
  }

  /** Whether `message` heads a thread with replies: on Main's root, Reply gathers them under it. */
  function headsAnsweredThread(message: ReplyMessageLike): boolean {
    const thread = deps.providerFacts.threading() ? deps.threadOf(message) : null;
    if (thread === null || (message.thread as { is_root?: boolean } | null)?.is_root !== true) return false;
    const said = deps.threadReplies(thread, message).count;
    return (typeof said === "number" && said > 0) ||
      deps.channelStore.current().messages.some((held) => deps.threadOf(held) === thread && String(held.id) !== String(message.id));
  }

  /**
   * Gather the replies of the message being answered under it in `list`, by the gathering All's N replies
   * makes (`#215 reply-coalesce`), in Main and a thread as well (`gathered.byReply`), and on a thread's
   * root in Main its thread's replies with them (`gathered.thread`). Says whether it did: not where they
   * are gathered already, where none is drawn, while a filter chooses rows (which puts a gathering back),
   * in the Pinned filter or under the cards, nor for a thread's own root, its whole thread being under it.
   *
   * @param {HTMLElement} list
   */
  function gatherForReply(list: HTMLElement): boolean {
    if (!replyState.target) return false;
    const id = String(replyState.target.id);
    // What a sent reply left gathered is put back by the next Reply, as N replies on another message puts
    // the first back: one gathering at a time.
    const priorGathered = deps.gatherState().gathered;
    if (priorGathered && priorGathered.byReply && priorGathered.parent !== id) deps.inColumn(priorGathered.column || deps.columnWork.active(), deps.putRepliesBack);
    if (list !== deps.el("discord-log") || deps.rowsChosen()) return false;
    if (deps.gatheredParent() === id) return false;
    if (rootOfOpenThread(replyState.target)) return false;
    // Not where the right column shows that thread already: it would be drawn twice side by side, as N
    // replies on a root in two deps.columns declines to (`toggleGathered`).
    const shownRight = deps.layoutShown.columns() && !deps.columnWork.side() && deps.columns.side.mode === "thread"
      ? deps.inColumn(deps.columns.side, () => String(deps.selection().threadId || "")) : "";
    const own = deps.selection().view === "main" && headsAnsweredThread(replyState.target) ? deps.threadOf(replyState.target) : null;
    const thread = own !== null && own !== shownRight ? own : null;
    if (thread === null && deps.gatherReport(id).gathered === 0) return false;
    deps.gatherState().gathered = { parent: id, context: deps.channelContextKey(), byReply: true, thread, column: deps.columnWork.active() };
    deps.renderChannelRows();
    deps.renderScrollTools();
    return true;
  }

  /**
   * The row of `list` the box is drawn under: the last drawn row of the message's replies gathered under
   * it — a thread's root's whole thread, across the window or on the right; a root's block under its card
   * — or the message's own row. Null where its row is not drawn: a filter hides it, or the list no longer
   * holds it.
   *
   * @param {HTMLElement} list
   */
  function inlineReplyAnchor(list: HTMLElement): HTMLElement | null {
    return replyState.target ? anchorUnder(list, String(replyState.target.id), replyState.target) : null;
  }

  /**
   * The row of `list` a box answering message `id` (`message`, else its copy on the row) is drawn under,
   * by `inlineReplyAnchor`'s rule: the last drawn row of what is gathered under it, or its own row. Null
   * where its row is not drawn.
   *
   * @param {HTMLElement} list
   * @param {string} id
   * @param {any} [message]
   */
  function anchorUnder(list: HTMLElement, id: string, message: ReplyMessageLike | null = null): HTMLElement | null {
    const rows = /** @type {HTMLElement[]} */ ([...list.children]);
    const own = rows.find((row) => deps.idsOf(row).includes(id));
    if (!own || !deps.rowShown(own as HTMLElement)) return null;
    const answered = message || deps.rowMessages(own).find((held) => String(held.id) === id) || null;
    const after = rows.slice(rows.indexOf(own) + 1);
    let last = own as HTMLElement;
    const block = own.getAttribute("data-unpack-thread");
    let under: (row: Element) => boolean;
    if (block !== null) {
      under = (row) => own.hasAttribute("data-root-child") && row.getAttribute("data-unpack-thread") === block;
    } else if (list === deps.el("discord-log") && answered && rootOfOpenThread(answered)) {
      under = () => true;
    } else {
      const replies = deps.repliesTo(id, rows.flatMap(deps.rowMessages));
      under = (row) => deps.idsOf(row).length > 0 && deps.idsOf(row).every((one) => replies.has(one));
    }
    for (const row of after) {
      if (!under(row)) break;
      if (deps.rowShown(row as HTMLElement)) last = row as HTMLElement;
    }
    return last;
  }

  /**
   * Draw the reply open in the stream under its row (`inlineReplyAnchor`): that row is named the anchor
   * web/voice.css draws the box under, and keeps room under itself as tall as the box, so the rows after
   * it move down by the box and no more. With no row to draw it under, the box is drawn in the list's flow
   * at its foot (no `data-anchored`). Run as the box opens and closes and after every pass that can
   * rebuild or move the rows; the room again whenever the box changes height (`fitInlineRoom`).
   */
  function placeInlineReply(): void {
    const list = replyState.docked && replyState.list && replyState.list.parentNode ? replyState.list : null;
    const anchor = list ? deps.inColumn(replyState.column, () => inlineReplyAnchor(list)) : null;
    if (replyState.anchor !== anchor && replyState.anchor !== null) {
      replyState.anchor.removeAttribute("data-inline-reply");
      replyState.anchor.style.removeProperty("--inline-reply-room");
    }
    replyState.anchor = anchor;
    if (list === null) return;
    const mount = replyMountFor(list);
    if (anchor !== null && !anchor.hasAttribute("data-inline-reply")) anchor.setAttribute("data-inline-reply", "");
    if (mount.hasAttribute("data-anchored") !== (anchor !== null)) mount.toggleAttribute("data-anchored", anchor !== null);
    fitInlineRoom();
    // `#467 inline-send-in-place`. A reply's row drawn where the box now is goes to the foot, and back.
    placeRowsInPlaceAgain();
  }

  /** Draw the deps.outbox's rows again in every column where any was sent from the box in the stream. */
  function placeRowsInPlaceAgain(): void {
    for (const entry of deps.outbox.values()) {
      if (entry.inPlace) {
        deps.eachColumn(() => deps.renderOutgoingMessages());
        return;
      }
    }
  }

  /** The room under the anchor row: the height of the box drawn there, in px. */
  function fitInlineRoom(): void {
    if (replyState.anchor === null || !replyState.list) return;
    const room = `${Math.ceil(replyMountFor(replyState.list).getBoundingClientRect().height || 0)}px`;
    if (replyState.anchor.style.getPropertyValue("--inline-reply-room") !== room) replyState.anchor.style.setProperty("--inline-reply-room", room);
  }

  /**
   * Open the reply's box in the stream of the column being worked on (`replyState.column`), on the list `from`
   * is a row of: the message's replies gathered under it, the box under them, the column's own box hidden,
   * and the message where it was on the screen. See the section note.
   *
   * @param {HTMLElement | null} from
   */
  function dockReply(from: Element | null): void {
    const list = replyListOf(from);
    const id = String(replyState.target?.id);
    const top = from && from.parentNode && deps.shownNode(from) ? from.getBoundingClientRect().top : NaN;
    replyState.list = list;
    replyState.docked = true;
    replyState.gathered = gatherForReply(list);
    // The message where it was, whatever the gathering moved round it.
    holdRowAt(listPane(list as HTMLElement) as { scroller(): HTMLElement }, list as HTMLElement, id, top);
    const mount = replyMountFor(list);
    (list.parentNode as HTMLElement).append(mount);
    mount.append(deps.leftEl("reply-compose"));
    mount.hidden = false;
    // One box on the page: the columns' own boxes, both, while the reply's is open.
    for (const id of ["channel-composer", "side-composer"]) deps.leftEl(id).hidden = true;
    renderReplyingMarks();
    deps.markCardBesideReply();
    // `#304 back-gesture-in-app`. A reply open in the stream is a level of its own.
    deps.scheduleBackSync();
    placeInlineReply();
    // Where the box is under the foot of the column: up by as little as shows the whole box, the box
    // being what the reader is about to write in. A message, or its replies, taller than the column leaves
    // above it is the one case where the row's top goes up under the head, a scroll up to read. Only for a
    // box drawn under its row: one at the list's foot is not a reason to take the reader there.
    if (!anchorsSupported() || replyState.anchor === null) return;
    const pane = replyPane();
    const over = mount.getBoundingClientRect().bottom - pane.foot();
    if (over > 0) pane.scroller().scrollTop += over;
  }

  /** Scroll `pane` so that the row of `list` holding message `id` has its top at `top` again, where both are known. */
  function holdRowAt(pane: { scroller(): HTMLElement }, list: HTMLElement, id: string, top: number): void {
    const row = deps.rowIn(list, id);
    if (row && deps.shownNode(row) && Number.isFinite(top)) pane.scroller().scrollTop += row.getBoundingClientRect().top - top;
  }

  /**
   * Take the reply's box out of the stream: no reply is open after this, the column's own box is back, and
   * the message it answered is no longer edged.
   */
  function undockReply(): void {
    if (!replyState.docked) return;
    // Not open in the stream from here on, before the box moves: Chromium takes the focus off a box as it
    // leaves the column, and that is not the column going (`noteReplyFocusLeaving`).
    replyState.docked = false;
    replyState.focusTaken = false;
    for (const id of ["column-reply", "side-reply"]) deps.leftEl(id).hidden = true;
    // Home, out of sight: the left column's place, hidden.
    deps.leftEl("column-reply").append(deps.leftEl("reply-compose"));
    // The right column's place goes home from over the cards.
    deps.leftEl("side-pane").append(deps.leftEl("side-reply"));
    for (const id of ["channel-composer", "side-composer"]) deps.leftEl(id).hidden = false;
    placeInlineReply();
    replyState.list = null;
    renderReplyingMarks();
    deps.markCardBesideReply();
    deps.scheduleBackSync();
    // `#846 draft-chip-honest`. The box at the foot is back on screen, after a Send as after a put-down.
    renderDraftChip();
  }

  /**
   * The reply open in the stream of message `id` has been sent (`sent`) or put down: its box goes, the
   * column's own comes back, and what Reply gathered is put back unless it was sent, a sent reply joining
   * the stack where it lands. Nothing jumps (`restore`): a reply sent from the foot of the column leaves
   * the reader at its foot; otherwise the message answered, where it is on the screen, stays where it is,
   * or else the row the reader is on. The keys go to the column's list where they were in the box (`refocus`). The page
   * putting a reply away because the column changes under it asks for neither: the reader is going
   * elsewhere.
   *
   * @param {string} id
   * @param {{sent?: boolean, restore?: boolean, refocus?: boolean}} [how]
   */
  function leaveDockedReply(id: string, how: { sent?: boolean; restore?: boolean; refocus?: boolean } = {}): void {
    const { sent = false, restore = true, refocus = true } = how;
    const box = deps.leftEl("reply-compose");
    const held = document.activeElement;
    const inside = Boolean(held) && deps.nodeWithin(box, held);
    const pane = replyPane();
    const column = replyState.column;
    const list = replyState.list;
    const area = pane.scroller();
    // Sent from the foot of the list, the reader stays at its foot, where the reply lands; put down, the
    // message stays where it is, which leaves the reader where they are.
    const footed = restore && sent && deps.atBottom(area);
    const row = list ? deps.rowIn(list, id) : null;
    const rect = row && deps.shownNode(row) ? row.getBoundingClientRect() : null;
    const top = rect && rect.bottom > pane.head() && rect.top < pane.foot() ? rect.top : NaN;
    const mark = restore && !footed && !Number.isFinite(top) && pane === columnPane(column) ? deps.inColumn(column, deps.captureScroll) : null;
    const scatter = replyState.gathered && !sent;
    replyState.gathered = false;
    undockReply();
    deps.inColumn(column, () => {
      const settledGathered = deps.gatherState().gathered;
      if (scatter && settledGathered && settledGathered.byReply && settledGathered.parent === id) {
        if (restore) {
          deps.gatherState().gathered = null;
          deps.renderChannelRows();
          deps.renderScrollTools();
        } else deps.putRepliesBack();
      }
    });
    if (footed) deps.followFoot(area);
    else if (restore && list && Number.isFinite(top)) holdRowAt(pane, list, id, top);
    else if (mark) deps.inColumn(column, () => deps.restoreScroll(mark));
    if (refocus && (inside || deps.focusLost())) deps.focusList(pane);
  }

  /**
   * Open again the reply put away when its list went (`replyState.pending`), as soon as its message is drawn on a
   * list on screen: the left column's, or the right one's where it is up. In the stream as Reply opens it,
   * words, quote and route and all being the message's draft. The focus goes back into the box only where
   * it was there when the list went and this follows at once; a reply reopened later, its message drawn
   * again when the reader came back to it, leaves the keys where the reader has them. Run after every
   * redraw of a list while one waits, and by the layout two deps.columns went to once it is drawn.
   */
  function reopenPendingReply(): void {
    const pendingNow = replyState.pending;
    if (pendingNow === null || !pendingNow.ready || replyState.docked || deps.screens.screen() !== "main" || deps.screens.view() !== "discord") return;
    // Not under a reader writing in a box: the box would go from under their hands. A later redraw asks again.
    const held = document.activeElement;
    if (deps.isComposer(held) && held !== deps.leftEl("reply-text")) return;
    const pending = replyState.pending as { id: string; typing: boolean };
    const { id, typing } = pending;
    /** @type {Array<{column: any, list: () => HTMLElement}>} */
    const lists = [{ column: deps.columns.main, list: () => deps.el(deps.pinsView().only ? "pinned-log" : "discord-log") }];
    if (deps.sideColumnUp()) lists.push({ column: deps.columns.side, list: () => deps.leftEl(deps.columns.side.mode === "thread" ? "side-log" : "side-cards") });
    for (const { column, list } of lists) {
      const row = deps.inColumn(column, () => {
        const shown = list();
        return shown && deps.shownNode(shown) ? deps.rowIn(shown, id) : null;
      });
      if (!row || !deps.shownNode(row)) continue;
      replyState.pending = null;
      deps.inColumn(column, () => openReply(deps.rowMessages(row), row, "", typing));
      return;
    }
  }

  /**
   * The reply's box, or something in it, has lost the focus. `#314 reply-focus-flake`.
   *
   * A WINDOW NARROWED UNDER A REPLY OPEN IN A COLUMN CAN TAKE THE FOCUS OFF ITS BOX BEFORE THE PAGE HANDS IT
   * ON. The page decides one column or two at the frame after a resize (`renderColumns`), and opens the reply
   * again in the stream then, with the focus if the box had it (`leaveColumns`, `reopenPendingReply`). But
   * Chromium, once it has laid the narrower window out without the right column, takes the focus off a box it
   * no longer draws, in a task of its own, which a slow frame lets come first. The hand-off then found the
   * focus nowhere and gave the keys to the list: the reader, mid-sentence, was out of the box. The browser
   * check of the reply in a column met it now and then on a loaded host; a reader would on a busy machine.
   *
   * So it is noted here: the focus leaving the box for nothing while the box is open in a column that is no
   * longer drawn is the browser's doing, and the hand-off gives it back while nothing else has taken it. Left
   * for another element, or for nothing while the box is still drawn (a click on what takes no focus, another
   * window), it was the reader's, and is not.
   *
   * @param {FocusEvent} event
   */
  function noteReplyFocusLeaving(event: FocusEvent): void {
    const box = deps.el("reply-compose");
    replyState.focusTaken = replyState.docked && !(event && event.relatedTarget) &&
      typeof box.getClientRects === "function" && box.getClientRects().length === 0;
  }

  /**
   * Put away a reply open in a column whose list is about to show something else, its words kept as a
   * draft. Nothing is restored, and the keys are left where they are: the reader is going elsewhere.
   */
  function putDockedReplyAway(): void {
    if (replyState.docked) closeReply({ restore: false, refocus: false });
  }

  /**
   * Edge the row answered by a reply open in a column, in that column, and no other row anywhere: on
   * opening, on closing, and as each row is drawn (`markReplying`).
   */
  function renderReplyingMarks(): void {
    /** @type {HTMLElement[]} */
    const rows = [];
    for (const held of deps.channelContexts.values()) rows.push(...(held.rows || []));
    for (const row of rows) markReplying(row as HTMLElement, false);
    for (const column of [deps.columns.main, deps.columns.side]) {
      deps.inColumn(column, () => {
        for (const row of [...deps.el("discord-log").children, ...deps.el("pinned-log").children]) {
          const first = deps.rowMessages(/** @type {HTMLElement} */ (row))[0];
          markReplying((row as HTMLElement), Boolean(first) && replyingTo(first as ReplyMessageLike));
        }
      });
    }
  }

  /** Whether `message` is the one a reply open in the column being worked on answers. */
  function replyingTo(message: ReplyMessageLike): boolean {
    return replyState.docked && replyState.target !== null && deps.columnWork.active() === replyState.column &&
      String(/** @type {any} */ (message).id) === String(/** @type {any} */ (replyState.target).id);
  }

  /** Edge `row` as the message being answered, or not. */
  function markReplying(row: HTMLElement, on: boolean): void {
    if (row.hasAttribute("data-replying") !== on) row.toggleAttribute("data-replying", on);
  }

  /**
   * The reply open in the stream follows the store, as the heading over an open thread does. A read that
   * places a message the stream left unplaced settles where its reply goes — the note under the box and
   * the new-thread choice say so at once. Called with every redraw of the channel's navigation, which every
   * read makes, and with every message the live stream brings into the store (`receiveLiveMessage`).
   *
   * `#185 reply-new-thread`. A reply that arrives, from a read or the live stream, in the thread the box
   * offers to start takes the offer away, as it turns the message's grey Thread into Thread(1): "Start a
   * new thread" would be false. A reply the reader had ticked it for still goes into that thread, now as
   * a reply in it, and the note says so (`replyRoute`'s `into`); an unticked one goes to the main channel,
   * as it was going to. The words, and where they go, stay with the draft.
   */
  function followReplyStore(): void {
    if (!replyState.docked || !replyState.target) return;
    let moved = false;
    // `#315 removed-reply-target`. Removed upstream since Reply was opened, wherever it was going: Send
    // refuses it now, and the note says so.
    if (!replyState.routing.removed && deps.removedUpstream(replyState.target.id)) {
      replyState.routing = replyRoute(replyState.target);
      deps.el("reply-branch").checked = false;
      deps.el("reply-branch-row").hidden = true;
      moved = true;
    } else if (replyState.routing.unknown) {
      const settled = replyRoute(replyState.target);
      if (!settled.unknown || settled.removed !== replyState.routing.removed) {
        replyState.routing = settled;
        deps.el("reply-branch").checked = false;
        deps.el("reply-branch-row").hidden = !settled.branch;
      }
    } else if (replyState.routing.branch || replyState.routing.into) {
      // Asked in the column Reply was opened from, as `openReply` asked it: the route reads the view.
      const bound = replyBoundForThread();
      const settled = deps.inColumn(replyState.column, () => replyRoute(replyState.target as ReplyMessageLike, bound));
      if (settled.branch !== replyState.routing.branch || settled.into !== replyState.routing.into) {
        replyState.routing = settled;
        deps.el("reply-branch").checked = Boolean(settled.branch) && bound;
        deps.el("reply-branch-row").hidden = !settled.branch;
        moved = true;
      }
    }
    // `#1052 post-to-main`: the offer follows the settled route, as the branch offer does; a
    // route that no longer joins a thread takes the tick away with the offer.
    deps.el("reply-to-main-row").hidden = !(replyState.routing.thread || replyState.routing.into);
    if (deps.el("reply-to-main-row").hidden) deps.el("reply-to-main").checked = false;
    renderReplyDestination();
    if (moved) rememberDraft();
  }

  return {
    replyState, replyRoute, replyBoundForThread, rememberDraft, draftMarker, renderDraftMarkers,
    noteDraft, inSight, renderDraftChip, openSentenceReply, pressDraftChip, cancelQuickReply, placeRowsInPlaceAgain,
    answerQuickReplyDiscard, quickReplyToStream, openReply, renderReplyDestination, closeReply,
    cancelReply, answerReplyDiscard, sendReply, replyPane, replyKeysHere, placeInlineReply,
    fitInlineRoom, reopenPendingReply, noteReplyFocusLeaving, putDockedReplyAway, replyingTo,
    markReplying, followReplyStore, anchorUnder, replySight: deps.replySight,
  };
}

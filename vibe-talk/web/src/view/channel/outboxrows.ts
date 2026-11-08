// The outbox's own rows (`#277 send-order`, `#467 in-place-sends`, `#500
// sent-receipts-pile-up`; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel; mu-readcore): a pending send drawn where it stands, its status words from
// present/notices, and the fan-out that redraws them in every column. The sends themselves
// are core/outbox's; this owner only draws them and retires receipts through it.

import type { Message } from "../../contract/index.ts";
import { threadOf, timeOf } from "../../core/channels/index.ts";
import { outgoingStatusWords, sendTimingTextWords } from "../../present/notices/index.ts";
import { rawTextOf } from "../../present/rows/index.ts";
import type { TimelineSelection } from "./readcore.ts";

/** A pending send as its row reads it (the outbox's live entry shape, mirrored). */
export interface OutgoingRowEntry {
  id: string;
  channel: string;
  threadId: string | null;
  placement?: string;
  replyTo?: string | null;
  text: string;
  remaining?: string;
  state: string;
  phase: string;
  detail?: string;
  postedCount: number;
  parts: readonly Message[];
  observedParts: readonly string[];
  progress?: { posted: number; parts: number } | null;
  confirming?: boolean;
  inPlace?: string;
  continueIn?: string | null;
  tickFailed?: boolean;
  serverTick?: unknown;
  upstreamTick?: unknown;
  showTimes?: boolean;
  inCards?: boolean;
  originView?: string;
}

/** What drawing unsent rows asks of the page. */
export interface OutboxRowsDependencies {
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  selectedChannelId(): string;
  selection(): TimelineSelection;
  outgoingThreadOf(entry: OutgoingRowEntry): string | null;
  entries(): Iterable<OutgoingRowEntry>;
  removeEntry(id: string): void;
  forgetObservations(id: string): void;
  receiptRetires(entry: OutgoingRowEntry, drawn: ReadonlySet<string>): boolean;
  retryOf(id: string): { at: number; offline: boolean; unsent: boolean } | undefined;
  editableOf(entry: OutgoingRowEntry): boolean;
  blockerOf(entry: OutgoingRowEntry): OutgoingRowEntry | null;
  inPlaceDrawn: Map<unknown, { anchors: HTMLElement[]; groups: HTMLElement[] }>;
  blockerRemoved(entry: OutgoingRowEntry): boolean;
  canRetry(entry: OutgoingRowEntry): boolean;
  snippetOf(entry: OutgoingRowEntry): string;
  sideCardsShown(): boolean;
  rowMessages(row: Element | null | undefined): Message[];
  threadHue(id: string): string;
  pinsOnly(): boolean;
  pendingThreadTimes(): Iterable<[string, number]>;
  sentFromThisPage(message: Message): boolean;
  ownSentAt(id: string): number;
  retryKnown(id: string): boolean;
  searchable(row: Element, ...parts: unknown[]): void;
  listState(): { newestId: string | null; backlog: number; runRows: number; outgoingKey?: string };
  renderReplyCounts(): void;
  renderChannelRows(): void;
  threadCards(summaries: unknown[]): Node[];
  sideColumnUp(): boolean;
  renderSideCards(): void;
  timelineRows(): { threads: unknown[] };
  columnWork: { active(): unknown };
  roomWatch: { observe(el: Element): void; unobserve(el: Element): void } | null;
  roomAnchors: WeakMap<HTMLElement, HTMLElement>;
  columns: { main: unknown; side: { mode: string } };
  eachColumn(fn: () => void): void;
  pendingThreads: { noteDrawn(): boolean };
  replyState: { threadId: string | null; anchor: unknown };
  shownNode(list: HTMLElement): boolean;
  anchorUnder(list: HTMLElement, id: string): HTMLElement | null;
  persist(): void;
  inThisColumn(fn: () => void): () => void;
  openThread(id: string): void;
  renderBodyInto(parent: HTMLElement, messages: readonly Message[]): void;
  textBody(text: string): HTMLElement;
  renderSideCards(): void;
  renderChannelRows(): void;
  retry(id: string): void;
  edit(id: string): void;
  pinsOnly(): boolean;
  sideCardsShown(): boolean;
  rowMessages(row: Element | null | undefined): Message[];
  channelStore: { current(): { channel: string; scope?: string | null; messages: { id: string; thread?: { is_root?: boolean; hue?: unknown } | null }[] } };
  threadHue(id: string): string;
  inThisColumn<T>(fn: () => T): T;
  guardQuietly(fn: (...args: never[]) => unknown): (...args: never[]) => unknown;
  openThread(id: string): void;
  renderBodyInto(parent: HTMLElement, messages: readonly Message[]): void;
  textBody(text: string): HTMLElement;
  storageOkay(): boolean;
  knownChannel(channel: string): { writable?: boolean } | null;
  retryOutgoingMessage(id: string): void;
  editOutgoingMessage(id: string): void;
  dismissOutgoingMessage(id: string): void;
  serviceOf(entry: OutgoingRowEntry): string;
  dismissedDays: number;
  unsentText(entry: OutgoingRowEntry): string;
  nowMs(): number;
  now(): number;
}

/** The outbox rows' commands: the legacy names, unchanged. */
export interface OutboxRows {
  outgoingInView(entry: OutgoingRowEntry): boolean;
  renderOutgoingMessages(): void;
  renderOutgoingEverywhere(): void;
  withPendingThreads(summaries: unknown[]): unknown[];
  outgoingReplyKey(): string;
  fitOutgoingRoom(group: HTMLElement, anchor: HTMLElement): void;
  dispose(): void;
}

/** The outbox rows over `deps`. */
export function createOutboxRows(deps: OutboxRowsDependencies): OutboxRows {
  const el = deps.el;
  const leftEl = deps.leftEl;
  const selectedChannelId = deps.selectedChannelId;
  const selection = deps.selection;
  const outgoingThreadOf = deps.outgoingThreadOf;
  const outbox = { values: () => deps.entries(), remove: (id: string) => deps.removeEntry(id) };
  const outgoingObservations = { delete: (id: string) => deps.forgetObservations(id) };
  const sentReceiptRetires = (entry: OutgoingRowEntry, drawn: ReadonlySet<string>) => deps.receiptRetires(entry, drawn);
  const outgoingStatus = (entry: OutgoingRowEntry) => {
    const blocker = deps.blockerOf(entry);
    return outgoingStatusWords(entry as never, blocker as never, deps.retryOf(entry.id),
      blocker ? deps.retryOf(blocker.id) : undefined,
      blocker ? deps.blockerRemoved(blocker) : false,
      deps.now(), blocker ? deps.editableOf(blocker) : false,
      deps.canRetry(entry), blocker ? deps.snippetOf(blocker) : "");
  };
  const outgoingInPlaceDrawn = deps.inPlaceDrawn;
  const sideCardsShown = deps.sideCardsShown;
  const rowMessages = deps.rowMessages;
  const channelStore = deps.channelStore;
  const threadHue = deps.threadHue;
  const pendingThreadTimes = deps.pendingThreadTimes;
  const sentFromThisPage = deps.sentFromThisPage;
  const ownPosts = { sentAt: (id: string) => deps.ownSentAt(id) };
  const outgoingRetries = { has: (id: string) => deps.retryKnown(id) };
  const searchable = deps.searchable;
  const editableSend = (entry: OutgoingRowEntry) => deps.editableOf(entry);
  const listState = deps.listState;
  const renderReplyCounts = deps.renderReplyCounts;
  const renderChannelRows = deps.renderChannelRows;
  const threadCards = deps.threadCards;
  const sideColumnUp = deps.sideColumnUp;
  const renderSideCards = deps.renderSideCards;
  const timelineRows = deps.timelineRows;
  const columnWork = deps.columnWork;
  const outgoingRoomWatch = deps.roomWatch;
  const columns = deps.columns;
  const eachColumn = deps.eachColumn;
  const pendingThreads = deps.pendingThreads;
  const replyState = deps.replyState;
  const outgoingRoomAnchors = deps.roomAnchors;
  const shownNode = deps.shownNode;
  const anchorUnder = deps.anchorUnder;
  const pinsView = () => ({ only: deps.pinsOnly() });
  const persistOutgoingMessages = () => deps.persist();
  const inThisColumn = deps.inThisColumn;
  const guardQuietly = deps.guardQuietly;
  const openThread = deps.openThread;
  const renderBodyInto = deps.renderBodyInto;
  const textBody = deps.textBody;
  const unsentText = deps.unsentText;
  const sendTimingText = (entry: OutgoingRowEntry) => sendTimingTextWords(entry as never, deps.serviceOf(entry), deps.nowMs());
  const knownChannel = deps.knownChannel;
  const retryOutgoingMessage = deps.retryOutgoingMessage;
  const editOutgoingMessage = deps.editOutgoingMessage;
  const dismissOutgoingMessage = deps.dismissOutgoingMessage;
  let disposed = false;
  void disposed;

function outgoingInView(entry: OutgoingRowEntry): boolean {
  if (entry.channel !== selectedChannelId()) return false;
  // `#466 reply-promotes-thread`: the thread it goes into, which a reply to a message on Slack is too.
  if (selection().view === "thread") return outgoingThreadOf(entry) === String(selection().threadId);
  if (selection().view === "flat") return true;
  // A reply opened from a root in the main view needs a visible receipt when Reply closes.
  return !entry.threadId || entry.originView === "main";
}

function renderOutgoingMessages() {
  const host = el("outgoing-log");
  const channel = selectedChannelId();
  // `#491 threads-view-sent-receipt`. And the rows among the Threads view's cards, where that view
  // draws a reply sent from it: the owner, 2026-10-23, "After it sent there was still a card on the
  // bottom AND it rendered as a card under the thread. The same message is on the screen twice."
  const drawnRows = [...el("discord-log").children];
  if (sideCardsShown()) {
    drawnRows.push(.../** @type {HTMLElement[]} */ ([...leftEl("side-cards").children])
      .filter((li) => li.hasAttribute("data-unpack-thread") || li.hasAttribute("data-main-row")));
  }
  const visibleIds = new Set(drawnRows.flatMap(rowMessages)
    .filter((message) => String(message.channel_id) === channel).map((message) => String(message.id)));
  let retired = false;
  const rows = [];
  for (const entry of outbox.values()) {
    // An ACK can render as an ordinary, actionable provider row immediately; its receipt, drawn
    // only where the message is not, goes by `sentReceiptRetires` (`#500 sent-receipts-pile-up`).
    const unseen = entry.channel === channel
      ? entry.parts.filter((part) => !visibleIds.has(part.id)) : entry.parts;
    if (sentReceiptRetires(entry, visibleIds)) {
      outbox.remove(entry.id);
      outgoingObservations.delete(entry.id);
      retired = true;
      continue;
    }
    if (!outgoingInView(entry)) continue;
    if (entry.state === "sent" && entry.parts.length && !unseen.length) continue;
    const row = document.createElement("li");
    row.className = "outgoing-message";
    row.setAttribute("data-outgoing-id", entry.id);
    const waiting = outgoingRetries.has(entry.id);
    // `#277 send-order`. Waiting its turn is neither sending nor failing.
    const queued = entry.state === "sending" && entry.phase === "queued";
    row.setAttribute("data-send-state", waiting ? "retrying" : queued ? "queued" : entry.state);
    row.setAttribute("data-who", "me");
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = entry.replyTo ? "You · Reply" : "You";
    // Never for the scope: a reply in a single-conversation channel is on Main already. The thread
    // a long post continued in counts too (`#263 post-split-per-provider`).
    const placedIn = entry.threadId || entry.continueIn;
    if (placedIn && selection().view !== "thread" && placedIn !== channelStore.current().scope) {
      const thread = document.createElement("button");
      thread.className = "thread-badge";
      thread.setAttribute("type", "button");
      thread.style.setProperty("--thread-hue", threadHue(placedIn));
      thread.textContent = "Thread";
      thread.addEventListener("click", inThisColumn(guardQuietly(() => openThread(placedIn))));
      meta.append(thread);
    }
    const body = document.createElement("div");
    body.className = "msg-text";
    const acknowledged = entry.state === "sent" && unseen.length > 0;
    const text = acknowledged
      ? unseen.map(rawTextOf).join("\n\n")
      : entry.postedCount ? entry.remaining : entry.text;
    // The parts the server acknowledged are drawn as it rendered them; text still on its way is
    // the reader's own draft, drawn as plain paragraphs until a read brings it back rendered.
    if (acknowledged) {
      renderBodyInto(body, unseen as unknown as Message[]);
    } else {
      body.append(textBody(text as string));
    }
    const status = document.createElement("div");
    status.className = "outgoing-status";
    status.setAttribute("role", "status");
    if (entry.state === "sending" && !queued) {
      const spinner = document.createElement("span");
      spinner.className = "outgoing-spinner";
      spinner.setAttribute("aria-hidden", "true");
      status.append(spinner);
    }
    const label = document.createElement("span");
    label.textContent = outgoingStatus(entry) + (deps.storageOkay() ? "" :
      " This browser could not save this message. Keep this page open; reloading may lose its text or delivery status.");
    status.append(label);
    // `#621 send-double-check`: two very small ticks right after the ellipsis.
    if (entry.state === "sending" || entry.state === "sent" || entry.tickFailed) {
      const ticks = document.createElement("span");
      ticks.className = "send-ticks";
      ticks.setAttribute("title", sendTimingText(entry));
      const first = document.createElement("span");
      first.className = `send-tick${entry.serverTick ? " ok" : ""}`;
      first.textContent = "✓";
      first.setAttribute("data-tick", "server");
      const second = document.createElement("span");
      second.className = `send-tick${entry.tickFailed ? " bad" : entry.upstreamTick ? " ok" : ""}`;
      second.textContent = entry.tickFailed ? "✗" : "✓";
      second.setAttribute("data-tick", "upstream");
      ticks.append(first, second);
      ticks.addEventListener("click", () => {
        entry.showTimes = !entry.showTimes;
        renderOutgoingEverywhere();
      });
      status.append(ticks);
      if (entry.showTimes) {
        const times = document.createElement("span");
        times.className = "send-times";
        times.textContent = ` ${sendTimingText(entry)}`;
        status.append(times);
      }
    }
    row.append(meta, body, status);
    // `#277 send-order`. A message waiting its turn behind another has not left the device: it can be
    // taken back, Edit and Dismiss, but there is no Retry to send it out of turn.
    if (entry.state === "failed" || entry.state === "unconfirmed" || queued) {
      const actions = document.createElement("div");
      actions.className = "outgoing-actions";
      const retry = document.createElement("button");
      retry.className = "outgoing-retry";
      retry.setAttribute("type", "button");
      retry.textContent = waiting ? "Retry now" : entry.postedCount ? "Retry unsent text" : "Retry";
      retry.disabled = !entry.remaining || knownChannel(entry.channel)?.writable === false;
      retry.addEventListener("click", inThisColumn(guardQuietly(() => retryOutgoingMessage(entry.id))));
      // `#193 compose-draft-loss`. Back into the box it was written in, and out of the outbox: only
      // where none of it can be in the channel already (`editableSend`).
      const edit = document.createElement("button");
      edit.className = "outgoing-edit";
      edit.setAttribute("type", "button");
      edit.textContent = "Edit";
      edit.setAttribute("title", entry.replyTo
        ? "Put this text back in Reply on the message it answers, and take it out of the outbox."
        : "Put this text back in the message box it came from, and take it out of the outbox.");
      edit.disabled = !unsentText(entry).trim();
      edit.addEventListener("click", inThisColumn(guardQuietly(() => editOutgoingMessage(entry.id))));
      const dismiss = document.createElement("button");
      dismiss.className = "outgoing-dismiss";
      dismiss.setAttribute("type", "button");
      dismiss.textContent = "Dismiss";
      dismiss.setAttribute("title",
        "Take this out of the outbox. This does not delete a message from the chat service. Undo " +
          `brings it back, and Settings keeps it under Recently dismissed for ${deps.dismissedDays} days.`);
      dismiss.addEventListener("click", inThisColumn(() => dismissOutgoingMessage(entry.id)));
      actions.append(...(queued ? [] : [retry]), ...(editableSend(entry) ? [edit, dismiss] : [dismiss]));
      row.append(actions);
    }
    // `#129 message-search`. What the reader WROTE, under the word the row prints for them: a
    // message you sent is a message you look for, and it is sitting in the same list as the ones
    // you received.
    searchable(row, "You", text);
    // `#211 link-filter`. A message you sent can hold a link like any other.
    (row as HTMLLIElement & { linkText?: string }).linkText = String(text);
    (row as HTMLLIElement & { outgoingEntry?: OutgoingRowEntry }).outgoingEntry = entry;
    rows.push(row);
  }
  if (retired) persistOutgoingMessages();
  // `#467 inline-send-in-place`. A reply sent from the box in the stream stays where the box was.
  const foot = placeOutgoingInPlace(rows);
  host.replaceChildren(...foot);
  // Nor over the Pinned filter, which shows pinned messages and nothing else. `#206 pin-message`.
  host.hidden = foot.length === 0 || pinsView().only;
  // `#228 reply-visible`. A reply of the owner's counts on the Reply of the message it answers from
  // the moment it is queued, and that count changes as it lands, fails or is dismissed: counted
  // again when the outbox's replies have changed since the counts were drawn. Not on a pass over
  // the rows, which has just counted them.
  if (outgoingReplyKey() !== listState().outgoingKey) renderReplyCounts();
}

// --- a reply's row where its box was ----------------------------------------------------------------
//
// `#467 inline-send-in-place`. The owner: "The compose box jumps to where it should be, in situ in the
// thread... But then the little 'Sending..' box, the one that would show Retry if it fails, jumps back
// down to the BOTTOM, not where the compose box was. This should provide better continuity."
//
// A reply sent from the box in the stream (`#410 inline-reply-editor`) keeps its row there: under the
// message it answered and what is gathered under it (`anchorUnder`), in the column's list when it draws
// that message — a thread, Main, All, in either column — saying how it goes ("Sending…", "Posting part 2
// of 3…", or "Not sent" with Retry and Edit),
// until the reply itself is drawn in its place, which retires the row as it always has. A message sent
// from the box at the foot keeps its row at the foot, where it was written. Drawn as the box is: never
// a row of the list, but named to its row by an anchor of its own (`--outgoing-N`), that row keeping
// room under itself as tall as what is drawn there, so the rows after it move down by that and no
// more, and nothing jumps when the reply takes its place. Without anchor positioning it is drawn after
// the list, at its foot. Not under the row the open reply's box is drawn under: that one place holds
// one thing, and the row goes to the foot until the box moves on (`placeInlineReply` places them again
// as the box opens and goes). Elsewhere, where the message is not drawn — another list, the cards, a
// search that hides it (whose pass places them again) — and under the Pinned filter, at the foot.

/** By column, the rows named as anchors on its last pass, and the lists of rows drawn under them. */

function fitOutgoingRoom(group: HTMLElement, anchor: HTMLElement): void {
  const room = `${Math.ceil(group.getBoundingClientRect().height || 0)}px`;
  if (anchor.style.getPropertyValue("--outgoing-room") !== room) anchor.style.setProperty("--outgoing-room", room);
}

/** Keeps each row's room as tall as what is drawn under it through a resize or a reflow, as the box's is. */

function placeOutgoingInPlace(rows: HTMLElement[]): HTMLElement[] {
  const drawn = outgoingInPlaceDrawn.get(columnWork.active()) || { anchors: [], groups: [] };
  for (const anchor of drawn.anchors) {
    anchor.removeAttribute("data-outgoing-anchor");
    anchor.style.removeProperty("anchor-name");
    anchor.style.removeProperty("--outgoing-room");
  }
  for (const group of drawn.groups) {
    if (outgoingRoomWatch) outgoingRoomWatch.unobserve(group);
    if (group.parentNode) group.parentNode.removeChild(group);
  }
  // Not under the Pinned filter, which shows pinned messages and nothing else (`#206 pin-message`): the
  // rows go to the foot, which is hidden there.
  if (pinsView().only) {
    outgoingInPlaceDrawn.set(columnWork.active(), { anchors: [], groups: [] });
    return rows;
  }
  // `#500 sent-receipts-pile-up`. Or the Threads view's cards, over the left column's list: a reply sent
  // from the box under an unpacked row (`#407 unpack-row-parity`) stays under that row, not at the foot
  // of the cards, until it is drawn there or its receipt goes; never under the same message in Main.
  const cards = columnWork.active() === columns.main && sideCardsShown() ? [leftEl("side-cards")] : [];
  /** @type {Map<HTMLElement, {list: HTMLElement, rows: HTMLElement[]}>} */
  const under = new Map();
  /** @type {HTMLElement[]} */
  const foot = [];
  for (const row of rows) {
    const entry = (row as HTMLElement & { outgoingEntry?: OutgoingRowEntry }).outgoingEntry;
    let found = null;
    if (entry && entry.inPlace) {
      for (const list of entry.inCards ? cards : [el("discord-log")]) {
        const anchor = shownNode(list) ? anchorUnder(list, String(entry.inPlace)) : null;
        if (anchor) {
          found = { list, anchor };
          break;
        }
      }
    }
    if (!found || found.anchor === replyState.anchor || found.anchor.hasAttribute("data-inline-reply")) {
      foot.push(row);
      continue;
    }
    const held = under.get(found.anchor) || { list: found.list, rows: [] };
    held.rows.push(row);
    under.set(found.anchor, held);
  }
  const next: { anchors: HTMLElement[]; groups: HTMLElement[] } = { anchors: [], groups: [] };
  let n = 0;
  for (const [anchor, { list, rows: placed }] of under) {
    const name = `--outgoing-${columnWork.active() === columns.side ? "side" : "main"}-${n++}`;
    const group = document.createElement("ol");
    group.className = "messages outgoing-inplace";
    group.setAttribute("aria-label", "Outgoing reply");
    group.style.setProperty("position-anchor", name);
    group.append(...placed);
    // Straight after the list: where it is drawn without anchor positioning, the list's foot.
    /** @type {HTMLElement} */ (list.parentNode).insertBefore(group, list.nextSibling);
    anchor.setAttribute("data-outgoing-anchor", "");
    anchor.style.setProperty("anchor-name", name);
    fitOutgoingRoom(group, anchor);
    outgoingRoomAnchors.set(group, anchor);
    if (outgoingRoomWatch) outgoingRoomWatch.observe(group);
    next.anchors.push(anchor);
    next.groups.push(group);
  }
  outgoingInPlaceDrawn.set(columnWork.active(), next);
  return foot;
}

/** The outbox's rows in every column showing messages: what the outbox did is no one column's. `#229`. */

function renderOutgoingEverywhere() {
  // `#466 reply-promotes-thread`. A send going into a thread, or gone from the outbox, changes that
  // thread's count and whether the Threads view has a card for it: every row, and the cards, again.
  if (!pendingThreads.noteDrawn()) {
    eachColumn(renderOutgoingMessages);
    return;
  }
  eachColumn(() => {
    renderChannelRows();
    if (selection().view === "threads") el("thread-list").replaceChildren(...threadCards(timelineRows().threads));
  });
  if (sideColumnUp() && columns.side.mode !== "thread") renderSideCards();
}

// --- the thread a send goes into, before it is there ------------------------------------------------
//
// `#466 reply-promotes-thread`. The owner, on Slack: "I expected the main-channel message to be promoted
// to the root of a new thread, and that didn't visually happen, not locally and not quickly ... It would
// be great if promoting a root message to a thread and visually linking the sent message to it happened
// locally, without waiting for some asynchronous server refresh."
//
// A send still in the outbox — on its way, unconfirmed, or not sent and waiting for the reader — counts
// in the thread it goes to, and is shown there: the message it answers reads as that thread's root, with
// the count and colour of a thread (`drawThreadChips`); the Threads view has a card for it
// (`threadCards`); and the thread itself shows the send's row (`outgoingInView`). All of it is derived
// from the outbox at every pass, keyed by the thread's own id (on Slack the message's ts, from its thread
// record, never its message id), and nothing is written into the store's copy of the message answered:
// a read whose copy predates the reply cannot undo it, and a send dismissed takes it back with nothing to
// undo. A send not sent keeps its thread, with Retry, until it is dismissed (replication model §5.2).
// Once sent, the reply is an ordinary held reply, which the count already takes (`max(reported, held)`).
//
// The count adds the sends to what the provider reported and what is held, which overcounts by one a
// send that landed though its answer was lost and that a later read already counts, until it is settled
// (`settleFromServer`).

// The pending threads' key as it stood when the rows were last passed over for it is their own
// (`pendingThreads.noteDrawn`, web/src/core/outbox, `#624 voice-page-modularity`).

/**
 * Which thread each send in the outbox posts into, and how many sends count in each thread of the channel
 * on screen and when the newest was written, worked out once for each change: web/src/core/outbox's
 * (`createPendingThreads`, `#624 voice-page-modularity`).
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


// `outgoingReplyKey` as it stood when the Reply counts were last drawn. Kept per column: `listState().outgoingKey` (`freshListState`, `#624 voice-page-modularity`).

/** Say on a row's Reply how many replies it has: "Reply (2)", and in words to a screen reader. */


function withPendingThreads(summaries: unknown[]): unknown[] {
  const listed = new Set((summaries as { id: unknown }[]).map((summary) => String(summary.id)));
  const viewing = String(selectedChannelId());
  const added = new Map<string, unknown>();
  const starting: [string, number][] = [...pendingThreadTimes()];
  if (channelStore.current().channel === viewing) {
    for (const held of channelStore.current().messages) {
      if (threadOf(held as Message) && (held.thread as { is_root?: boolean }).is_root !== true && sentFromThisPage(held as Message)) {
        starting.push([String((held.thread as { id: unknown }).id), ownPosts.sentAt(String(held.id)) || timeOf(held, "timestamp")]);
      }
    }
  }
  starting.sort((a, b) => b[1] - a[1]);
  for (const [thread, at] of starting) {
    if (listed.has(thread) || added.has(thread)) continue;
    const root = channelStore.current().messages.find((held) => threadOf(held as Message) === thread && (held.thread as { is_root?: boolean }).is_root === true);
    if (!root) continue;
    added.set(thread, {
      id: thread, title: "", root, display_name: null, summary: null, hue: (root.thread as { hue?: unknown }).hue ?? null,
      reply_count: null, reply_count_exact: false, complete: false,
      updated_at: new Date(at || Date.now()).toISOString(),
    });
  }
  return added.size ? [...added.values(), ...summaries] : summaries;
}

function outgoingReplyKey(): string {
  const viewing = String(selectedChannelId());
  const parts: string[] = [viewing];
  for (const entry of outbox.values()) {
    if (entry.channel !== viewing || !entry.replyTo) continue;
    const first = entry.parts[0];
    parts.push(`${entry.id}:${entry.state}:${entry.postedCount}:${outgoingRetries.has(entry.id)}:` +
      `${first ? String(first.id) : ""}`);
  }
  return parts.join(" ");
}

  return {
    outgoingInView, renderOutgoingMessages, renderOutgoingEverywhere, withPendingThreads, outgoingReplyKey, fitOutgoingRoom,
    dispose() { disposed = true; },
  };
}

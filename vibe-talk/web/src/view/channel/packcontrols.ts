// The threads column's cards packed and unpacked (`#440 thread-card-pack-toggle`, `#428
// summary-cards-full`, `#522 thread-card-max-lines`; `#624 voice-page-modularity`, moved from
// the legacy script into view/channel; mu-readcore): the Show replies toggle and card states,
// packing a thread from the keyboard, and the side column's cards built and filtered. The unpacked
// block itself is unpackblock.ts's; the gaps among the cards are cardgaps.ts's.

import type { Message } from "../../contract/index.ts";
import type { UnpackChoice } from "../../core/channels/index.ts";
import { CARD_STATES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID } from "../../present/rows/index.ts";
import { rowShown } from "./rows.ts";
import { childByClass, nodeWithin, setAttributeIfChanged, setTextIfChanged } from "../dom/index.ts";
import { fillCardText, markCardLayout } from "./cards.ts";

/** A thread choice with the fields a card stands for. */
export type CardChoice = UnpackChoice & { count?: number; root?: Message | null; at?: number; started?: number; summary?: unknown; exact?: boolean; read?: boolean };

/** What packing and the side cards ask of the page. */
export interface PackControlsDependencies {
  leftEl(id: string): HTMLElement;
  selectedChannelId(): string;
  cardPacking: {
    mode(): string;
    setMode(mode: string, channel?: string): void;
    choose(channel: string, id: string, state: string): void;
    nextMode(): string;
    choiceOf(channel: string, id: string): string;
  };
  cardView(choice: CardChoice): { mode: string; cut: { head: number } | null; body(): Node | null };
  cardsCycle(): boolean;
  keepingCardPlace(change: () => void, still?: boolean): void;
  cardOpener(kind: string, replies: string, unread: string, open: () => void): HTMLElement;
  searchable(row: Element, text: string, name: string, root: unknown): void;
  threadName(choice: CardChoice, limit: number): string;
  guard(fn: () => void): () => void;
  enterCard(id: string, summary: unknown, row: Element): void;
  paintLitCard(row: Element, id: string): void;
  markReadingCard(row: Element): void;
  linkedMarkDrawOn(row: HTMLElement): void;
  keyContext(target: unknown): { row: HTMLElement | null };
  keepSelection(): void;
  setStatus(text: string): void;
  threadHue(id: string): string;
  readAloudNow(): { readingMode: boolean };
  readCardAloud(row: Element): void;
  toggleLitCard(id: string, row: Element): void;
  sayRowShortcut(el: HTMLElement, name: string): void;
  fullLocalAt(date: Date): string;
  columnsSideMode(): string;
  cardsDrawnForget(): void;
  sideColumnUp(): boolean;
  keepingCardStill(id: string, change: () => void, slide?: boolean): void;
  renderSideColumn(): void;
  markOf(pane: unknown): unknown;
  sidePaneValue: unknown;
  litCard(): string | null;
  layoutThreadList(): boolean;
}

/** The pack controls' commands: the legacy names, unchanged. */
export interface PackControls {
  renderUnpackToggle(): void;
  setUnpackMode(mode: string): void;
  setCardState(id: string, state: string): void;
  cycleCardState(choice: CardChoice, row: Element | null): void;
  packThread(id: string): void;
  packFromKeyboard(target?: unknown): boolean;
  cardState(choice: CardChoice, read: boolean): string;
  packButton(id: string): HTMLElement;
  unpackFromKeyboard(): void;
  cardRow(id: string): HTMLElement | null;
  sideCard(parts: { choice: CardChoice; view?: { mode: string; cut: { head: number } | null; body(): Node | null }; text: string; summarized: boolean; root?: unknown; age: string; replies: string; unread: string; read: boolean; state?: string; size?: { messages: number; exact: boolean } }): HTMLElement;
  countSideCard(row: HTMLElement, card: { choice: CardChoice; age: string; replies: string; unread: string; read: boolean; size?: { messages: number; exact: boolean } }): void;
  dispose(): void;
}

/** The pack controls over `deps`. */
export function createPackControls(deps: PackControlsDependencies): PackControls {
  const leftEl = deps.leftEl;
  const selectedChannelId = deps.selectedChannelId;
  const cardPacking = deps.cardPacking;
  const cardView = deps.cardView;
  const cardsCycle = deps.cardsCycle;
  const keepingCardPlace = deps.keepingCardPlace;
  const cardOpener = deps.cardOpener;
  const searchable = deps.searchable;
  const threadName = deps.threadName;
  const guardQuietly = deps.guard;
  const enterCard = deps.enterCard;
  const paintLitCard = deps.paintLitCard;
  const linkedMark = { drawOn: (row: HTMLElement) => deps.linkedMarkDrawOn(row) };
  const keyContext = deps.keyContext;
  const keepSelection = deps.keepSelection;
  const setStatus = deps.setStatus;
  const threadHue = deps.threadHue;
  const readAloudOwner = () => ({ readingMode: deps.readAloudNow().readingMode, readCardAloud: (row: Element) => deps.readCardAloud(row), markReadingCard: (row: Element) => deps.markReadingCard(row) });
  const toggleLitCard = deps.toggleLitCard;
  const sayRowShortcut = deps.sayRowShortcut;
  const fullLocalAt = deps.fullLocalAt;
  const columns = { side: { get mode() { return deps.columnsSideMode(); } } };
  const layoutShown = { threadList: () => deps.layoutThreadList() };
  const cardsDrawn = { forget: () => deps.cardsDrawnForget() };
  const sideColumnUp = deps.sideColumnUp;
  const keepingCardStill = deps.keepingCardStill;
  const renderSideColumn = deps.renderSideColumn;
  const markOf = deps.markOf;
  const sidePane = deps.sidePaneValue;
  const litCard = deps.litCard;
  let disposed = false;
  void disposed;


// `#432 read-mode-unify`. Unread only (`#259 thread-list-unread-filter`) was the cards' own filter, a
// chip and a choice kept on this device. It is the read mode now, as for every list: Hide read leaves the
// read threads out, Fold read folds them into runs (`foldCardRuns`), Show read shows them.

/** `filterSideCards`, the reader's place in the cards kept: the search typed, opened or closed. */


// --- unpack every thread under its card (#404 threads-unpack-all) ----------------------------------
//
// The owner, 2026-10-21: in All, **N replies** bridges one thread at a time — the children come up
// to meet the parent, which stays put — and bridging another thread breaks the first. He wants
// every thread chained at once in one stream. So in the Threads view (Cmd+3), which already lists
// threads by last updated, **Unpack msgs** (key u; "Unpack all" until `#421 chip-states`) draws each thread's messages under its card, in
// time order, joined by the same bridge All's gathering draws (`#215 reply-coalesce`), tinted by
// the thread's colour. The read modes apply inside each block as they do in a thread: under
// Fold read a run of read rows is one grey line hanging off the spine by its own stub; under
// Hide read read rows are absent. Blocks are later siblings of their card in `#side-cards`, never
// children, so every walk that knows cards by `data-card` is untouched. A thread whose replies the
// store does not hold in full is read only as its block nears the screen, at most a few at a time,
// and only its newest page: older history stays behind opening the thread, and its block says so.

// --- each thread packed or unpacked from its card (#440 thread-card-pack-toggle) -------------------------
//
// The owner, 2026-10-22: "I want to toggle pack/unpack on different threads the same way I toggle
// collapse/expand on messages. Currently CLICK on a thread card in thread list view lights up that card
// ... and dims the colored left edge of all the other threads. That's a behavior from the interleave All
// view that really doesn't make sense in the thread view ... a 3 state toggle. Collapsed, expanded card
// but no children, expanded card and expanded children ... a little up triangle to pack those messages back
// into the card." And: "let's also go 3-way with unpacking in thread list view ... The middle state will
// be to pack read threads and unpack any with non-zero unread child messages ... ONLY in this middle state
// I would also like to render as grey the colorful left sidebar of the READ cards".
//
//   * UNPACK, THREE WAYS: Pack all, Unpack unread (a thread with any unread message unpacked, a read one
//     packed, by `threadIsRead`), Unpack all; the chip says which, and a press or u goes on to the next.
//     Kept on the server for every device (`#457 settings-server-side`). Only in Unpack unread is a read card's colour strip grey, by the very rule that
//     greys the other cards while one is highlighted (web/voice.css).
//   * EACH THREAD, THREE WAYS, in the Threads view only: a click on its card, or Enter or o on it, goes
//     collapsed (three lines) -> whole, nothing unpacked -> whole with its messages unpacked -> collapsed.
//     A summarized card (`#405`), written to fit, skips the middle state: whole and three lines are the
//     same card. The Threads view highlights no card and dims none; two columns keep `#277`'s highlight.
//   * A THREAD'S CHOICE OUTRANKS UNPACK for that thread until Unpack is pressed again, which gives every
//     thread of the channel back to it. Kept on the server with Unpack (`THREAD_CARD_STATES_KEY`, `#457 settings-server-side`).
//   * PACKED IN PLACE: the up triangle in the first unpacked row's gutter, where All's bridge has its X, or
//     ^ on a thread's card or row, packs it, its card held where it is on the screen; a thread read under
//     Unpack unread packs where it is too (`keepingCardPlace`), its card staying in the list.

// Unpack's state and each thread's own choice of its card, and how both are kept, are
// web/src/present/rows' (`createCardPacking`: UNPACK_MODES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID,
// CARD_STATES, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY; `#624 voice-page-modularity`).


// --- unmatched runs among the thread cards ------------------------------------------------------
//
// `#439 search-unmatched-separators`. The cards are threads, and a thread is a container of
// messages: where the search leaves whole threads out between two it shows, the line counts the
// MESSAGES in them — "… 15 unmatched messages in 3 threads …" — each thread's from what the page
// knows of it (`cardMessageCount`), "at least" where a count is not exact. Main's own messages
// among the cards (`#419 threads-view-main-messages`) count as themselves, and a run holding any
// of them does not lay its whole count at the threads' door: it names no threads. A thread's
// unpacked messages (`#404 threads-unpack-all`) go with their card and are already in its count.
//
// DRAWN WHILE SEARCH WORDS CHOOSE CARDS, in both lists of cards: the right column's and the thread
// list's across the window (one list, `filterSideCards`), and the Threads list in one column
// (`applySearch`). While words are typed the read modes over the cards stand suspended with the
// messages' (`readRunsSuspended`, `hidingRead`), so a read thread the words leave out is counted
// here under the one word "unmatched", as the channel's gaps count what Links and whose messages
// leave out; Fold read's own lines over the cards (`foldCardRuns`) are the read runs', drawn only
// with no search choosing. The line is the channel gap's own (`.row-seam`, on the card after its
// run), and a tap opens its run the same way: shown, not matched, until the search changes.

/**
 * The opened card runs under the searches as they stand — thread ids, and the ids of Main's
 * messages among the cards — or null when there are none and `create` does not ask for them.
 * The read mode is in `searchKey`, since it chooses cards beside the words.
 *
 * @param {boolean} create
 * @returns {{threads: Set<string>, ids: Set<string>} | null}
 */

function cardState(choice: CardChoice, read: boolean): string {
  if (!cardsCycle()) return "collapsed";
  const own = cardPacking.choiceOf(String(selectedChannelId()), choice.id);
  if (own) return own;
  return cardPacking.mode() === "all" || (cardPacking.mode() === "unread" && !read) ? "unpacked" : "collapsed";
}

/**
 * The reads unpacking owes the threads — each read once, again only once its thread has grown past what
 * the read brought, `UNPACK_READS_AT_ONCE` at a time — are web/src/core/channels's (`createUnpackReads`,
 * `#624 voice-page-modularity`); the read itself, and drawing what it brought, are here.
 */

function renderUnpackToggle() {
  const button = leftEl("side-unpack");
  const offered = columns.side.mode === "list" && layoutShown.threadList();
  if (button.hidden === offered) button.hidden = !offered;
  const unpackMode = cardPacking.mode();
  const pressed = unpackMode === "all" ? "true" : unpackMode === "unread" ? "mixed" : "false";
  if (button.getAttribute("aria-pressed") !== pressed) button.setAttribute("aria-pressed", pressed);
  setTextIfChanged(button, UNPACK_MODE_NAMES[unpackMode] as string);
  const next = UNPACK_MODE_NAMES[cardPacking.nextMode()];
  const title = `${UNPACK_MODE_NAMES[unpackMode]}. Press for ${(next as string).toLowerCase()}`;
  if (button.title !== title) button.title = title;
}

/**
 * The reader pressed Unpack, or u: its next state, kept, every thread of the channel given
 * back to it (their own choices let go), and the cards drawn again where the reader is.
 */

function setUnpackMode(mode: string): void {
  cardPacking.setMode(mode, String(selectedChannelId()));
  renderUnpackToggle();
  cardsDrawn.forget();
  if (!sideColumnUp()) return;
  // `#459 pack-keeps-card-still`. The card the reader is plainly on held still, as one packed by itself is;
  // with none, their place kept as for any change to the cards.
  const still = stillCardOnScreen();
  if (still !== null) keepingCardStill(still, renderSideColumn);
  else keepingCardPlace(renderSideColumn);
}

/**
 * The card the reader is plainly on in the cards: the highlighted one (`#277 thread-card-focus`), else the
 * selected one, while it is on screen. Null for none, as with the selection on an unpacked row.
 */

function stillCardOnScreen(): string | null {
  if (!sideColumnUp() || columns.side.mode !== "list") return null;
  const area = leftEl("side-scroll");
  const top = area.getBoundingClientRect().top;
  const foot = top + (area.clientHeight || 0);
  const mark = markOf(sidePane) as { card: boolean; id: string } | null;
  for (const id of [litCard(), mark && mark.card ? mark.id : null]) {
    const row = id === null ? null : cardRow(id);
    if (!row || !rowShown(row)) continue;
    const box = row.getBoundingClientRect();
    if (box.bottom > top && box.top < foot) return id;
  }
  return null;
}

/** u, over the Threads view: Unpack's next state. Elsewhere, nothing. */

function unpackFromKeyboard(): boolean {
  if (!cardsCycle()) return false;
  setUnpackMode(cardPacking.nextMode());
  setStatus(UNPACK_MODE_SAID[cardPacking.mode()] as string);
  return true;
}

/**
 * Thread `id` drawn `state` by its own choice, outranking Unpack until it is pressed again, kept on this
 * device; the cards drawn again with its card held exactly where it is on the screen, past the foot of
 * the list where that is what it takes (`keepingCardStill`, `#459 pack-keeps-card-still`).
 */

function setCardState(id: string, state: string): void {
  cardPacking.choose(String(selectedChannelId()), id, state);
  // `#460 pack-gap-slide`: what is below the thread slides into its new place.
  keepingCardStill(id, () => {
    cardsDrawn.forget();
    renderSideColumn();
    // The card is drawn anew: the selection, its Tab stop and its focus go to the new one.
    keepSelection();
  }, true);
}

/** Thread `id`'s card in the cards on screen, or null. */

function cardRow(id: string): HTMLElement | null {
  return ([...leftEl("side-cards").children] as HTMLElement[])
    .find((one) => one.hasAttribute("data-card") && one.getAttribute("data-id") === id) || null;
}

/**
 * A press on the Threads view's card `row` of thread `choice`, or Enter or o on it: its next state, round
 * collapsed, whole and unpacked; a summarized card, the same whole and collapsed, round two.
 */

function cycleCardState(choice: CardChoice, row: Element | null): void {
  // `#528`: and a card waiting for its summary, which says no more whole; a clamped card still opens to K lines.
  const order = ["summary", "pending"].includes(cardView(choice).mode) ? ["collapsed", "unpacked"] : CARD_STATES;
  const now = String((row as Element).getAttribute("data-card-state") || "collapsed");
  const at = order.indexOf(now);
  const next = at < 0 ? "unpacked" : (order[(at + 1) % order.length] as string);
  setCardState(choice.id, next);
  setStatus(next === "unpacked" ? "Thread unpacked under its card." : next === "expanded" ? "Card shown whole." : "Thread packed.");
}

/**
 * Pack thread `id` into its card, from its triangle or ^: collapsed, its card held where it is. Whether
 * there was one to pack.
 */

function packThread(id: string): boolean {
  if (!cardRow(id)) return false;
  setCardState(id, "collapsed");
  setStatus("Thread packed.");
  return true;
}

/** ^, over the Threads view: pack the thread of the selected card or unpacked row. Elsewhere, nothing. */

function packFromKeyboard(target?: unknown): boolean {
  if (!cardsCycle()) return false;
  const { row } = keyContext(target);
  const id = row ? row.getAttribute("data-card") !== null ? row.getAttribute("data-id") : row.getAttribute("data-unpack-thread") : null;
  if (!id) return false;
  return packThread(id);
}

/** The up triangle in a thread's first unpacked row's gutter, where All's bridge has its X: packs the thread. */

function packButton(id: string): HTMLElement {
  const button = document.createElement("button");
  button.className = "reply-ungather unpack-pack";
  button.setAttribute("type", "button");
  button.setAttribute("aria-label", "Pack thread");
  button.setAttribute("title", "Pack this thread's messages back into its card (^)");
  const mark = document.createElement("span");
  mark.className = "reply-ungather-mark unpack-pack-mark";
  mark.setAttribute("aria-hidden", "true");
  button.append(mark);
  button.addEventListener("click", (event) => {
    // The row's own tap would fold the message or read it aloud; this tap is the triangle's.
    if (event) event.stopPropagation();
    packThread(id);
  });
  return button;
}



/** Whether thread `choice` still owes unpacking a read: held replies short of its count, uncovered. */

function sideCard({ choice, view = cardView(choice), text, summarized, root, age, replies, unread, read, state = "collapsed", size }: { choice: CardChoice; view?: { mode: string; cut: { head: number } | null; body(): Node | null }; text: string; summarized: boolean; root?: unknown; age: string; replies: string; unread: string; read: boolean; state?: string; size?: { messages: number; exact: boolean } }): HTMLElement {
  const row = document.createElement("li");
  row.className = "side-card-row";
  // `data-id` and `data-card`: the keys move through the cards as through messages, and know them apart.
  row.setAttribute("data-id", choice.id);
  row.setAttribute("data-card", "");
  // `#428 summary-cards-full`. The stylesheet draws such a card's text whole, at every
  // collapsed-lines count: the summary is not raw text and is never clamped.
  if (summarized) row.setAttribute("data-summarised-card", "");
  // `#522 thread-card-max-lines`. A root split for the card, its words already K card lines and "…".
  // `#528 thread-card-markdown-and-cap`: or held to K lines by the stylesheet, its root inward whole;
  // or saying its root's first lines while its summary is on its way.
  markCardLayout(row, view.mode, view.cut as never);
  row.style.setProperty("--thread-hue", threadHue(choice.id));
  const button = document.createElement("button");
  button.className = "side-card";
  button.setAttribute("type", "button");
  const body = document.createElement("span");
  body.className = "side-card-text";
  fillCardText(body, view, text);
  button.append(body);
  // `#440 thread-card-pack-toggle`. In the Threads view a press goes round the card's states; in two
  // columns it highlights the card, as it always did (`#277 thread-card-focus`).
  // `#532 shift-o-latency`: the thread as last drawn (`cardChoice`), the card being kept across passes.
  (row as HTMLLIElement & { cardChoice?: CardChoice }).cardChoice = choice;
  const now = () => (row as HTMLLIElement & { cardChoice?: CardChoice }).cardChoice as CardChoice;
  // `#540 read-aloud-desktop`: in reading mode a press reads the thread's first message aloud.
  const press = () => (readAloudOwner().readingMode ? readAloudOwner().readCardAloud(row)
    : cardsCycle() ? cycleCardState(now(), row) : toggleLitCard(now().id, row));
  button.addEventListener("click", press);
  sayRowShortcut(button, "side-card");
  row.setAttribute("data-card-state", state);
  const facts = document.createElement("span");
  facts.className = "side-card-facts";
  facts.textContent = age;
  // `#479 message-time-full-date`. Hovering a card's age says the day it last moved, whole.
  const lastMoved = Number.isFinite(choice.at) ? choice.at : choice.started;
  if (Number.isFinite(lastMoved)) facts.setAttribute("title", fullLocalAt(new Date(lastMoved as number)));
  // `#259 thread-list-unread-filter`: the card's "N unread" or "N new".
  if (unread) row.setAttribute("data-unread", "");
  // `#432 read-mode-unify`. Read: every message of its thread, root and replies, as the read modes see it.
  if (read) row.setAttribute("data-thread-read", "");
  const open = cardOpener("side-card", replies, unread, () => guardQuietly(() => enterCard(now().id, now().summary, row))());
  row.append(button, facts, open);
  // A press on the card's facts, or on its edge round the buttons, is a press on the card. Its two
  // buttons answer their own.
  row.addEventListener("click", (event) => {
    const at = event && event.target;
    if (at && at !== row && !nodeWithin(facts, at)) return;
    press();
  });
  // Where the search says it found the card in its replies (`markReplyMatch`).
  (row as HTMLLIElement & { cardFacts?: HTMLElement }).cardFacts = facts;
  // The thread's first message, which a copy of the card copies (`#296 copy-highlighted-message`). The
  // cards are drawn again whenever what they say changes, a first message arriving included.
  (row as HTMLLIElement & { cardRoot?: unknown }).cardRoot = choice.root;
  // `#439 search-unmatched-separators`. What a gap among the cards counts for this whole thread.
  (row as HTMLLIElement & { cardMessages?: number }).cardMessages = size ? size.messages : 1;
  (row as HTMLLIElement & { cardMessagesExact?: boolean }).cardMessagesExact = size ? size.exact : false;
  paintLitCard(row, choice.id);
  readAloudOwner().markReadingCard(row);
  // `#129 message-search`. A card stands in this list where messages stand in the others, so the
  // search reaches it, as it reaches the cards of the Threads list; and by its first message's words
  // where the card says a summary of them. `#262 search-load-older`.
  searchable(row, text, threadName(choice, Infinity), root);
  // `#890 voice-page-linked-highlight`. A card whose first message is the linked one is marked as its row is.
  linkedMark.drawOn(row);
  return row;
}

/**
 * `#532 shift-o-latency`. Card `row`, drawn by `sideCard`, brought up to date where only what it counts has
 * changed (`CardParts`): how long since its thread moved, its replies and unread, whether it is read, and
 * the size a gap among the cards counts for it. In place, as `sideCard` would draw them.
 *
 * @param {HTMLElement} row
 * @param {{choice: any, age: string, replies: string, unread: string, read: boolean, size: {messages: number, exact: boolean}}} card
 */

function countSideCard(row: HTMLElement, { choice, age, replies, unread, read, size }: { choice: CardChoice; age: string; replies: string; unread: string; read: boolean; size?: { messages: number; exact: boolean } }): void {
  (row as HTMLLIElement & { cardChoice?: CardChoice }).cardChoice = choice;
  const facts = (row as HTMLLIElement & { cardFacts?: HTMLElement }).cardFacts;
  if (facts) {
    setTextIfChanged(facts, age);
    const lastMoved = Number.isFinite(choice.at) ? choice.at : choice.started;
    if (Number.isFinite(lastMoved)) setAttributeIfChanged(facts, "title", fullLocalAt(new Date(lastMoved as number)));
  }
  if (row.hasAttribute("data-unread") !== Boolean(unread)) row.toggleAttribute("data-unread", Boolean(unread));
  if (row.hasAttribute("data-thread-read") !== Boolean(read)) row.toggleAttribute("data-thread-read", Boolean(read));
  const was = childByClass(row, "side-card-open");
  if (was) was.replaceWith(cardOpener("side-card", replies, unread, () => guardQuietly(() => enterCard(choice.id, choice.summary, row))()));
  (row as HTMLLIElement & { cardMessages?: number }).cardMessages = size ? size.messages : 1;
  (row as HTMLLIElement & { cardMessagesExact?: boolean }).cardMessagesExact = size ? size.exact : false;
}

// --- one card highlighted ------------------------------------------------------------------------
//
// `#277 thread-card-focus`. The owner, 2026-10-13, of the thread cards: "It is VERY busy with all the
// colored lines... Let's make it so when we highlight a thread card all the other lines grey out and we
// can see what we've highlighted. I also don't like the cards expanding. Let's leave the thread cards
// contracted. Let's CLICK to put a persistent highlight and expand on one card (NOT enter the thread).
// To enter the thread you can click the 'N reply M unread' bit let's say or press '->' to go into the
// thread."
//
// The cards are the right column's in two columns and the Threads list's in one, and both work so.
// (`#281 view-keys` made the thread list, Ctrl+3, the right column's cards across the window, in one
// column and on a phone: they work so there too, as they do on the right.)
//
//   * EVERY CARD STAYS CONTRACTED: three lines on the right, the thread's name in the list. `#257
//     summary-truncation` showed the whole of the card under the mouse or the keys, and with the mouse
//     moving over the cards that was a card growing and shrinking under it at every move.
//   * A PRESS ON A CARD HIGHLIGHTS IT — a click anywhere on it but its replies button, or Enter on it
//     with the keys or Tab — and shows it whole. It stays: the mouse moving over the cards, the keys
//     walking through them and the cards drawn again leave it where it is. Another card pressed takes
//     it, and the same one pressed again or Esc puts it out. One per channel, for the session.
//   * WHILE ONE IS HIGHLIGHTED, EVERY OTHER CARD'S EDGE AND UNREAD COUNT GO GREY, and so does every
//     other link between the columns, the highlighted card's drawn over them: the stylesheet greys the
//     cards while the highlighted one shows (a card a filter hides greys nothing), and the links are
//     greyed where they are drawn (`planColumnLinks`, web/src/view/channel/columnlinks.ts).
//   * THE WAY IN is the card's "3 replies · 2 unread", a button of its own (`cardOpener`), or →: into
//     the highlighted card's thread, or with none the card the keys are on — and on a card's own
//     button, where Tab leaves the focus, that card's (`cardForArrow`). In two columns ← comes back out
//     to the cards, the keys on the card entered, as Esc and the X come back to them; in one column ←,
//     Esc and Back come back to the Threads list, as they always did.

/**
 * By channel, the thread whose card is highlighted, for this session.
 *
 * @type {Map<string, string>}
 */

  return {
    renderUnpackToggle, setUnpackMode, setCardState, cycleCardState, packThread,
    packFromKeyboard, unpackFromKeyboard, cardRow, sideCard, countSideCard, cardState, packButton,
    dispose() { disposed = true; },
  };
}

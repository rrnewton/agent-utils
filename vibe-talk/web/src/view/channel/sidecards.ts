// The right column's cards (`#231 two-column-polish`, `#261 two-column-connectors`, `#262
// search-load-older`, `#277 thread-card-focus`, `#281 view-keys`, `#419
// threads-view-main-messages`, `#426 threads-sort-toggle`, `#428 summary-cards-full`, `#432
// read-mode-unify`, `#439 search-unmatched-separators`, `#440 thread-card-pack-toggle`, `#466
// reply-promotes-thread`, `#480 fold-one-message-key`, `#489 card-root-lines`, `#491
// threads-view-sent-receipt`, `#500 sent-receipts-pile-up`, `#522 thread-card-max-lines`, `#528
// thread-card-markdown-and-cap`, `#532 shift-o-latency`; `#624 voice-page-modularity`, moved
// from the legacy script into view/channel for the plan's step 17): what each thread's card
// says and counts, and the pass that draws them, keeping what a pass that changes nothing
// drew. The tallies themselves (how many replies, what is unseen) stay the counts' owner's;
// they are handed in. The store, the rows and the unpacked blocks are handed in too.

import type { Message } from "../../contract/index.ts";
import { THREAD_ORDERS, renderedHtmlOf, rawTextOf } from "../../present/rows/index.ts";
import { createCardsDrawn } from "./cardsdrawn.ts";
import type { CardsDrawn } from "./cardsdrawn.ts";

/** A thread as its card reads it. */
export interface SideChoice {
  id: string;
  root?: Message | null;
  count?: unknown;
  summary?: { reply_count_exact?: boolean } | null;
  at?: number;
  started?: number;
}

/** The reply tally, as a card reads it. */
export interface SideTally {
  threads: Map<string, { unread: number; held: number }>;
}

/** One thread as the cards draw it, in parts (`sideCardParts`). */
export interface CardParts {
  card: string;
  counts: string;
  block: string;
  read: string;
  fold: string;
}

/** A card's whole input (`sideCardParts`). */
export interface SideCard {
  choice: SideChoice;
  view: { mode: string; cut?: { head: string; whole: string } | null };
  text: string;
  summarized: boolean;
  root: string;
  age: string;
  replies: string;
  unread: string;
  read: boolean;
  state: string;
  size: { messages: number; exact: boolean };
}

/** What the side cards are handed by the page. */
export interface SideCardsDependencies {
  leftEl(id: string): HTMLElement;
  cardView(choice: SideChoice): { text: string; mode: string; cut?: { head: string; whole: string } | unknown };
  messageText(message: Message): string;
  stillToDo(root: Message, dismissed: Set<string>): boolean;
  channelCanon(): { channel: string; dismissed: Set<string> };
  sideColumn(): unknown;
  inColumn<T>(column: unknown, act: () => T): T;
  unseenReplies(id: string, root: Message | null | undefined, tally: SideTally, summary: SideChoice["summary"]): number;
  repliesPartlyHeld(id: string, root: Message | null | undefined, held: number, tally: SideTally): boolean;
  exactReplyCount(id: string, root: Message | null | undefined, tally: SideTally, summary?: SideChoice["summary"]): number | null;
  pendingInto(id: string): number;
  threadCount(count: number, exact: boolean | undefined): string;
  unpackHeldMessages(canon: unknown, choice: SideChoice, sameChannel: boolean): readonly Message[];
  unpackSignature(choice: SideChoice, held: readonly Message[]): string;
  unpackReadSignature(held: readonly Message[], dismissed: Set<string>, filter: unknown): string;
  unpackFilter(): unknown;
  foldedOneIds(key: string): Iterable<string>;
  viewKey(view: string, id?: string): string;
  threadChoices(): SideChoice[];
  inThreadOrder(choices: readonly SideChoice[], order: string): SideChoice[];
  cardOrderOnScreen(): string;
  watchPendingSummaries(choices: readonly SideChoice[], listId: string): void;
  cardIsSummarized(choice: SideChoice): boolean;
  briefAge(at: number): string;
  threadName(choice: SideChoice, limit: number): string;
  unpackWatch(): { unwatch(child: Element): void; stop(): void };
  dressChannelRow(row: HTMLElement, pass: unknown): void;
  answeredBy(rows: readonly HTMLElement[], viewing: string): unknown;
  replyTally(): SideTally;
  readMode(): string;
  readRunsSuspended(): boolean;
  cardPackingMode(): string;
  combineOn(): boolean;
  nowPlaying(): { id: string } | null;
  pendingRead(): unknown;
  markReadingCard(card: HTMLElement): void;
  mainRowsOnScreen(): boolean;
  mainRowMessages(except: Set<string>): Message[];
  messageLook(message: Message): string;
  readLook(message: Message, dismissed: Set<string>, filter: unknown): string;
  olderThreadsToRead(): boolean;
  keepingCardPlace(act: () => void): void;
  sideCard(card: SideCard): HTMLElement;
  countSideCard(card: HTMLElement, cardInput: SideCard): void;
  paintLitCard(card: HTMLElement, id: string): void;
  unpackBlock(choice: SideChoice, tally: SideTally): HTMLElement[];
  redrawUnpackBlock(choice: SideChoice, rows: HTMLElement[], tally: SideTally): void;
  doneArchived(): Set<string>;
  setDoneArchived(archived: Set<string>): void;
  caughtUpDone(archived: Set<string>, shown: readonly string[]): Set<string>;
  mainRunsAmong(main: readonly Message[], times: readonly number[]): Message[][];
  mainRows(run: readonly Message[], tally: SideTally): HTMLElement[];
  foldCardRuns(list: HTMLElement): void;
  filterSideCards(): void;
  cardsCycle(): boolean;
  sideCardsShown(): boolean;
  outboxValues(): Iterable<{ state: string; inPlace?: unknown }>;
  eachColumn(act: () => void): void;
  renderOutgoingMessages(): void;
  replyState(): { docked: boolean; pending: unknown };
  placeInlineReply(): void;
  reopenPendingReply(): void;
  setTextIfChanged(el: HTMLElement, text: string): void;
  threadPagesLoading(): boolean;
  selectedChannelId(): unknown;
  cardState(choice: SideChoice, read: boolean): string;
}

/** The side cards, as the page calls them. */
export interface SideCards {
  readonly cardsDrawn: CardsDrawn<{ parts: CardParts; card: HTMLElement; rows: HTMLElement[] }, HTMLElement[]>;
  nameSideRegion(name: string): void;
  sideCardText(choice: SideChoice): string;
  cardLines(message: Message): string;
  sideCardRootText(choice: SideChoice): string;
  threadIsRead(choice: SideChoice, unread: string): boolean;
  sideCardUnread(choice: SideChoice, tally: SideTally): string;
  sideCardReplies(choice: SideChoice, tally: SideTally): string;
  cardMessageCount(choice: SideChoice, tally: SideTally): { messages: number; exact: boolean };
  sideCardParts(card: SideCard): CardParts;
  placeChildren(list: HTMLElement, wanted: readonly HTMLElement[]): void;
  redressMainRows(rows: readonly HTMLElement[], tally: SideTally): void;
  renderSideCards(): void;
}

/** Make the side cards from the page's store, tallies and rows. */
export function createSideCards(deps: SideCardsDependencies): SideCards {
  const cardsDrawn = createCardsDrawn<{ parts: CardParts; card: HTMLElement; rows: HTMLElement[] }, HTMLElement[]>();
  function nameSideRegion(name: string): void {
    const region = deps.leftEl("side-scroll");
    if (region.getAttribute("aria-label") !== name) region.setAttribute("aria-label", name);
  }

  /**
   * What a card says of its thread: "display_name: summary" when the summariser has named it and summed
   * it up, else the text of its first message, else its name. Whitespace is folded, so a raw card's
   * clamped lines show lines of words rather than a paragraph break; a summarised card's words are
   * drawn whole besides (`#428 summary-cards-full`).
   */
  function sideCardText(choice: SideChoice): string {
    // `#522 thread-card-max-lines`, `#528 thread-card-markdown-and-cap`: a root longer than the card
    // shows, its first lines and "…" (`cardView`).
    return deps.cardView(choice).text;
  }

  /**
   * `#489 card-root-lines`. A message's words as a card shows them in place of a summary: line by line
   * as its body is drawn (a line end at each line break and each block's edge, `messageText`), each
   * line's white space closed up and blank lines dropped. The owner, 2026-10-22: a card showing his
   * "header / ----- / body" as one squashed line should "render properly"; the stylesheet keeps these
   * line ends (`white-space: pre-line` on the card's text).
   */
  function cardLines(message: Message): string {
    return deps.messageText(message).split("\n").map((line) => line.replace(/\s+/g, " ").trim()).filter(Boolean).join("\n");
  }

  /**
   * The words of a thread's first message, for the search over its card, when the card says something
   * else — the summariser's name and summary — or "". `#262 search-load-older`: a summed-up thread is
   * found by what its first message says too, not only by the summary of it.
   */
  function sideCardRootText(choice: SideChoice): string {
    const first = choice.root ? cardLines(choice.root) : "";
    return first && first !== sideCardText(choice) ? first : "";
  }

  /**
   * Whether thread `choice` is read, as the read modes see a thread (`#432 read-mode-unify`): no unread
   * reply held or reported (`unread`, `sideCardUnread`'s words) and its root read too. One unread message,
   * the root or any reply, makes the whole thread unread, packed under its card or unpacked.
   */
  function threadIsRead(choice: SideChoice, unread: string): boolean {
    return unread === "" && !(choice.root && deps.stillToDo(choice.root, deps.channelCanon().dismissed));
  }

  /** What a card says of its thread's unread replies, as a root on Main says it ("3 unread", "1 new"), or "". */
  function sideCardUnread(choice: SideChoice, tally: SideTally): string {
    const entry = tally.threads.get(choice.id);
    const unread = entry ? entry.unread : 0;
    const unseen = deps.unseenReplies(choice.id, choice.root, tally, choice.summary);
    if (unread > 0) return `${unread}${deps.repliesPartlyHeld(choice.id, choice.root, entry ? entry.held : 0, tally) ? "+" : ""} unread`;
    return unseen > 0 ? `${unseen} new` : "";
  }

  /**
   * What a card says of its thread's replies, "3 replies" or "about 3 replies", or "" with none.
   *
   * `#231 two-column-polish`. By the rule of the thread's tag on a root in Main (`drawThreadChips`):
   * the count reported, never fewer than the replies the store holds nor than an exact count the
   * store's copy of the root gives. A reply that comes in on the stream is counted on its thread's
   * summary and root as it is folded in (`foldLiveMessage`), so the card counts it with its unread
   * chip, not up to a minute later at the thread list's next read.
   */
  function sideCardReplies(choice: SideChoice, tally: SideTally): string {
    const entry = tally.threads.get(choice.id);
    const reported = typeof choice.count === "number" ? choice.count : 0;
    // `#466 reply-promotes-thread`: with the sends in the outbox going into it, as its root's tag counts.
    const count = Math.max(reported, entry ? entry.held : 0, deps.exactReplyCount(choice.id, choice.root, tally) || 0) +
      deps.pendingInto(choice.id);
    return count > 0 ? deps.threadCount(count, choice.summary ? choice.summary.reply_count_exact : true) : "";
  }

  /**
   * How many messages thread `choice` holds, the root counted, and whether the page knows exactly.
   * `#439 search-unmatched-separators`: what a gap among the cards counts for a whole thread. Never
   * more than is known: an inexact reported count is left out rather than rounded into a claim, and
   * the count is then the replies this page holds and no more — which the line says as "at least".
   * By the same rule as `sideCardReplies`, so a gap and the card it stands for never disagree where
   * the count is exact.
   *
   * @returns {{messages: number, exact: boolean}}
   */
  function cardMessageCount(choice: SideChoice, tally: SideTally): { messages: number; exact: boolean } {
    const entry = tally.threads.get(choice.id);
    const held = entry ? entry.held : 0;
    const exactHeld = deps.exactReplyCount(choice.id, choice.root, tally, choice.summary);
    const reported = typeof choice.count === "number" ? choice.count : null;
    // The count's own source decides its exactness, as `exactCount` decides it: the summary's flag
    // where the count is the summary's, and the root record's where it is the record's — a
    // summary-less thread can still carry a count its record says is not exact.
    const reportedExact = reported !== null && (choice.summary
      ? choice.summary.reply_count_exact !== false
      : !(choice.root && choice.root.thread && choice.root.thread.reply_count_exact === false));
    const known = Math.max(exactHeld === null ? -1 : exactHeld, reportedExact && reported !== null ? reported : -1);
    // Exact only where an exact source accounts for every reply held: one gone stale below what
    // this page holds says the thread has grown since, and how far is not known.
    return { messages: Math.max(held, known) + 1, exact: known >= held && known >= 0 };
  }

  /**
   * One thread as the cards draw it, in parts (`renderSideCards`, `#532 shift-o-latency`): `card`, what its
   * card says; `counts`, what it counts (its age, replies, unread and whether it is read, which a card
   * already drawn takes in place, `countSideCard`); `block`, what its unpacked block draws
   * (`unpackSignature`); `read`, which of the block's messages are still to do (`unpackReadSignature`); and
   * `fold`, which of them the reader folded away one by one (`#480 fold-one-message-key`). Each of the
   * last three is "" for a thread not unpacked.
   *
   * @typedef {{card: string, counts: string, block: string, read: string, fold: string}} CardParts
   */


  /**
   * One thread's parts as a pass over the cards finds them (`CardParts`).
   *
   * @param {{choice: any, view: any, text: string, summarized: boolean, root: string, age: string, replies: string, unread: string, read: boolean, state: string, size: {messages: number, exact: boolean}}} card
   * @returns {CardParts}
   */
  function sideCardParts(card: SideCard): CardParts { const { choice, view, text, summarized, root, age, replies, unread, read, state, size } = card;
    // Everything `sideCard` draws from but its counts: the name the search finds it by, where its root is
    // cut, and the root's body the card draws. The highlight is painted on a kept card in place.
    const parts = { card: JSON.stringify([choice.id, view.mode, text, summarized, root, state, deps.threadName(choice, Infinity),
      view.cut ? [view.cut.head, view.cut.whole] : null, choice.root ? renderedHtmlOf(choice.root) || rawTextOf(choice.root) : ""]),
      counts: JSON.stringify([age, replies, unread, read, size.messages, size.exact]), block: "", read: "", fold: "" };
    if (state !== "unpacked") return parts;
    const held = deps.unpackHeldMessages(deps.channelCanon(), choice, deps.channelCanon().channel === String(deps.selectedChannelId()));
    return { ...parts, block: deps.unpackSignature(choice, held), read: deps.unpackReadSignature(held, deps.channelCanon().dismissed, deps.unpackFilter()),
      fold: [...deps.foldedOneIds(deps.viewKey("thread", choice.id))].sort().join(",") };
  }

  /**
   * Put `wanted` in `list` as its children, in that order, moving only what is out of place: an element
   * left where it was keeps its box, so the browser lays out only what changed (`#532 shift-o-latency`).
   * What is not wanted goes, and an unpacked block's loading line going is no longer watched.
   *
   * @param {HTMLElement} list
   * @param {HTMLElement[]} wanted
   */
  function placeChildren(list: HTMLElement, wanted: readonly HTMLElement[]): void {
    const keep = new Set(wanted);
    for (const child of [...list.children]) {
      if (keep.has(child as HTMLElement)) continue;
      deps.unpackWatch().unwatch(child);
      list.removeChild(child);
    }
    let at = list.firstElementChild;
    for (const node of wanted) {
      if (at === node) {
        at = at.nextElementSibling;
        continue;
      }
      list.insertBefore(node, at);
    }
  }

  /**
   * Main's message rows kept from the last pass (`renderSideCards`), dressed again from what the page holds
   * now (`dressChannelRow`), as a new row would be: a pin, a flash or a reply since they were drawn.
   *
   * @param {HTMLElement[]} rows
   * @param {ReplyTally} tally
   */
  function redressMainRows(rows: readonly HTMLElement[], tally: SideTally): void {
    const viewing = String(deps.selectedChannelId());
    const pass = { tally, answered: deps.answeredBy(rows, viewing), viewing, inMain: false, inAll: false };
    for (const row of rows) deps.dressChannelRow(row, pass);
  }

  /**
   * One card per thread the channel's store knows, each a fixed three lines of what the thread is about,
   * then how long since it moved, and the button into it saying how many replies it has and how many of
   * them are unread. A press on a card highlights it (`#277 thread-card-focus`); its button goes into the
   * thread — selected on the right in two columns, and from the thread list across the window
   * (`enterCard`). Redrawn only when what it would draw has changed, so a refresh with nothing new draws
   * nothing here either. `#281 view-keys`: the one rendering of the channel's threads, in two columns and
   * in the thread list alike.
   *
   * `#261 two-column-connectors`. NEWEST ACTIVITY AT THE FOOT, as the channel's own lists have their
   * newest message: the owner, "We have the main chat views in forward-chronological mode with newest at
   * bottom. But now it's flipped with newest at top on threads list. I want the newest thread on the
   * bottom and us default scrolled to the bottom of the list." So a thread that moves goes to the foot,
   * a reader there follows it, and a reader up the list keeps the card they were on where it was
   * (`keepingCardPlace`). The way to older threads is at the head, above the oldest card.
   */
  function renderSideCards(): void {
    const list = deps.leftEl("side-cards");
    const tally = deps.replyTally();
    // `#426 threads-sort-toggle`. In the order chosen over the Threads view's cards, else the default.
    const order = deps.cardOrderOnScreen();
    const choices = deps.inThreadOrder(deps.threadChoices(), order);
    // `#528 thread-card-markdown-and-cap`. A card waiting for its summary reads the Threads list again soon.
    deps.watchPendingSummaries(choices, deps.leftEl("side-cards").id);
    const cards = choices.map((choice) => ({
      choice,
      view: deps.cardView(choice) as SideCard["view"],
      text: sideCardText(choice),
      // `#428 summary-cards-full`. With the text, so a summary arriving with the same words as the
      // root's still redraws the card unclamped (below, and in the signature).
      summarized: deps.cardIsSummarized(choice),
      root: sideCardRootText(choice),
      age: deps.briefAge(Number.isFinite(choice.at) ? (choice.at as number) : (choice.started as number)),
      replies: sideCardReplies(choice, tally),
      unread: sideCardUnread(choice, tally),
      size: cardMessageCount(choice, tally),
    })).map((card) => ({ ...card, read: threadIsRead(card.choice, card.unread) }))
      // `#440 thread-card-pack-toggle`. Each thread's card, collapsed, whole or unpacked (`cardState`).
      .map((card) => ({ ...card, state: deps.cardState(card.choice, card.read) }));
    const unpacked = cards.some((card) => card.state === "unpacked");
    // `#419 threads-view-main-messages`. And the main channel's messages that are not threads, among them.
    const main = deps.mainRowsOnScreen() ? deps.mainRowMessages(new Set(cards.map(({ choice }) => choice.id))) : [];
    // `#532 shift-o-latency`. What the whole list is drawn under, then each thread's card and block, then
    // Main's messages: a pass that changes one thread draws that thread again and keeps every other one.
    const whole = JSON.stringify([order, deps.readMode(), deps.readRunsSuspended(), deps.cardPackingMode(),
      main.length ? [deps.readMode(), deps.combineOn(), (deps.nowPlaying()?.id ?? ""), deps.pendingRead()] : ""]);
    const parts = cards.map(sideCardParts);
    const signature = JSON.stringify([whole, parts, main.map((message) => [deps.messageLook(message), deps.readLook(message, deps.channelCanon().dismissed, deps.unpackFilter())])]);
    const more = deps.olderThreadsToRead();
    if (cardsDrawn.changed(signature)) {
      // Kept only from a pass under the same whole, and never where the cards were asked to be drawn anew
      // (`cardsDrawn.forget`: something the signatures do not say has changed).
      const keep = cardsDrawn.redraw(signature, whole);
      deps.keepingCardPlace(() => {
        if (!keep) deps.unpackWatch().stop();
        const kept = cards.map((card) => (keep ? cardsDrawn.kept().threads.get(card.choice.id) : undefined));
        // A card that says the same is kept, its counts brought up to date where they changed: putting a new
        // element in the list costs its every row laid out again, a count changed in place does not.
        const cardRows = cards.map((card, at) => {
          const was = kept[at];
          const nowParts = parts[at] as CardParts;
          if (!was || was.parts.card !== nowParts.card) return deps.sideCard(card);
          if (was.parts.counts !== nowParts.counts) deps.countSideCard(was.card, card);
          else (was.card as HTMLElement & { cardChoice: unknown }).cardChoice = card.choice;
          // Highlighted or put out in place (`toggleLitCard`), and by the view since: as a card drawn now.
          deps.paintLitCard(was.card, card.choice.id);
          deps.markReadingCard(was.card);
          return was.card;
        });
        // `#407 unpack-row-parity`. Unpacked rows are built in the right column, the column of the cards
        // the keys and the mouse move them in (`sidePane`), its read state brought into line with the
        // store's for every message drawn, as each thread's own would be: a key acting on a row asks it.
        const blocks = unpacked ? deps.inColumn(deps.sideColumn(), () => {
          const shown = cards.filter((card) => card.state === "unpacked")
            .flatMap((card) => deps.unpackHeldMessages(deps.channelCanon(), card.choice, deps.channelCanon().channel === String(deps.selectedChannelId())).map((message) => String(message.id)));
          deps.setDoneArchived(deps.caughtUpDone(deps.doneArchived(), shown));
          return cards.map((card, at) => {
            const was = kept[at];
            const now = parts[at] as CardParts;
            if (card.state !== "unpacked") return [];
            if (!was || was.parts.block === "" || was.parts.block !== now.block) return deps.unpackBlock(card.choice, tally);
            // The same block, but for its messages' read state or what was folded away one by one: its rows
            // dressed again and their runs placed again where they are, in the frame of the act.
            if (was.parts.read !== now.read || was.parts.fold !== now.fold) {
              deps.redrawUnpackBlock(card.choice, was.rows, tally);
              // Placing the runs can remember a run opened as it grew (`readRunPlan`), which the block's
              // signature says: read again, so the next pass finds it the same.
              now.block = deps.unpackSignature(card.choice, deps.unpackHeldMessages(deps.channelCanon(), card.choice, deps.channelCanon().channel === String(deps.selectedChannelId())));
            }
            return was.rows;
          });
        }) : [];
        /** @type {ReturnType<typeof cardsDrawn.kept>["threads"]} */
        const threads = new Map(cards.map((card, at) =>
          [card.choice.id, { parts: parts[at] as CardParts, card: cardRows[at] as HTMLElement, rows: unpacked ? (blocks[at] as HTMLElement[]) : [] }] as const));
        const drawn = cards.map((_card, at) => [cardRows[at] as HTMLElement, ...(unpacked ? (blocks[at] as HTMLElement[]) : [])]);
        // `#419 threads-view-main-messages`. Main's messages in the list's order: each run of them between
        // two cards, by when it was posted against when each card's thread last moved. Only they are drawn
        // as the right column's rows; the cards are drawn as before. A run drawn the same is kept, dressed
        // again from what the page holds now.
        const runs = deps.mainRunsAmong(main, cards.map((card) => (THREAD_ORDERS[order] as { time(choice: SideChoice): number }).time(card.choice)));
        /** @type {Map<string, HTMLElement[]>} */
        const mainKept = new Map();
        const between = main.length ? deps.inColumn(deps.sideColumn(), () => {
          deps.setDoneArchived(deps.caughtUpDone(deps.doneArchived(), main.map((message) => String(message.id))));
          return runs.map((run) => {
            if (run.length === 0) return [];
            // By its messages as drawn: a Done or a put back is in how a row is dressed, done again below.
            const key = JSON.stringify(run.map(deps.messageLook));
            const was = keep ? cardsDrawn.kept().main.get(key) : undefined;
            const rows = was || deps.mainRows(run, tally);
            if (was) redressMainRows(rows, tally);
            mainKept.set(key, rows);
            return rows;
          });
        }) : runs.map(() => []);
        const wanted = [...drawn.flatMap((rows, at) => [...(between[at] as HTMLElement[]), ...rows]), ...(between[drawn.length] as HTMLElement[])];
        if (keep) placeChildren(list, wanted);
        else list.replaceChildren(...wanted);
        cardsDrawn.keep({ whole, threads, main: mainKept });
        // `#432 read-mode-unify`. Under Fold read, the runs of read threads and Main's read messages.
        deps.foldCardRuns(list);
        deps.filterSideCards();
      });
    } else {
      deps.filterSideCards();
    }
    // `#440 thread-card-pack-toggle`. Unpack's state over the Threads view's cards, where the stylesheet greys
    // a read card under Unpack unread; nothing over two columns' cards.
    const mode = deps.cardsCycle() ? deps.cardPackingMode() : "";
    if ((list.getAttribute("data-unpack") || "") !== mode) list.setAttribute("data-unpack", mode);
    // Above the cards, so the reader's place is kept as it comes or goes.
    const button = deps.leftEl("side-more-threads") as HTMLButtonElement;
    if (button.hidden === more) {
      deps.keepingCardPlace(() => {
        button.hidden = !more;
      });
    }
    // `#281 view-keys`. It reads where it stands now, and says so while it does.
    const loading = deps.threadPagesLoading();
    deps.setTextIfChanged(button, loading ? "Loading older threads…" : "Older threads…");
    if (button.disabled !== loading) button.disabled = loading;
    // `#491 threads-view-sent-receipt`. A sent reply these cards now draw takes its receipt away: the
    // outbox was drawn before them on a pass over the rows, and an unpack read draws them on its own.
    // `#500 sent-receipts-pile-up`: and a reply's row drawn under the row it answers here is placed
    // again under that row as now drawn.
    if (deps.sideCardsShown() && [...deps.outboxValues()].some((entry) => entry.state === "sent" || entry.inPlace)) {
      deps.eachColumn(deps.renderOutgoingMessages);
    }
    // `#410 inline-reply-editor`. A reply open under a row unpacked here, under it as the rows now stand;
    // one waiting for its message, opened again once the message is drawn here.
    if (deps.replyState().docked) deps.placeInlineReply();
    else if (deps.replyState().pending) deps.reopenPendingReply();
  }



  return {
    cardsDrawn,
    nameSideRegion, sideCardText, cardLines, sideCardRootText, threadIsRead, sideCardUnread,
    sideCardReplies, cardMessageCount, sideCardParts, placeChildren, redressMainRows, renderSideCards,
  };
}

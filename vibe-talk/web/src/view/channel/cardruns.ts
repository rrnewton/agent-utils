// The search over the cards, the runs of read threads over them, and the links between Main's
// roots and their cards (`#129 message-search`, `#439 search-unmatched-separators`, `#432
// read-mode-unify`, `#261 two-column-connectors`; `#624 voice-page-modularity`, moved from the
// legacy script). The read mode is only ever read here, through a handed-in question; nothing
// about how the reader cycles it moves or changes. The selection's following of the links
// (`noteLinkedSelection`) is the links owner's own act, called as it was. Words, order and
// timing are as they were in the script.

import { cardRunText } from "../../present/rows/index.ts";
import { setClassWord, setTextIfChanged } from "../dom/index.ts";
import { cardRunIds } from "./cards.ts";
import { createColumnLinks } from "./columnlinks.ts";
import type { ColumnLinks, LinkPane } from "./columnlinks.ts";
import { idsOf } from "./rows.ts";

/** A run's place for an item, as the read runs' plan hands it on. */
export interface CardRunPlace {
  readonly role: string | null;
}

/** What the card runs ask of the page. */
export interface CardRunsDependencies {
  leftById(id: string): HTMLElement;
  // The search and read filter over the cards.
  noteSuspendedRuns(): boolean;
  unpackOnScreen(): boolean;
  readMode(): string;
  readRunsSuspended(): boolean;
  forgetCards(): void;
  renderSideCards(): void;
  searchOpen(): boolean;
  searchNode(): unknown;
  searchTerms(): string[];
  threadsWithHits(node: unknown): ReadonlySet<string> | null;
  openedCardGaps(create: boolean): { ids: ReadonlySet<string>; threads: ReadonlySet<string> } | null;
  hidingRead(): boolean;
  rowMatches(row: HTMLElement, node: unknown, text: string | null): boolean;
  markReplyMatch(row: HTMLElement, inReplies: boolean): void;
  markSearchHits(row: HTMLElement, terms: readonly string[]): void;
  olderThreadsFailure(): string;
  allLoaded(): boolean;
  setCardsCount(cards: number, shown: number): void;
  threadListOnScreen(): boolean;
  sayCardCount(): void;
  renderRepliesSearch(): void;
  drawCardGaps(list: HTMLElement): void;
  inOpenedGap(row: HTMLElement, ids: ReadonlySet<string>): boolean;
  // The runs of read threads over the cards.
  selectedChannelId(): unknown;
  rowIsRead(row: HTMLElement): boolean;
  readRunPlan(items: readonly HTMLElement[], key: string, isRead: (row: HTMLElement) => boolean, ids: typeof cardRunIds): Map<HTMLElement, CardRunPlace>;
  readRunLine(row: HTMLElement): HTMLElement;
  placeInReadRun(row: HTMLElement, role: string | null, count: number): void;
  placeReadRunPeel(row: HTMLElement, count: number): void;
  keepingCardPlace(change: () => void): void;
  // The links between Main's roots and their cards.
  makeColumnLinksDeps(): {
    onTwoColumns(): boolean;
    sideMode(): string;
    mainList(): HTMLElement | null;
    openThread(): { readonly id: string; readonly root?: unknown } | null;
    activePane(): LinkPane | null;
    sidePane(): LinkPane;
    litCard(): string | null;
    cardRow(thread: string): HTMLElement | null;
    hue(thread: string): number;
  };
}

/** The card runs' acts and the links they own. */
export interface CardRuns {
  readonly columnLinks: ColumnLinks;
  /** The key the cards' opened runs are kept under, by channel (`readRuns`). */
  cardRunKey(): string;
  filterSideCards(): void;
  sideListState(cards: number, unread: number, shown: number): string;
  refilterSideCards(): void;
  cardRunRows(head: HTMLElement): HTMLElement[];
  foldCardRuns(list: HTMLElement): void;
  columnLinksShown(): boolean;
  scheduleColumnLinks(): void;
  drawColumnLinks(): void;
  noteLinkedSelection(): void;
  glideBehavior(): ScrollBehavior;
}

/** Make the card runs, in the page's construction order. */
export function createCardRuns(deps: CardRunsDependencies): CardRuns {
  /**
   * The search text over the cards, as over the Threads list's (`#129 message-search`): a card the
   * text does not match goes while the search is open. Links leaves them: a card has no links of its
   * own, and its thread's are in its messages.
   *
   * `#432 read-mode-unify`. And under Hide read a card whose thread is read goes too, its unpacked rows
   * with it (`threadIsRead`: no unread reply held or reported, and its root read), as Unread only did
   * (`#259 thread-list-unread-filter`); under Fold read a card in a run is behind its line
   * (`foldCardRuns`). The search and the read mode are ANDed, and a list they empty says why, over
   * where the cards were (`sideListState`).
   *
   * The owner asked whether the search should reach a thread's replies too. What is cheap is done: the
   * replies the page already holds for each thread are searched with the card's own words — each term
   * in one or the other — and a card found only with its replies says so, "in replies", beside its
   * facts (`markReplyMatch`). Nothing is read as the search is typed: a reply the page does not hold is
   * found once the cards' own walk further back has read it (`searchReplies`, `#262 search-load-older`).
   */
  function filterSideCards(): void {
    // A filter choosing rows opens every run, and closing it folds them again: unpacked blocks' runs, and
    // under Fold read the runs of read threads over the cards (`foldCardRuns`, `#432 read-mode-unify`).
    // Noted whether or not the blocks are drawn again (`cardsDrawn.noteSuspended`).
    if (deps.noteSuspendedRuns() && (deps.unpackOnScreen() || deps.readMode() === "fold")) {
      deps.forgetCards();
      deps.renderSideCards();
      return;
    }
    // `#606 server-side-search`. The shared language, message by message: a thread with any hit is
    // chosen, by its card's own words or by a reply the page holds.
    const node = deps.searchOpen() ? deps.searchNode() : null;
    const typed = node !== null;
    const marking = deps.searchOpen() ? deps.searchTerms() : [];
    const allRows = [...deps.leftById("side-cards").children] as HTMLElement[];
    const rows = allRows.filter((row) => row.hasAttribute("data-card"));
    const replyHits = typed ? deps.threadsWithHits(node) : null;
    // `#439 search-unmatched-separators`. The runs among the cards the reader has opened: shown, as in
    // the channel's lists, but never counted among the matches the bar names.
    const openedCards = deps.openedCardGaps(false);
    let unread = 0;
    let shown = 0;
    const cardShown = new Map<string, boolean>();
    for (const row of rows) {
      const text = row.getAttribute("data-search") || "";
      const fresh = !row.hasAttribute("data-thread-read");
      const revealed = openedCards !== null && openedCards.threads.has(String(row.getAttribute("data-id")));
      // `#432 read-mode-unify`. Hide read takes a read thread out with its unpacked rows, as it takes a read
      // message out of Main; a card in a folded run is hidden behind the run's line. A card in a run
      // the reader opened from its unmatched line (`#439`) stands through either, while the search stands.
      const kept = (((!deps.hidingRead() || fresh) && row.getAttribute("data-read-run") !== "member") || revealed);
      const own = !typed || deps.rowMatches(row, node, text);
      const inReplies = !own && replyHits !== null && replyHits.has(String(row.getAttribute("data-id")));
      const said = own || inReplies || revealed;
      if (row.hidden === kept) row.hidden = !kept;
      setClassWord(row, "search-hidden", !said);
      deps.markReplyMatch(row, inReplies);
      // `#249 search-highlight`. And where, once the typing pauses (`searchField.marked()`): the card's own
      // words, whatever its replies matched.
      deps.markSearchHits(row, own || inReplies ? marking : []);
      if (fresh) unread += 1;
      if (kept && (own || inReplies)) shown += 1;
      cardShown.set(String(row.getAttribute("data-id")), kept && said);
    }
    // `#419 threads-view-main-messages`. Main's messages among the cards: the read mode chose them as they
    // were drawn (`mainRowMessages`), a folded run hides its members behind its line, and the search finds
    // them by their words.
    for (const row of allRows) {
      if (!row.hasAttribute("data-main-row")) continue;
      const revealed = openedCards !== null && deps.inOpenedGap(row, openedCards.ids);
      const kept = row.getAttribute("data-read-run") !== "member" || revealed;
      const matched = !typed || deps.rowMatches(row, node, row.getAttribute("data-search") || "");
      const said = matched || revealed;
      if (row.hidden === kept) row.hidden = !kept;
      setClassWord(row, "search-hidden", !said);
      deps.markSearchHits(row, matched ? marking : []);
    }
    // `#404 threads-unpack-all`. A card's unpacked block goes where its card goes, a row hidden in a
    // collapsed run of read messages staying hidden behind its line (`#407 unpack-row-parity`).
    for (const row of allRows) {
      const thread = row.getAttribute("data-unpack-thread");
      if (thread === null) continue;
      const shownBlock = cardShown.get(thread) === true && row.getAttribute("data-read-run") !== "member";
      // ...and a block whose card heads a folded run of read threads goes behind the run's line with it.
      const folded = row.getAttribute("data-card-run") === "member";
      if (row.hidden === (shownBlock && !folded)) row.hidden = !(shownBlock && !folded);
      // `#606 server-side-search`. Unpacked under a search, a thread shows only its hits, its other
      // messages folded into lines between them (`drawUnpackGaps`), until the reader opens one.
      const hit = !typed || deps.rowMatches(row, node, row.getAttribute("data-search")) ||
        (openedCards !== null && deps.inOpenedGap(row, openedCards.ids));
      setClassWord(row, "search-hidden", !hit);
      deps.markSearchHits(row, typed && hit ? marking : []);
    }
    // `#281 view-keys`. A failed read of older threads is said over the cards it left as they were.
    const state = [deps.olderThreadsFailure(), sideListState(rows.length, unread, shown)].filter((part) => part !== "").join(" ");
    setTextIfChanged(deps.leftById("side-list-state"), state);
    if (deps.leftById("side-list-state").hidden !== (state === "")) deps.leftById("side-list-state").hidden = state === "";
    deps.setCardsCount(rows.length, shown);
    if (deps.threadListOnScreen()) deps.sayCardCount();
    // `#262 search-load-older`. The way to their threads' older replies, as the search and All's reach say.
    deps.renderRepliesSearch();
    // `#439 search-unmatched-separators`. A line in each run the search leaves among the cards.
    deps.drawCardGaps(deps.leftById("side-cards"));
    // `#261 two-column-connectors`. The cards shown are what the links go to.
    scheduleColumnLinks();
  }

  /**
   * What the right column says over its cards, or "" while it shows some: none in the channel, none
   * unread under Hide read (`#432 read-mode-unify`; Unread only, `#259`, before it), or none the search
   * matches.
   */
  function sideListState(cards: number, unread: number, shown: number): string {
    if (cards === 0) return "No threads in this channel yet.";
    if (shown > 0) return "";
    if (deps.hidingRead() && unread === 0) return "No unread threads.";
    // `#262 search-load-older`. How far back the search has looked, where that is the whole channel: until
    // then the way further back under this line says the day (`renderRepliesSearch`).
    const whole = deps.allLoaded() ? " — all of this channel is loaded" : "";
    return `No threads match that search${whole}.`;
  }

  // `#432 read-mode-unify`. Unread only (`#259 thread-list-unread-filter`) was the cards' own filter, a
  // chip and a choice kept on this device. It is the read mode now, as for every list: Hide read leaves the
  // read threads out, Fold read folds them into runs (`foldCardRuns`), Show read shows them.

  /** `filterSideCards`, the reader's place in the cards kept: the search typed, opened or closed. */
  function refilterSideCards(): void {
    deps.keepingCardPlace(() => filterSideCards());
  }

  /** The key the cards' opened runs are kept under, by channel (`readRuns`). */
  const cardRunKey = (): string => `cards:${String(deps.selectedChannelId())}`;

  // A card's DOM helpers (cardRootOf, threadCardId, cardGapGroups, markCardLayout, fillCardText, ...) are
  // web/src/view/channel/'s, and the Main runs among the cards (mainRunsAmong) web/src/present/rows/'s
  // (`#624 voice-page-modularity`).

  /** Whether an item of the cards is read: a card by its thread (`threadIsRead`), a Main row by its messages. */
  const cardRunRead = (row: HTMLElement): boolean => (row.hasAttribute("data-card") ? row.hasAttribute("data-thread-read") : deps.rowIsRead(row));

  /** The items of the run whose line is on `head`, the line's item first: cards and Main rows, not unpacked rows. */
  function cardRunRows(head: HTMLElement): HTMLElement[] {
    const rows = [...(head.parentNode || deps.leftById("side-cards")).children] as HTMLElement[];
    const run = [head];
    for (let at = rows.indexOf(head) + 1; at < rows.length; at += 1) {
      const row = rows[at] as HTMLElement;
      if (row.getAttribute("data-card-run") === "member") continue;
      if (row.getAttribute("data-read-run") !== "member" || row.hasAttribute("data-unpack-thread")) break;
      run.push(row);
    }
    return run;
  }

  /**
   * Fold the runs of read items over the cards in `list` under Fold read, and undo them otherwise: each run's
   * line on its first item, its other items hidden, and the rows unpacked under its cards marked as in the run
   * (`data-card-run`), over the folds of their own block, which come back when the run is opened.
   */
  function foldCardRuns(list: HTMLElement): void {
    const rows = [...list.children] as HTMLElement[];
    const items = rows.filter((row) => row.hasAttribute("data-card") || row.hasAttribute("data-main-row"));
    const placed = deps.readMode() === "fold" && !deps.readRunsSuspended()
      ? deps.readRunPlan(items, cardRunKey(), cardRunRead, cardRunIds) : new Map<HTMLElement, CardRunPlace>();
    const isItem = new Set(items);
    let run: { row: HTMLElement; threads: number; messages: number } | null = null;
    const say = (): void => {
      if (!run) return;
      const line = deps.readRunLine(run.row);
      const words = cardRunText(run.threads, run.messages);
      if (line.textContent !== words) line.textContent = words;
      line.setAttribute("title", `Show ${words.replace(/^… /, "these ")}`);
      deps.placeReadRunPeel(run.row, run.threads + run.messages);
    };
    for (const row of rows) {
      if (isItem.has(row)) {
        const role = (placed.get(row) || { role: null }).role;
        if (role !== "member") {
          say();
          run = role === "head" ? { row, threads: 0, messages: 0 } : null;
        }
        deps.placeInReadRun(row, role, 0);
        if (run) {
          if (row.hasAttribute("data-card")) run.threads += 1;
          else run.messages += idsOf(row).length;
        }
        continue;
      }
      if (run) row.setAttribute("data-card-run", "member");
      else if (row.hasAttribute("data-card-run")) row.removeAttribute("data-card-run");
    }
    say();
  }

  // The connectors between Main's roots and their cards on the right are `createColumnLinks`'
  // (`#624 voice-page-modularity`), with what is drawn and what it costs. The page tells it which
  // columns show, the selection and the highlighted card; the names below are the page's commands
  // to it. Nothing here scrolls for the mouse (`#1144`).
  const columnLinks = createColumnLinks({
    lookup: deps.leftById,
    ...deps.makeColumnLinksDeps(),
  });

  /** Whether the links have a place: two columns, the channel on the main screen, and cards or a thread's card. */
  function columnLinksShown(): boolean {
    return columnLinks.shown();
  }

  /** Ask for the links to be drawn again at the next animation frame. */
  function scheduleColumnLinks(): void {
    columnLinks.schedule();
  }

  /** Draw the links as the page stands now (`flyThreadCard`'s frames, and the frame asked for). */
  function drawColumnLinks(): void {
    columnLinks.draw();
  }

  /** The selection was drawn again (`drawSelection`): the links follow it. */
  function noteLinkedSelection(): void {
    columnLinks.noteSelection();
  }

  /**
   * How a deliberate glide moves (a jump the reader asked for, never the links'): "smooth", or
   * "auto" where the stylesheet says so (`--column-glide`).
   */
  function glideBehavior(): ScrollBehavior {
    return columnLinks.glideBehavior();
  }

  return {
    columnLinks,
    cardRunKey,
    filterSideCards,
    sideListState,
    refilterSideCards,
    cardRunRows,
    foldCardRuns,
    columnLinksShown,
    scheduleColumnLinks,
    drawColumnLinks,
    noteLinkedSelection,
    glideBehavior,
  };
}

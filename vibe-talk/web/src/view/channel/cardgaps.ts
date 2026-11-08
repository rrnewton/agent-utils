// The gaps a search opens in the cards and in an unpacked thread's block (`#624
// voice-page-modularity`, moved from the legacy script into view/channel; mu-readcore):
// which rows are hidden between shown ones, the line that says so, and opening it. The
// seam lines themselves are seams.ts's (`createSeamLines`); this owner only decides where
// a gap line goes and what opening it draws.

import { cardGapGroups, cardGapTally } from "./cards.ts";

/** A card or row with its seam lines held on it (seams.ts). */
type GapRow = HTMLElement & { seamBelowLine?: { gapUnpack?: boolean } | null; seamLine?: unknown };

/** What drawing card gaps asks of the page. */
export interface CardGapsDependencies {
  leftEl(id: string): HTMLElement;
  selectedChannelId(): string;
  searchKey(): string;
  openedCards(key: string, create: boolean): { ids: Set<string>; threads: Set<string> } | null;
  rowShown(row: Element): boolean;
  placeSeam(row: Element, seam: unknown, below: boolean): void;
  gapWords(count: number): string;
  cardGapWords(messages: number, threads: number, exact: boolean, onlyCards: boolean): string;
  keepingCardPlace(change: () => void): void;
  filterSideCards(): void;
  applySearch(): void;
  searchField: { open(): boolean; query(): string };
  searchTyped(query: string): boolean;
  cardThread(row: Element): string | null;
  idsOf(row: Element | null | undefined): string[];
}

/** The card gaps' commands: the legacy names, unchanged. */
export interface CardGaps {
  openedCardGaps(create: boolean): { ids: Set<string>; threads: Set<string> } | null;
  unpackBlocks(list: HTMLElement): { card: HTMLElement | null; rows: HTMLElement[] }[];
  drawUnpackGaps(list: HTMLElement, searching: boolean): void;
  openUnpackGap(row: HTMLElement, below: boolean): HTMLElement | null;
  drawCardGaps(list: HTMLElement): void;
  openCardGap(row: HTMLElement, below: boolean): HTMLElement | null;
  markReplyMatch(row: HTMLElement & { cardFacts?: HTMLElement; replyHint?: HTMLElement | null }, on: boolean): void;
  dispose(): void;
}

/** The card gaps over `deps`. */
export function createCardGaps(deps: CardGapsDependencies): CardGaps {
  const leftEl = deps.leftEl;
  const selectedChannelId = deps.selectedChannelId;
  const searchKey = deps.searchKey;
  const rowShown = deps.rowShown;
  const placeSeam = deps.placeSeam;
  const gapWords = deps.gapWords;
  const cardGapWords = deps.cardGapWords;
  const keepingCardPlace = deps.keepingCardPlace;
  const filterSideCards = deps.filterSideCards;
  const applySearch = deps.applySearch;
  const searchField = deps.searchField;
  const searchTyped = deps.searchTyped;
  const cardThread = deps.cardThread;
  const idsOf = deps.idsOf;
  const openedGaps = { cards: (key: string, create: boolean) => deps.openedCards(key, create) };
  let disposed = false;
  void disposed;

function openedCardGaps(create: boolean): { ids: Set<string>; threads: Set<string> } | null {
  // Forgotten the moment any filter changes, as among messages (`openedGapIds`).
  return openedGaps.cards(JSON.stringify([searchKey(), String(selectedChannelId())]), create);
}

/**
 * The blocks of messages unpacked under the cards of `list`, each as its card and its rows in order.
 * `#606 server-side-search`.
 *
 * @param {HTMLElement} list
 */

function unpackBlocks(list: HTMLElement): { card: HTMLElement | null; rows: HTMLElement[] }[] {
  /** @type {Map<string, {card: any, rows: any[]}>} */
  const blocks = new Map();
  let card = null;
  for (const row of /** @type {any[]} */ ([...list.children])) {
    const thread = row.getAttribute("data-unpack-thread");
    if (thread === null) {
      card = row.hasAttribute("data-card") ? row : null;
      continue;
    }
    if (!blocks.has(thread)) blocks.set(thread, { card: card && String(cardThread(card)) === thread ? card : null, rows: [] });
    /** @type {{card: any, rows: any[]}} */ (blocks.get(thread)).rows.push(row);
  }
  return [...blocks.values()];
}

/**
 * `#606 server-side-search`. In each thread unpacked under its card while a search is typed, a line
 * for each run of its messages the search leaves out: "… 3 unmatched messages …" above the hit after
 * the run, or below the last hit for a run at the end, or below the card for a run before the first
 * hit — above that hit instead when the cards' own line for the threads after it holds the card's
 * place below (`drawCardGaps` draws first), so neither line takes the other's. A block the read mode
 * or the search hides is skipped; every line comes down with the search. A tap opens its run
 * (`openUnpackGap`), as among the channel's messages.
 *
 * @param {HTMLElement} list
 * @param {boolean} searching
 */

function drawUnpackGaps(list: HTMLElement, searching: boolean): void {
  for (const { card, rows } of unpackBlocks(list)) {
    /** @type {Map<any, {above: any, below: any}>} */
    const wanted = new Map();
    // The cards' own line for the threads after this one, on the card's place below.
    const cardBusy = Boolean(card && card.hasAttribute("data-seam-below") && (card as GapRow).seamBelowLine &&
      !(card as GapRow).seamBelowLine?.gapUnpack);
    if (searching) {
      let run = 0;
      let last = null;
      const line = (count: number) => ({ kind: "gap", unpack: true, words: gapWords(count) });
      for (const row of rows) {
        if (row.hidden) continue;
        if (!rowShown(row)) {
          run += Math.max(1, idsOf(row).length);
          continue;
        }
        const onCard = run > 0 && last === null && card && !cardBusy;
        wanted.set(row, { above: run > 0 && (last !== null || !onCard) ? line(run) : null, below: null });
        if (onCard) wanted.set(card, { above: null, below: line(run) });
        run = 0;
        last = row;
      }
      if (run > 0) {
        if (last !== null) /** @type {{above: any, below: any}} */ (wanted.get(last)).below = line(run);
        else if (card && rowShown(card) && !cardBusy) wanted.set(card, { above: null, below: line(run) });
      }
    }
    for (const row of rows) {
      const want = wanted.get(row) || { above: null, below: null };
      if (!row.hidden || row.hasAttribute("data-seam") || row.hasAttribute("data-seam-below")) {
        placeSeam(row, want.above, false);
        placeSeam(row, want.below, true);
      }
    }
    // The card's line below it is the block's alone while the block wants one; the cards' own runs
    // put theirs above the next card shown, so the two never meet on one card.
    if (card) {
      const own = wanted.get(card);
      const held = (card as GapRow).seamBelowLine;
      if (own) placeSeam(card, own.below, true);
      else if (held && held.gapUnpack) placeSeam(card, null, true);
    }
  }
}

/**
 * Open the run of an unpacked thread's messages a line on `row` stands for: above it, the messages the
 * search hid before it; below it (or below the card, `row` being the card), those after it. Returns the
 * first row shown, for a keyboard's focus. `#606 server-side-search`.
 */

function openUnpackGap(row: HTMLElement, below: boolean): HTMLElement | null {
  const list = row.parentNode;
  if (!list) return null;
  const thread = row.getAttribute("data-unpack-thread") ?? String(cardThread(row));
  const block = unpackBlocks(list as HTMLElement).find((one) => one.rows.length > 0 &&
    String((one.rows[0] as HTMLElement).getAttribute("data-unpack-thread")) === String(thread));
  const opened = openedCardGaps(true);
  if (!block || !opened) return null;
  const rows = block.rows.filter((one) => !one.hidden);
  const at = row.hasAttribute("data-unpack-thread") ? rows.indexOf(row) : -1;
  let first: HTMLElement | null = null;
  const take = (one: HTMLElement) => {
    for (const id of idsOf(one)) opened.ids.add(id);
    if (first === null || !below) first = one;
  };
  if (below) {
    for (let next = at + 1; next < rows.length && !rowShown(rows[next] as HTMLElement); next += 1) take(rows[next] as HTMLElement);
  } else {
    for (let next = at - 1; next >= 0 && !rowShown(rows[next] as HTMLElement); next -= 1) take(rows[next] as HTMLElement);
  }
  if (first === null) return null;
  keepingCardPlace(filterSideCards);
  return first;
}

/**
 * Draw a line in each run the search leaves among `list`'s cards, and take away every one no
 * longer wanted: `drawSeams` for a list of threads. Each card keeps the words it was drawn with
 * (`placeSeam`), so a pass that changes nothing writes nothing, as among messages.
 */

function drawCardGaps(list: HTMLElement): void {
  if (!list) return;
  const searching = searchField.open() && searchTyped(searchField.query());
  if (!searching) {
    // No grouping to do: only lines a closed search left behind come down, and on most passes
    // there are none — an attribute each, asked on the way past.
    for (const row of list.children) {
      if (row.hasAttribute("data-seam")) placeSeam(row, null, false);
      if (row.hasAttribute("data-seam-below")) placeSeam(row, null, true);
    }
    return;
  }
  const groups = cardGapGroups(list as HTMLElement);
  /** @type {Map<any, {above: any, below: any}>} */
  const wanted = new Map();
  let run: { messages: number; threads: number; exact: boolean; onlyCards: boolean } | null = null;
  let last = null;
  const line = (tally: { messages: number; threads: number; exact: boolean; onlyCards: boolean }) => ({ kind: "gap", cards: true, words: cardGapWords(tally.messages, tally.threads, tally.exact, tally.onlyCards) });
  for (const group of groups) {
    if (group.other) {
      run = null;
      continue;
    }
    if (!rowShown(group.head)) {
      const one = cardGapTally(group);
      if (run === null) run = one;
      else run = {
        messages: run.messages + one.messages, threads: run.threads + one.threads,
        exact: run.exact && one.exact, onlyCards: run.onlyCards && one.onlyCards,
      };
      continue;
    }
    wanted.set(group.head, { above: searching && run !== null ? line(run) : null, below: null });
    run = null;
    last = group.head;
  }
  if (last !== null && searching && run !== null) wanted.get(last).below = line(run);
  const none = { above: null, below: null };
  for (const group of groups) {
    const want = wanted.get(group.head) || none;
    placeSeam(group.head, want.above, false);
    placeSeam(group.head, want.below, true);
  }
  // `#606 server-side-search`. Then the lines inside the threads unpacked under the cards: a card's
  // line below it is the block's while the block wants one.
  drawUnpackGaps(list, searching);
}

/**
 * Open the run a card gap's line on `row` stands for, as `openSearchGap` opens a run of messages:
 * its threads remembered, the cards filtered again where they stand. Returns its first card.
 */

function openCardGap(row: HTMLElement, below: boolean): HTMLElement | null {
  const list = row.parentNode;
  if (!list) return null;
  const groups = cardGapGroups(list as HTMLElement);
  const at = groups.findIndex((group) => group.head === row);
  if (at < 0) return null;
  const opened = openedCardGaps(true);
  if (!opened) return null;
  let first: HTMLElement | null = null;
  const take = (group: { card?: boolean; head: HTMLElement }) => {
    if (group.card) opened.threads.add(cardThread(group.head) as string);
    else for (const id of idsOf(group.head)) opened.ids.add(id);
    if (first === null || !below) first = group.head;
  };
  if (below) {
    for (let next = at + 1; next < groups.length && !groups[next]?.other && !rowShown((groups[next] as { head: HTMLElement }).head); next += 1) take(groups[next] as { card?: boolean; head: HTMLElement });
  } else {
    for (let next = at - 1; next >= 0 && !groups[next]?.other && !rowShown((groups[next] as { head: HTMLElement }).head); next -= 1) take(groups[next] as { card?: boolean; head: HTMLElement });
  }
  if (first === null) return null;
  if (list === leftEl("side-cards")) keepingCardPlace(filterSideCards);
  else applySearch();
  return first;
}

/**
 * Say on card `row`, or stop saying, that the search found it only with its thread's replies: "in
 * replies", at the end of its facts, made when it is wanted and gone when it is not.
 *
 * @param {HTMLElement & {cardFacts?: HTMLElement, replyHint?: HTMLElement | null}} row
 * @param {boolean} on
 */

function markReplyMatch(row: HTMLElement & { cardFacts?: HTMLElement; replyHint?: HTMLElement | null }, on: boolean): void {
  if (Boolean(row.replyHint) === on || !row.cardFacts) return;
  if (on) {
    const hint = document.createElement("span");
    hint.className = "side-card-hint";
    hint.textContent = "in replies";
    row.cardFacts.append(hint);
    row.replyHint = hint;
    row.setAttribute("data-search-in", "replies");
  } else if (row.replyHint) {
    row.cardFacts.removeChild(row.replyHint);
    row.replyHint = null;
    row.removeAttribute("data-search-in");
  }
}

/**
 * What the right column says over its cards, or "" while it shows some: none in the channel, none
 * unread under Hide read (`#432 read-mode-unify`; Unread only, `#259`, before it), or none the search
 * matches.
 *
 * @param {number} cards How many cards there are.
 * @param {number} unread How many of them have unread replies.
 * @param {number} shown How many the filters leave.
 */

  return {
    openedCardGaps, unpackBlocks, drawUnpackGaps, openUnpackGap, drawCardGaps, openCardGap,
    markReplyMatch,
    dispose() { disposed = true; },
  };
}

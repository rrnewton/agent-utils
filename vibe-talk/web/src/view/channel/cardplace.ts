// The reader's place in the cards, kept while they change (`#261 two-column-connectors`, `#280
// scroll-stability`, `#612 view-stability`; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel): where the reader is in the cards, the rows on screen and their cards' neighbours, and
// putting the reader back on them once the cards are drawn again, held by what kept most of the screen
// still when the card held has moved. Opening the cards, and remembering a place per channel, are the
// page's; why a moved card is not followed unless most of the screen moved with it is told where the
// legacy script opens the section.

import { rowShown } from "./rows.ts";

/** A card's neighbours in the list, by id: the cards before and after it, null at the ends. */
export interface Neighbours {
  readonly prev: string | null;
  readonly next: string | null;
}

/**
 * A place in the cards, as `anchor` reads it: the foot, or a card and how far below the top of the list's
 * window it stood, or with `id` null the scroll offset; and since `#612 view-stability` the rows on screen with
 * their cards' neighbours, which `place` holds the reader by when that card has moved.
 */
export interface CardPlace {
  readonly newest: boolean;
  readonly id: string | null;
  readonly offset: number;
  readonly rows?: readonly { id: string; card: string | null; own: boolean; offset: number; px: number }[];
  readonly cards?: ReadonlyMap<string, Neighbours | undefined>;
}

/**
 * Each of `rows`, by its id (`idOf`), with the ids of the rows before and after it, null at the ends: the cards
 * of a list, for whether one has moved since (`neighbourStayed`).
 */
function rowNeighbours(rows: readonly Element[], idOf = (row: Element): string => String(row.getAttribute("data-id"))): Map<string, Neighbours> {
  const id = (row: Element | undefined): string | null => (row ? idOf(row) : null);
  return new Map(rows.map((row, at) => [idOf(row), { prev: id(rows[at - 1]), next: id(rows[at + 1]) }]));
}

/**
 * Whether the row `id` still stands beside a row it stood beside: in `now`, the rows' neighbours as drawn, and
 * `was`, as they were when the place was read (`rowNeighbours`). A row in neither place has moved, re-sorted
 * to the foot by a reply, and a row not drawn has gone; one whose neighbours were not noted is taken to stay.
 */
function neighbourStayed(id: string, now: ReadonlyMap<string, Neighbours>, was: ReadonlyMap<string, Neighbours | undefined>): boolean {
  const is = now.get(id);
  const then = was.get(id);
  return is !== undefined && (!then || is.prev === then.prev || is.next === then.next);
}

/**
 * Whether two places in the cards, as `anchor` reads them, are the same: both the foot, or the same
 * card within a pixel of the same height. `#280 scroll-stability`.
 */
function sameCardPlace(one: CardPlace | null, other: CardPlace | null): boolean {
  if (!one || !other || one.newest !== other.newest) return false;
  return one.newest || (one.id === other.id && Math.abs(one.offset - other.offset) <= 1);
}

/** What keeping the place asks of the page. */
export interface CardPlacesDependencies {
  /** The element with this id in the left screen (`leftEl`). */
  lookup(id: string): HTMLElement;
  /**
   * Whether the cards are where a place can be read: the right column up, showing the cards, and not being
   * opened, which places the reader itself (`sideColumnUp`, `columns.side.mode`, `cardsOpening`).
   */
  readable(): boolean;
  /** Whether `area` is scrolled to its foot (`atBottom`). */
  atBottom(area: HTMLElement): boolean;
  /** Keep `area` at its foot (`followFoot`). */
  followFoot(area: HTMLElement): void;
  /** Move `area` by a measured delta, or put it at a measured position, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(area: HTMLElement, delta: number): void;
  toPosition(area: HTMLElement, top: number): void;
  /** The highlighted card's thread (`litCard`), or null. */
  litCard(): string | null;
  /** The selected card's id in the right column (`markOf(sidePane)`), or null. */
  selectedCard(): string | null;
  /** End a pack slide, so the place is read where the cards are laid out (`finishPackSlide`). */
  finishSlide(): void;
}

/** The place's commands. */
export interface CardPlaces {
  /** Where the reader is in the cards, or null with the cards not on screen, or being opened. */
  anchor(): CardPlace | null;
  /** Put the reader back on `anchor` in the cards; whether the reader was placed. */
  place(anchor: CardPlace | null, follow?: boolean): boolean;
  /** Run `change` to the cards, keeping the reader's place in them. */
  keeping(change: () => void, follow?: boolean): void;
}

/** Reading nothing until asked. */
export function createCardPlaces(deps: CardPlacesDependencies): CardPlaces {
  /**
   * Where the reader is in the cards, to put them back after the cards change under them: a thread that
   * moved going to the foot, the cards filtered, the line over them coming or going. `#261
   * two-column-connectors`. As a list of messages keeps its reader (`keepingFilterPlace`, whose rules
   * these are): the reader on the foot follows it; else the highlighted card while it shows (`#277
   * thread-card-focus`), else the selected one, else the first card shown whole (`#612 view-stability`), else
   * the first card showing, stays at its height. With every card off screen inside unpacked replies, keep the
   * scroll offset. Null with the cards not on screen, and while they are being opened, which places the reader
   * itself (`placeOpenedCards`).
   *
   * `#612 view-stability`: and `rows`, every row on screen with its offset and its card, fully shown first, with
   * `cards`, the neighbours of each of those cards, for when the card held is the one that moved.
   */
  function cardAnchor(): CardPlace | null {
    if (!deps.readable()) return null;
    const area = deps.lookup("side-scroll");
    if (deps.atBottom(area)) return { newest: true, id: null, offset: 0 };
    const top = area.getBoundingClientRect().top;
    const foot = top + (area.clientHeight || 0);
    const list = [...deps.lookup("side-cards").children] as HTMLElement[];
    const rows = list.filter((row) => row.hasAttribute("data-card") && rowShown(row));
    const seen = (row: HTMLElement): boolean => {
      const box = row.getBoundingClientRect();
      return box.bottom > top && box.top < foot;
    };
    // `#612 view-stability`. Shown whole: a card half off the top that grows — its summary come, its root edited —
    // grows over the cards under it, and they are what the reader is reading.
    const whole = (row: HTMLElement): boolean => {
      const box = row.getBoundingClientRect();
      return box.top >= top - 1 && box.bottom <= foot + 1;
    };
    const card = (id: string | null): HTMLElement | undefined => (id === null ? undefined : rows.find((one) => one.getAttribute("data-id") === id));
    const row = [card(deps.litCard()), card(deps.selectedCard())].find((one) => one && seen(one)) ||
      rows.find((one) => seen(one) && whole(one)) || rows.find(seen);
    // The rows on screen, in the list's order, read until the first below the foot: their boxes, and how much of
    // each shows.
    const around: { row: HTMLElement; main: boolean; owner: HTMLElement | null; offset: number; px: number; whole: boolean }[] = [];
    // The card each row is under: itself, or the card whose replies are unpacked after it.
    let underCard: HTMLElement | null = null;
    for (const one of list) {
      if (one.hasAttribute("data-card")) underCard = one;
      if (!one.hasAttribute("data-id") || !rowShown(one)) continue;
      const box = one.getBoundingClientRect();
      if (box.height > 0 && box.top >= foot) break;
      if (box.bottom <= top || box.top >= foot) continue;
      const main = one.hasAttribute("data-main-row");
      const owner = main ? null : underCard;
      if (!main && !owner) continue;
      around.push({ row: one, main, owner, offset: box.top - top, px: Math.min(box.bottom, foot) - Math.max(box.top, top),
        whole: box.top >= top - 1 && box.bottom <= foot + 1 });
    }
    const near = rowNeighbours(list.filter((one) => one.hasAttribute("data-card")));
    const cards = new Map<string, Neighbours | undefined>();
    const marks = [...around.filter((one) => one.whole), ...around.filter((one) => !one.whole)].map((one) => {
      const owner = one.owner ? String(one.owner.getAttribute("data-id")) : null;
      if (owner !== null) cards.set(owner, near.get(owner));
      return { id: String(one.row.getAttribute("data-id")), card: owner, own: one.row.hasAttribute("data-card"),
        offset: one.offset, px: one.px };
    });
    if (row) cards.set(String(row.getAttribute("data-id")), near.get(String(row.getAttribute("data-id"))));
    // Deep inside an unpacked thread, every card can be above the viewport. Keep the scroll
    // offset then: rebuilding those rows must still restore the reader after their nodes go.
    if (!row) return { newest: false, id: null, offset: area.scrollTop, rows: marks, cards };
    return { newest: false, id: String(row.getAttribute("data-id")), offset: row.getBoundingClientRect().top - top, rows: marks, cards };
  }

  /**
   * The row of `place` to hold the reader by now that the cards are drawn again (`#612 view-stability`): when a
   * card whose rows showed more of the screen than every row that stayed has moved, its first row, wherever it
   * now is; else, when the card held has moved or gone, the first row on screen that stayed — Main's messages,
   * or a row whose card still stands beside a card it stood beside — and where that row stood. Null when the
   * card held is the one to hold, or when no row is drawn.
   */
  function stayedCardRow(place: CardPlace): { row: HTMLElement; offset: number } | null {
    if (!place.rows || !place.cards) return null;
    const list = [...deps.lookup("side-cards").children] as HTMLElement[];
    const cards = list.filter((row) => row.hasAttribute("data-card"));
    const byId = new Map(cards.map((card) => [card.getAttribute("data-id"), card]));
    const now = rowNeighbours(cards);
    const was = place.cards;
    type Mark = NonNullable<CardPlace["rows"]>[number];
    const stayed = (mark: Mark): boolean => mark.card === null || neighbourStayed(mark.card, now, was);
    const found = (mark: Mark): HTMLElement | undefined => {
      if (mark.card === null) return list.find((one) => one.hasAttribute("data-main-row") && one.getAttribute("data-id") === mark.id);
      const owner = byId.get(mark.card);
      if (!owner || mark.own) return owner;
      for (let at = list.indexOf(owner) + 1; at < list.length && !(list[at] as HTMLElement).hasAttribute("data-card"); at += 1) {
        if ((list[at] as HTMLElement).getAttribute("data-id") === mark.id) return list[at];
      }
      return undefined;
    };
    const firstDrawn = (marks: readonly Mark[]): { row: HTMLElement; offset: number } | null => {
      for (const mark of marks) {
        const row = found(mark);
        if (row && rowShown(row)) return { row, offset: mark.offset };
      }
      return null;
    };
    // Most of the screen moved together: follow it, and nothing the reader sees moves.
    let still = 0;
    const moved = new Map<string | null, number>();
    for (const mark of place.rows) {
      if (stayed(mark)) still += mark.px;
      else moved.set(mark.card, (moved.get(mark.card) || 0) + mark.px);
    }
    const [heaviest, px] = [...moved].reduce<[string | null, number]>((best, entry) => (entry[1] > best[1] ? entry : best), [null, 0]);
    if (heaviest !== null && px > still) {
      const followed = firstDrawn(place.rows.filter((mark) => mark.card === heaviest));
      if (followed) return followed;
    }
    if (place.id !== null && neighbourStayed(place.id, now, was)) return null;
    return firstDrawn(place.rows.filter(stayed));
  }

  /**
   * Put the reader back on `anchor` in the cards: the foot, or that card at its height — or, where the
   * filters now hide it, the nearest card shown, in the list's order. Whether the reader was placed:
   * not for a card that has gone, nor where no card is shown.
   *
   * `#612 view-stability`: not after a card that has moved, re-sorted to the foot by a reply or gone, through a
   * change nobody asked for. The reader is held by what kept most of the screen still (`stayedCardRow`), and is
   * placed while one did. With `follow`, a place the reader chose to come back to, the card wherever it now is.
   */
  function placeCardAnchor(anchor: CardPlace | null, follow = false): boolean {
    if (!anchor) return false;
    const area = deps.lookup("side-scroll");
    if (anchor.newest) {
      deps.followFoot(area);
      return true;
    }
    const stayed = follow ? null : stayedCardRow(anchor);
    if (stayed) {
      deps.nudge(area, stayed.row.getBoundingClientRect().top - area.getBoundingClientRect().top - stayed.offset);
      return true;
    }
    if (anchor.id === null) {
      deps.toPosition(area, anchor.offset);
      return true;
    }
    const rows = ([...deps.lookup("side-cards").children] as HTMLElement[])
      .filter((row) => row.hasAttribute("data-card"));
    const at = rows.findIndex((row) => row.getAttribute("data-id") === anchor.id);
    if (at < 0) return false;
    let row = rowShown(rows[at] as HTMLElement) ? rows[at] as HTMLElement : null;
    for (let step = 1; !row && step < rows.length; step += 1) {
      row = [rows[at + step], rows[at - step]].find((one) => one && rowShown(one)) || null;
    }
    if (!row) return false;
    deps.nudge(area, row.getBoundingClientRect().top - area.getBoundingClientRect().top - anchor.offset);
    return true;
  }

  return {
    anchor: cardAnchor,
    place: placeCardAnchor,
    /**
     * Run `change` to the cards, keeping the reader's place in them. `#261 two-column-connectors`. With
     * `follow`, a change the reader asked for that re-sorts the cards: the card they are on is kept wherever it
     * goes (`#612 view-stability`).
     */
    keeping(change, follow = false) {
      // `#460 pack-gap-slide`. A slide is ended first, so the place is read where the cards are laid out.
      deps.finishSlide();
      const anchor = cardAnchor();
      change();
      placeCardAnchor(anchor, follow);
    },
  };
}

// What the page and the tests use of it.
export { neighbourStayed, rowNeighbours, sameCardPlace };

// What stands under the main screen's lists, as the dock-growth keepers last noted it (`#218 desktop-dock`,
// `#229 desktop-two-column`, `#280 scroll-stability` x `#281 view-keys`; `#624 voice-page-modularity`, moved
// from the legacy script into view/channel): for Main's list and for the right column, the list's height and
// the height under it (the dock, and the send card docked under the thread list), from which the next report
// reckons how far the dock grew. The measure is here (`heightUnderLists`); keeping a reader at the foot
// is the keepers' (their ResizeObservers).

/** A keeper's note: the list's height and the height under it, in px. */
export interface UnderListNote {
  readonly list: number;
  readonly dock: number;
}

/**
 * Main's note, and, from a send card that has come up docked under the thread list since, how far from the
 * foot of Main's list its reader was before it did.
 */
export interface MainUnderListNote extends UnderListNote {
  readonly gap?: number;
}

/** What measuring under the lists asks of the page. */
export interface UnderListsDependencies {
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  screens(): { screen(): string };
}

/** The notes' commands and questions. */
export interface UnderLists {
  /** What the keeper of Main's list noted at its last report. Null before the first. */
  main(): MainUnderListNote | null;
  noteMain(now: MainUnderListNote): void;
  /**
   * The send card moved into the dock under the thread list or out of it, changing the height under the
   * lists by `px`: no growth at all for Main's keeper, since the card left the foot of Main's list, or came
   * back to it, in the same move. Nothing before Main's first report.
   */
  cardMoved(px: number): void;
  /**
   * The send card came up docked under the thread list, Main's reader `gap` px from the foot of its list
   * just before: Main's keeper judges whether that reader was at the foot by `gap` at its next report.
   * Nothing before Main's first report.
   */
  cardCame(gap: number): void;
  /**
   * What the keeper of the right column noted at its last report, or as the cards were last put back where
   * the reader had them in the thread list. Null before the first.
   */
  side(): UnderListNote | null;
  noteSide(now: UnderListNote): void;
  /**
   * How much stands under the main screen's lists, for the dock-growth keepers: the dock, and the
   * send card while it is docked under the thread list (`#269 approval-card-inline` x `#281 view-keys`).
   * Off the main screen the card is docked too, but there the lists are not drawn and nothing is kept.
   */
  heightUnderLists(): number;
  /**
   * `#280 scroll-stability` x `#281 view-keys`. The send card moved into the dock under the thread list
   * or out of it, changing `heightUnderLists` by `px`: told to Main's keeper as no growth at all, since
   * the card left the foot of Main's list, or came back to it, in the same move (`placePostCard`). The
   * right column's keeper is not told: its cards never held the card, and a move into the dock is the
   * dock growing under them, except where the change of layout that moves it puts the cards back where
   * the reader had them in the thread list (`cardsPlacedUnderLists`).
   */
  cardMovedUnderLists(px: number): void;
  /**
   * `#280 scroll-stability` x `#281 view-keys`. The send card came up docked under the thread list, Main's
   * reader `gap` px from the foot of its list just before (`placePostCard`). Told to Main's keeper, which
   * judges whether that reader was at the foot by `gap` at its next report, rather than by how near the
   * foot they are left: the box at the list's foot shrank in the same layout as the dock grew.
   */
  cardCameUnderLists(gap: number): void;
  /**
   * `#280 scroll-stability` x `#269 approval-card-inline` x `#281 view-keys`. The cards were just put back
   * where the reader had them in the thread list, on the way back to it from two columns (`moveLayout`,
   * `layoutShown.holdListCards`), in their column as it now stands, with everything now under it: told to the right
   * column's keeper, which measures what grows under the cards from there. The send card that change moved
   * into the dock under the list, and the picker's new name if it wrapped the dock, are not the dock growing
   * under a reader at the foot of two columns' cards. Counted as growth, they took anyone within the slack
   * plus the card's height of the list's foot for a reader at it: in Chromium at a desk, the long draft's
   * card 246px tall, a reader put back 160px up the list's cards was pulled to its foot.
   */
  cardsPlacedUnderLists(): void;
}

/** No report yet. */
export function createUnderLists(deps: UnderListsDependencies): UnderLists {
  let mainUnderLists: MainUnderListNote | null = null;
  let sideUnderLists: UnderListNote | null = null;

  function cardMoved(px: number): void {
    if (mainUnderLists !== null) mainUnderLists = { ...mainUnderLists, dock: mainUnderLists.dock + px };
  }

  function cardCame(gap: number): void {
    if (mainUnderLists !== null) mainUnderLists = { ...mainUnderLists, gap };
  }

  function noteSide(now: UnderListNote): void {
    sideUnderLists = now;
  }

  function heightUnderLists(): number {
    const card = deps.leftEl("post-confirm");
    const docked = deps.screens().screen() === "main" && !card.hidden && card.getAttribute("data-place") === "dock";
    return (deps.el("dock").offsetHeight || 0) + (docked ? card.offsetHeight || 0 : 0);
  }

  function cardMovedUnderLists(px: number): void {
    cardMoved(px);
  }

  function cardCameUnderLists(gap: number): void {
    cardCame(gap);
  }

  function cardsPlacedUnderLists(): void {
    noteSide({ list: deps.leftEl("side-scroll").clientHeight, dock: heightUnderLists() });
  }

  return {
    main: () => mainUnderLists,
    noteMain(now) {
      mainUnderLists = now;
    },
    cardMoved,
    cardCame,
    side: () => sideUnderLists,
    noteSide,
    heightUnderLists,
    cardMovedUnderLists,
    cardCameUnderLists,
    cardsPlacedUnderLists,
  };
}

// What stands under the main screen's lists, as the dock-growth keepers last noted it (`#218 desktop-dock`,
// `#229 desktop-two-column`, `#280 scroll-stability` x `#281 view-keys`; `#624 voice-page-modularity`, moved
// from the legacy script into view/channel): for Main's list and for the right column, the list's height and
// the height under it (the dock, and the send card docked under the thread list), from which the next report
// reckons how far the dock grew. Measuring, and keeping a reader at the foot, are the page's (the keepers'
// ResizeObservers, `heightUnderLists`).

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
}

/** No report yet. */
export function createUnderLists(): UnderLists {
  let mainUnderLists: MainUnderListNote | null = null;
  let sideUnderLists: UnderListNote | null = null;

  return {
    main: () => mainUnderLists,
    noteMain(now) {
      mainUnderLists = now;
    },
    cardMoved(px) {
      if (mainUnderLists !== null) mainUnderLists = { ...mainUnderLists, dock: mainUnderLists.dock + px };
    },
    cardCame(gap) {
      if (mainUnderLists !== null) mainUnderLists = { ...mainUnderLists, gap };
    },
    side: () => sideUnderLists,
    noteSide(now) {
      sideUnderLists = now;
    },
  };
}

// When the page was last hidden, for how long the reader was away (`#203 incremental-refresh`; `#624
// voice-page-modularity`, moved from the legacy script into core/sync). What coming back does with it is the
// page's.

/** The page's time away: its commands. */
export interface AwayClock {
  /** The page is hidden at `now`; a page already hidden keeps the first time. */
  hide(now: number): void;
  /** The page is visible at `now`: how long it was hidden, or 0 if it was not; the clock starts over. */
  show(now: number): number;
}

/** Visible. */
export function createAwayClock(): AwayClock {
  let hiddenAt = 0;
  return {
    hide(now) {
      if (!hiddenAt) hiddenAt = now;
    },
    show(now) {
      const away = hiddenAt ? now - hiddenAt : 0;
      hiddenAt = 0;
      return away;
    },
  };
}

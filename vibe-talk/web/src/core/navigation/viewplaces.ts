// Where the reader was in each view of the one scrolling list (`#189 restore-ui-state`; `#624
// voice-page-modularity`, moved from the legacy script into core/navigation): the offset they left a view at,
// and whether that was at its newest line. Coming back puts them at the offset, but a reader who left at the
// newest goes to the newest, and so does a first entry (`atNewest` is an instruction, not an offset: turns
// may have arrived since). Also whether the last entry was such a return, which the read that follows needs
// so as not to undo it. Measuring the list and scrolling it are the page's (`showView`).

/** Where a view was left: its offset, and whether that was its newest line. */
export interface ViewPlace {
  readonly top: number;
  readonly atNewest: boolean;
}

/** The views' places. */
export interface ViewPlaces<View extends string> {
  /** The reader leaves `view` at `place`. */
  leave(view: View, place: ViewPlace): void;
  /**
   * The reader enters `view`: the offset to put them back at, or null to take them to the newest (never
   * here before, or left at the newest). Whether it was a return is kept for `restored`.
   */
  enter(view: View): number | null;
  /** Whether the last entry put the reader back rather than taking them to the newest. */
  restored(): boolean;
}

/** No view entered yet. */
export function createViewPlaces<View extends string>(): ViewPlaces<View> {
  const places = new Map<View, ViewPlace>();
  let viewRestored = false;
  return {
    leave(view, place) {
      places.set(view, { top: place.top, atNewest: place.atNewest });
    },
    enter(view) {
      const held = places.get(view);
      const returning = held !== undefined && !held.atNewest;
      viewRestored = returning;
      return returning ? held!.top : null;
    },
    restored: () => viewRestored,
  };
}

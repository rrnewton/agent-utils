// The one numeric anchor invariant (`#2091 vibe-talk-shrink-anchor-policy`, owner
// requests 307 and 308): for every READER-caused height change, the chosen anchor's
// viewport Y moves at most half a pixel. The three shrink cases (307) and the
// #2076 grow rule are one policy, stated here once so the page, the tests and the
// live debug mode all judge by the same function:
//
//   * "above" — the item's top is on screen: the content above holds still and
//     the content below moves (grow: the touched row's top holds, #2076).
//   * "below" — the top is off-screen above and the bottom is on screen: the
//     content just below the item holds still and the content above moves.
//   * "seam" — the item spans both edges: the seam where the content before and
//     after meet (at the fold line) lands at the viewport's centre.
//
// The one documented exception: arrivals while the reader is idle at the bottom
// ("arrival-bottom") follow the newest line and are exempt — sticking to the
// bottom is the arrival path's, never the reader's own act's.

/** How far an anchor may move, in px, before the invariant is broken (owner 308). */
export const ANCHOR_TOLERANCE_PX = 0.5;

/** Which of the three shrink cases a box stands in, against the visible band. */
export type ShrinkCase = "above" | "below" | "seam";

/** Every anchor kind the invariant judges: the shrink cases, growth, and the exempt arrival. */
export type AnchorKind = ShrinkCase | "grow" | "arrival-bottom";

/** A box's edges in viewport coordinates, as `getBoundingClientRect` reads them. */
export interface AnchorBox {
  readonly top: number;
  readonly bottom: number;
}

/**
 * The case a shrinking (or growing) item stands in: its top on screen is
 * "above", its top off-screen above with its bottom on screen is "below", and
 * spanning both edges is "seam". An item wholly above the band reads "below":
 * the content below it is the anchor either way.
 */
export function shrinkCase(box: AnchorBox, head: number, foot: number): ShrinkCase {
  if (box.top >= head - 1) return "above";
  if (box.bottom <= foot + 1) return "below";
  return "seam";
}

/** One anchor measurement to judge: where the anchor stood, and where it stands now. */
export interface AnchorCheck {
  readonly kind: AnchorKind;
  /** The anchor's viewport Y before the change (the seam's, for "seam": judged against `centreY` instead). */
  readonly beforeY: number;
  /** The anchor's viewport Y after the change. */
  readonly afterY: number;
  /** The seam's viewport Y after the change ("seam" only). */
  readonly seamY?: number;
  /** The viewport's centre Y ("seam" only). */
  readonly centreY?: number;
}

/** How far the anchor moved, in px: the seam's distance from centre, else the anchor's own move. */
export function anchorDeltaPx(check: AnchorCheck): number {
  if (check.kind === "arrival-bottom") return 0;
  if (check.kind === "seam") {
    return Math.abs((check.seamY ?? check.afterY) - (check.centreY ?? check.beforeY));
  }
  return Math.abs(check.afterY - check.beforeY);
}

/**
 * The invariant itself (owner 308): the chosen anchor's viewport Y moves at
 * most `ANCHOR_TOLERANCE_PX` — the seam within that of centre — for every
 * reader-caused height change. Arrivals while idle at the bottom are exempt.
 * The tests and the live debug mode both call this function.
 */
export function anchorInvariantHolds(check: AnchorCheck): boolean {
  if (check.kind === "arrival-bottom") return true;
  return anchorDeltaPx(check) <= ANCHOR_TOLERANCE_PX;
}

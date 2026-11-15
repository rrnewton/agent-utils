// Follow-scroll for autoplay read-aloud (`#1587 autoplay-follow-scroll`): while a read
// plays, keep the highlighted sentence (or, with no sentence light, the row being read)
// in the middle third of the visible list. Pure geometry and pause state, so the rule
// is testable without a browser; `read-aloud.ts` supplies the live rects and performs
// the scroll. The port is `#scroll-area` as laid out: with Discuss docked the
// stylesheet ends it at the panel's top edge (`#1526`), so its rect already excludes
// the panel and no Discuss-specific math belongs here.

/** The comfortable band: the middle third of the port, as viewport-pixel edges. */
export function followBand(portTop: number, portBottom: number): { top: number; bottom: number } {
  const third = (portBottom - portTop) / 3;
  return { top: portTop + third, bottom: portBottom - third };
}

/** Whether `target` (viewport-pixel edges) sits inside `band`. */
export function inFollowBand(targetTop: number, targetBottom: number, band: { top: number; bottom: number }): boolean {
  return targetTop >= band.top && targetBottom <= band.bottom;
}

/**
 * The scroll delta that centres `target` in `band`, or 0 where it already sits in
 * the band: follow only when the highlight leaves the comfortable third, then bring
 * it to the middle rather than nudging it to the nearest edge.
 */
export function followDelta(targetTop: number, targetBottom: number, band: { top: number; bottom: number }): number {
  if (inFollowBand(targetTop, targetBottom, band)) return 0;
  return (targetTop + targetBottom) / 2 - (band.top + band.bottom) / 2;
}

/** Smooth unless the reader prefers reduced motion, where the follow jumps. */
export function followBehavior(reducedMotion: boolean): "smooth" | "auto" {
  return reducedMotion ? "auto" : "smooth";
}

/**
 * Whether following is paused for `readId`. Rule: a scroll the follow itself did not
 * cause (the reader scrolling, dragging or flicking) pauses following until the
 * highlight is back in the band or a different read starts; a programmatic follow
 * scroll never pauses it.
 */
export function createFollowPause(now: () => number = () => Date.now()) {
  let pausedFor: string | null = null;
  let ownScrollAt = Number.NEGATIVE_INFINITY;
  return {
    /** The follow is about to scroll: the scroll event it promptly causes is not the reader's. */
    noteOwnScroll(): void { ownScrollAt = now(); },
    /** A scroll event on the port: the reader's own scroll pauses following. A follow's
     *  own event is only recognised promptly — a stale flag must not swallow a later
     *  scroll of the reader's (the jump-chip walk scrolls away and waits). */
    noteScroll(readId: string): void {
      if (now() - ownScrollAt < 1000) { ownScrollAt = Number.NEGATIVE_INFINITY; return; }
      pausedFor = readId;
    },
    paused(readId: string, targetInBand: boolean): boolean {
      if (pausedFor === null) return false;
      if (pausedFor !== readId || targetInBand) { pausedFor = null; return false; }
      return true;
    },
    reset(): void { pausedFor = null; ownScrollAt = Number.NEGATIVE_INFINITY; },
  };
}

export type FollowPause = ReturnType<typeof createFollowPause>;

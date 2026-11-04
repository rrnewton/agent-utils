// Whether the reply open in the stream was in sight when the Draft chip was last drawn (`#846
// draft-chip-honest`; `#624 voice-page-modularity`, moved from the legacy script into view/shell): a scroll
// redraws the chip only when that changes. Every drawing of the chip keeps it, so it is never stale. Looking,
// and drawing the chip, are the page's.

/** The reply's last sight: its commands. */
export interface ReplySight {
  /** The chip was drawn with the reply in sight or not, as `inSight` says: kept. */
  keep(inSight: boolean): void;
  /** A frame's look: whether `now` differs from what was kept, keeping it when it does. */
  crossed(now: boolean): boolean;
}

/** Out of sight: no chip drawn yet. */
export function createReplySight(): ReplySight {
  let replyWasInSight = false;
  return {
    keep(inSight) {
      replyWasInSight = inSight;
    },
    crossed(now) {
      if (now === replyWasInSight) return false;
      replyWasInSight = now;
      return true;
    },
  };
}

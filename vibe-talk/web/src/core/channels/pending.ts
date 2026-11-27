// The replies an open thread's root reports and the page has not brought yet, as boxes that stand
// where they will land (`#2385 vibe-talk-thread-open-layout-first`). The first slice of the sparse
// incremental loading design, scoped to the thread view: a box set priced from the count the page
// already holds, before any body arrives. What to draw is decided here, with no DOM; the boxes
// themselves are view/channel's (`createPendingReplies`).
//
// The owner, 2026-11-27, of a thread that showed one reply for a long while and then the rest at
// once: "blank boxes would have appeared from the very first packets that are received instead of
// waiting in this ambiguous state for messages to then all the sudden appear later."

/**
 * At most this many boxes stand at once: the read that brings the bodies asks for the thread's
 * newest page (50), so a box past that would stand for a reply no read on its way brings.
 */
export const PENDING_REPLY_BOXES_MAX = 50;

/**
 * A box's height while no reply of the thread is drawn to measure: the phone fixture's median
 * row (`DEFAULT_ROW_PX`, heights.ts), in CSS px.
 */
export const PENDING_REPLY_PX_DEFAULT = 84;

/** What a box set is: how many boxes, each how tall (CSS px), and the line that says so. */
export interface PendingRepliesPlan {
  readonly count: number;
  readonly px: number;
  readonly line: string;
}

/** The words for `count` replies on their way: "8 more replies loading", "1 more reply loading". */
export function pendingRepliesLine(count: number): string {
  return `${count} more ${count === 1 ? "reply" : "replies"} loading`;
}

/**
 * A box's height from the heights of the thread's replies drawn on screen: their median, so one
 * long reply does not size every box; the default when none is drawn.
 */
export function pendingReplyPx(measured: readonly number[]): number {
  const usable = measured.filter((px) => Number.isFinite(px) && px > 0).sort((a, b) => a - b);
  if (!usable.length) return PENDING_REPLY_PX_DEFAULT;
  return Math.round(usable[Math.floor(usable.length / 2)] as number);
}

/**
 * The boxes to draw for an open thread missing `missing` replies its root reports, or null for
 * none. Only while a read that brings them is on its way (`readComing`): a box with no read behind
 * it would say "loading" of something nothing is loading. Never with the boxes switched off.
 */
export function planPendingReplies(missing: number, readComing: boolean, enabled: boolean,
  measured: readonly number[]): PendingRepliesPlan | null {
  if (!enabled || !readComing || !Number.isFinite(missing) || missing <= 0) return null;
  const count = Math.min(Math.floor(missing), PENDING_REPLY_BOXES_MAX);
  return { count, px: pendingReplyPx(measured), line: pendingRepliesLine(count) };
}

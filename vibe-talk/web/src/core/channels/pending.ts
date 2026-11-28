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

/**
 * What a box set is: how many boxes, each how tall (CSS px), and the line that says so. `heights`,
 * when the thread's layout priced the boxes one per reply (`planLayoutReplies`): each box's own
 * height, in order, `count` of them; without it every box is `px`.
 */
export interface PendingRepliesPlan {
  readonly count: number;
  readonly px: number;
  readonly line: string;
  readonly heights?: readonly number[];
}

/**
 * One reply as the thread's layout describes it, as far as pricing its box reads it: the server's
 * `ThreadLayoutEntry` (`GET .../threads/{id}/layout`, `#2385`), its body's size before the body.
 */
export interface LayoutReply {
  readonly id: string;
  /** The stored body's length in UTF-16 code units (what `String.length` says of it). */
  readonly length_utf16: number;
  /** The stored body's lines: its line breaks plus one. */
  readonly lines: number;
  readonly flags: { readonly attachments: boolean; readonly long: boolean };
}

/** Characters a reply's line holds, as the height estimator reads a body (heights.ts `heightFor`). */
export const LAYOUT_CHARS_PER_LINE = 40;

/** How much each line past the third adds to a box, as a share of the base: heights.ts' own step. */
export const LAYOUT_LINE_STEP = 0.18;

/**
 * The lines a long body's box is priced at: the page folds a body over `COLLAPSE_OVER_CHARS`
 * (view/channel/folding.ts; the server's `flags.long` says so) to its collapsed lines, 3 unless the
 * reader set otherwise (core/settings), with its elided line under them.
 */
export const LAYOUT_FOLDED_LINES = 4;

/**
 * One box's height (whole CSS px) for the reply its layout entry describes, from `basePx`, the
 * median reply row on screen (`pendingReplyPx`): the height estimator's rule (heights.ts
 * `heightFor`) on the entry's size. A body of up to three lines (by its length at about forty
 * characters a line, or its own line breaks, whichever says more) is the base; each line past that
 * adds a step; a long body is priced as the folded row the page draws for it. A shared file is a
 * line of the body already (`lines`), so `flags.attachments` adds nothing of its own here.
 */
export function layoutReplyPx(entry: LayoutReply, basePx: number): number {
  const base = Number.isFinite(basePx) && basePx > 0 ? basePx : PENDING_REPLY_PX_DEFAULT;
  const length = Math.max(0, Number(entry.length_utf16) || 0);
  const breaks = Math.max(1, Math.floor(Number(entry.lines) || 1));
  const wrapped = Math.max(1, Math.ceil(length / LAYOUT_CHARS_PER_LINE), breaks);
  const drawn = entry.flags && entry.flags.long ? Math.min(wrapped, LAYOUT_FOLDED_LINES) : wrapped;
  return Math.round(drawn <= 3 ? base : base * (1 + (drawn - 3) * LAYOUT_LINE_STEP));
}

/**
 * The boxes re-priced one per reply from the thread's layout (`#2385`, S-t1), or null where the
 * layout says nothing the count-priced boxes do not. The boxes stand below the rows the page holds,
 * so they are the replies the layout lists AFTER the last one the page holds (`held`), each priced
 * from its own entry (`layoutReplyPx`); a reply the layout lists above that lands above the rows,
 * where no box stands for it. When the count says more replies are missing (`missing`) than the
 * layout lists at all (the server's copy has not read them yet), the rest stand after them at the
 * base. At most `PENDING_REPLY_BOXES_MAX`, the newest kept: the read on its way brings the newest.
 */
export function planLayoutReplies(entries: readonly LayoutReply[], held: (id: string) => boolean, missing: number,
  basePx: number): PendingRepliesPlan | null {
  const base = Number.isFinite(basePx) && basePx > 0 ? Math.round(basePx) : PENDING_REPLY_PX_DEFAULT;
  let after = 0;
  let above = 0;
  entries.forEach((entry, index) => { if (held(String(entry.id))) after = index + 1; });
  entries.slice(0, after).forEach((entry) => { if (!held(String(entry.id))) above += 1; });
  const priced = entries.slice(after).filter((entry) => !held(String(entry.id))).map((entry) => layoutReplyPx(entry, base));
  if (!priced.length) return null;
  const unlisted = Math.max(0, Math.floor(Number.isFinite(missing) ? missing : 0) - above - priced.length);
  const heights = [...priced, ...Array.from({ length: unlisted }, () => base)].slice(-PENDING_REPLY_BOXES_MAX);
  return { count: heights.length, px: base, line: pendingRepliesLine(heights.length), heights };
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

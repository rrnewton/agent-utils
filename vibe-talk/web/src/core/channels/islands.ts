// Islands and gaps in one channel's held history (`#2426 vibe-talk-sparse-loading-program`, slice
// R2). Pure: no DOM, no timers, no network. The view asks what to let go of and where its gaps
// stand, and draws them; the reads that fill a gap are the page's.
//
// The owner, 2026-11-27: "If we scroll back and back and back and load tons of messages then
// eventually it would break off into a "separate" island... keeping a contiguous region around our
// farback scroll, and keeping a small window of most recent, but losing what's inbetween. Jumping to
// "newest" should still be fast and constant time."
//
// An ISLAND is a dense, gap-free stretch of a view's order that the page holds. A GAP is a stretch
// between two islands that it does not hold. The newest island always ends at the view's newest
// message the page knows, so going to the newest is a scroll to the foot: no read. A projection
// never invents coverage (views.ts): every gap is drawn, never closed up silently.

import type { Message } from "../../contract/index.ts";
import { timeOf } from "./canon.ts";
import type { ViewGap } from "./canon.ts";
import type { HeldRow } from "./residency.ts";

export type { ViewGap };


/** How much one view keeps, in messages (structural: core/settings declares the budget). */
export interface IslandBudget {
  /** The newest window: always held, so the newest is a scroll away. */
  readonly newestRows: number;
  /** The island the reader stands in, when it is not the newest window: this many around them. */
  readonly islandRows: number;
}

/**
 * The interim island budgets (owner, 2026-11-27: "even on desktop I think 5000 messages is too large a
 * target to load. The loading should be incremental and small"): the newest window and the far-back
 * island, each on its own, by form factor. The residency layer's budget (`#2427`) takes over the
 * newest window when it lands.
 */
export const ISLAND_NEWEST_PHONE = 150;
export const ISLAND_NEWEST_DESKTOP = 200;
export const ISLAND_ROWS_PHONE = 150;
export const ISLAND_ROWS_DESKTOP = 200;

/** The budget on a phone (`phone`) or a desk. */
export function islandBudget(phone: boolean): IslandBudget {
  return phone
    ? { newestRows: ISLAND_NEWEST_PHONE, islandRows: ISLAND_ROWS_PHONE }
    : { newestRows: ISLAND_NEWEST_DESKTOP, islandRows: ISLAND_ROWS_DESKTOP };
}

/** Whether held rows are in the order the plan needs: time, then id, as the store keeps them. */
function ordered(rows: readonly HeldRow[]): boolean {
  for (let at = 1; at < rows.length; at += 1) {
    const a = rows[at - 1] as HeldRow;
    const b = rows[at] as HeldRow;
    if (b.ms < a.ms) return false;
  }
  return true;
}

/**
 * A break-off: the stretch of held rows to let go of BETWEEN the reader's island and the newest
 * window, or null when nothing should go. Only when the reader stands outside the newest window
 * (`anchor` is the index of the row being read in `rows`, oldest first), and only when what is held
 * passes the two budgets together by `slack` rows (so a break-off is not taken back and forth at the
 * edge). The island keeps `islandRows` rows around the anchor (half above, half below, moved to fit
 * inside the held stretch); the newest window keeps the last `newestRows`. Every pinned id between
 * them stays, splitting the stretch: the answer is the runs to let go of, oldest first, each one a
 * gap once it goes. Rows below the reader's island and above the newest window only: never a row
 * older than the island (the floor's business, the residency layer's oldest-first discard).
 */
export function planBreakOff(
  rows: readonly HeldRow[],
  anchor: number | null,
  pins: ReadonlySet<string>,
  budget: IslandBudget,
  slack = 0,
): { first: number; last: number }[] | null {
  const total = rows.length;
  const newest = Math.max(0, Math.floor(budget.newestRows));
  const island = Math.max(1, Math.floor(budget.islandRows));
  if (anchor === null || !Number.isInteger(anchor) || anchor < 0 || anchor >= total) return null;
  if (!ordered(rows)) return null;
  const newestFrom = Math.max(0, total - newest);
  // The reader is in the newest window: nothing between to let go of.
  if (anchor >= newestFrom) return null;
  if (total <= newest + island + Math.max(0, slack)) return null;
  // The island: `island` rows around the anchor, kept inside the held stretch.
  const above = Math.floor((island - 1) / 2);
  let islandFrom = Math.max(0, anchor - above);
  let islandTo = Math.min(total - 1, islandFrom + island - 1);
  islandFrom = Math.max(0, islandTo - island + 1);
  if (islandTo >= newestFrom - 1) return null;
  // Between: (islandTo, newestFrom). Pins split it into runs.
  const runs: { first: number; last: number }[] = [];
  let start = -1;
  for (let at = islandTo + 1; at < newestFrom; at += 1) {
    const pinned = pins.has((rows[at] as HeldRow).id);
    if (!pinned && start < 0) start = at;
    if (pinned && start >= 0) {
      runs.push({ first: start, last: at - 1 });
      start = -1;
    }
  }
  if (start >= 0) runs.push({ first: start, last: newestFrom - 1 });
  return runs.length ? runs : null;
}

/**
 * The top trim of a far-back reader's island: the index of the first held row to keep, letting
 * every row older than it go, or null when none should go. Only when the reader stands outside the
 * newest window and what is held passes the two budgets together by `slack` rows (inside the newest
 * window the residency layer's oldest-first discard has it). The island keeps `islandRows` rows
 * around the anchor; the oldest pinned row older than the island stops the trim (it stays, and so
 * does everything below it, as the residency layer's discard does).
 */
export function planTopTrim(
  rows: readonly HeldRow[],
  anchor: number | null,
  pins: ReadonlySet<string>,
  budget: IslandBudget,
  slack = 0,
): number | null {
  const total = rows.length;
  const newest = Math.max(0, Math.floor(budget.newestRows));
  const island = Math.max(1, Math.floor(budget.islandRows));
  if (anchor === null || !Number.isInteger(anchor) || anchor < 0 || anchor >= total) return null;
  if (anchor >= Math.max(0, total - newest)) return null;
  if (total <= newest + island + Math.max(0, slack)) return null;
  const above = Math.floor((island - 1) / 2);
  let keepFrom = Math.max(0, anchor - above);
  for (let at = 0; at < keepFrom; at += 1) {
    if (pins.has((rows[at] as HeldRow).id)) { keepFrom = at; break; }
  }
  return keepFrom > 0 ? keepFrom : null;
}

/**
 * The gap a let-go run leaves in a view: its edges are the held rows just around it, `count` the
 * run's length and `heightPx` its measured height (null when not measured). Null when the run has
 * no held row on one side (it would not be BETWEEN two islands).
 */
export function gapForRun(rows: readonly HeldRow[], run: { first: number; last: number }, heightPx: number | null): ViewGap | null {
  const older = rows[run.first - 1];
  const newer = rows[run.last + 1];
  if (!older || !newer || run.last < run.first) return null;
  return {
    olderId: older.id,
    olderMs: older.ms,
    newerId: newer.id,
    newerMs: newer.ms,
    count: run.last - run.first + 1,
    heightPx: heightPx !== null && Number.isFinite(heightPx) && heightPx >= 0 ? Math.round(heightPx) : null,
  };
}

/**
 * The gaps of `gaps` that still stand above a view's floor `floorMs`: a gap whose newer edge is at or
 * below the floor lies wholly in what the page gave up from the top (residency's discard at the newest,
 * `#2427`), so the walk back from the floor's cursor reaches its stretch and no seam says it is missing.
 */
export function gapsAboveFloor(gaps: readonly ViewGap[] | undefined, floorMs: number | null): ViewGap[] {
  if (!gaps) return [];
  return floorMs === null ? [...gaps] : gaps.filter((gap) => gap.newerMs > floorMs);
}

/**
 * `gaps` with `added` among them, oldest first. A gap that shares an edge with `added` is one the new
 * stretch let go of runs into (a far-back reader moving further back lets go of the rows just above
 * the gap that stood): the two are one gap, from the older edge of one to the newer edge of the
 * other, their counts and heights summed (null when either is unknown).
 */
export function withGap(gaps: readonly ViewGap[] | undefined, added: ViewGap): ViewGap[] {
  let joined = added;
  const next: ViewGap[] = [];
  for (const gap of gaps || []) {
    if (gap.olderId === joined.olderId || gap.newerId === joined.newerId) {
      const older = gap.olderMs <= joined.olderMs ? gap : joined;
      const newer = gap.newerMs >= joined.newerMs ? gap : joined;
      joined = {
        olderId: older.olderId,
        olderMs: older.olderMs,
        newerId: newer.newerId,
        newerMs: newer.newerMs,
        count: gap.count === null || joined.count === null ? null : gap.count + joined.count,
        heightPx: gap.heightPx === null || joined.heightPx === null ? null : gap.heightPx + joined.heightPx,
      };
    } else {
      next.push(gap);
    }
  }
  next.push(joined);
  next.sort((a, b) => a.olderMs - b.olderMs || (a.olderId < b.olderId ? -1 : a.olderId > b.olderId ? 1 : 0));
  return next;
}

/**
 * Where each gap stands among a view's projected rows: by the id of the row drawn just above it.
 * A gap whose older edge is no longer among the rows (let go of in turn, or never drawn in this
 * view) stands above the first row newer than its older edge's time, so it is still drawn; a gap
 * with no row newer than it is not between two islands any more, and is left out.
 */
export function gapPlaces(rows: readonly Message[], gaps: readonly ViewGap[] | undefined): Map<string, ViewGap> {
  const places = new Map<string, ViewGap>();
  if (!gaps || !gaps.length || !rows.length) return places;
  const index = new Map<string, number>();
  rows.forEach((row, at) => index.set(String(row.id), at));
  for (const gap of gaps) {
    let at = index.has(gap.olderId) ? (index.get(gap.olderId) as number) : -1;
    if (at < 0) {
      // The last row at or before the older edge's time.
      for (let probe = rows.length - 1; probe >= 0; probe -= 1) {
        if (timeOf(rows[probe], "timestamp") <= gap.olderMs) { at = probe; break; }
      }
    }
    if (at < 0 || at >= rows.length - 1) continue;
    places.set(String((rows[at] as Message).id), gap);
  }
  return places;
}

/** Which way a fill grows a gap's edge: from its older side down, or from its newer side up. */
export type FillSide = "older" | "newer";

/**
 * A gap after a fill from `side` landed `arrived` (messages of the view, any order): the edge on
 * that side moves to the arrived top-level message nearest the other edge (replies ride with their
 * roots and are not counted), `count` drops by how many landed
 * inside the gap, and `heightPx` by their share. Null when the gap is closed: an arrived message is
 * the other edge (or past it), or the read said there is nothing more that way (`exhausted`).
 */
export function narrowGap(gap: ViewGap, side: FillSide, arrived: readonly Message[], exhausted: boolean): ViewGap | null {
  const inside: { id: string; ms: number }[] = [];
  let touched = false;
  for (const message of arrived) {
    // Replies ride with their roots (Slack's All carries a root's replies at their own, later times):
    // only top-level messages say how far a fill reached.
    const thread = message.thread as { is_root?: unknown } | null | undefined;
    if (thread && thread.is_root !== true) continue;
    const id = String(message.id);
    const ms = timeOf(message, "timestamp");
    if (!Number.isFinite(ms)) continue;
    if (side === "older" ? id === gap.newerId || ms > gap.newerMs : id === gap.olderId || ms < gap.olderMs) touched = true;
    else if (ms >= gap.olderMs && ms <= gap.newerMs && id !== gap.olderId && id !== gap.newerId) inside.push({ id, ms });
  }
  if (touched || exhausted) return null;
  if (!inside.length) return gap;
  inside.sort((a, b) => a.ms - b.ms || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const edge = side === "older" ? (inside[inside.length - 1] as { id: string; ms: number }) : (inside[0] as { id: string; ms: number });
  const count = gap.count === null ? null : Math.max(0, gap.count - inside.length);
  if (count === 0) return null;
  const heightPx = gap.heightPx === null || gap.count === null || gap.count <= 0
    ? null : Math.round(gap.heightPx * (count as number) / gap.count);
  return side === "older"
    ? { ...gap, olderId: edge.id, olderMs: edge.ms, count, heightPx }
    : { ...gap, newerId: edge.id, newerMs: edge.ms, count, heightPx };
}

/**
 * The threads an around answer left out of a refilled stretch of All (`around.replies_left_out`: the
 * server could not prove their replies whole, so it sent the roots alone), by store. Their roots'
 * chips say the replies are not loaded, until the store holds them (`repliesLeftOut`).
 */
const LEFT_OUT = new WeakMap<object, Set<string>>();

/** Note that `ids`' replies were left out of a stretch folded into `canon`. */
export function noteRepliesLeftOut(canon: object, ids: readonly unknown[]): void {
  if (!ids.length) return;
  let held = LEFT_OUT.get(canon);
  if (!held) {
    held = new Set<string>();
    LEFT_OUT.set(canon, held);
  }
  for (const id of ids) held.add(String(id));
}

/** Whether thread `id`'s replies were left out of a stretch folded into `canon`. */
export function repliesLeftOut(canon: object, id: unknown): boolean {
  const held = LEFT_OUT.get(canon);
  return held !== undefined && held.has(String(id));
}

/** The thread's replies came (its own read): its root's chip counts them as any other. */
export function forgetRepliesLeftOut(canon: object, id: unknown): void {
  const held = LEFT_OUT.get(canon);
  if (held) held.delete(String(id));
}

/**
 * Where a stretch an `around` read brought for a link lands among what a view holds (`#2426` R3,
 * links always work): `rows` the view's held rows and `stretch` the stretch's messages of the view,
 * both oldest first; `gaps` its gaps; `floorMs` its cover's floor (null when none).
 *
 * - Below everything held: a new island under the old floor, the floor moving down to the stretch's
 *   oldest, with an uncounted gap between it and the oldest row held (`lowersFloor`).
 * - Overlapping the oldest row held: the held stretch simply grows down (`lowersFloor`, no gap).
 * - Inside a gap: the gap narrows from the edge the stretch touches, or, touching neither, splits
 *   into two uncounted gaps around a new island.
 * - Among held rows: nothing changes.
 */
export function placeStretch(
  rows: readonly HeldRow[],
  gaps: readonly ViewGap[],
  stretch: readonly HeldRow[],
): { gaps: ViewGap[]; lowersFloor: boolean; floorMs: number | null } {
  const unchanged = { gaps: [...gaps], lowersFloor: false, floorMs: null };
  if (!stretch.length || !rows.length) return unchanged;
  const held = new Set(rows.map((row) => row.id));
  const oldest = stretch[0] as HeldRow;
  const newest = stretch[stretch.length - 1] as HeldRow;
  const first = rows[0] as HeldRow;
  // Under everything held, or reaching down past it from the oldest row held.
  if (newest.ms < first.ms || (held.has(first.id) && stretch.some((row) => row.id === first.id) && oldest.ms <= first.ms)) {
    const touching = stretch.some((row) => row.id === first.id);
    const added: ViewGap[] = touching ? [] : [{
      olderId: newest.id, olderMs: newest.ms, newerId: first.id, newerMs: first.ms, count: null, heightPx: null,
    }];
    return { gaps: [...gaps, ...added].sort((a, b) => a.olderMs - b.olderMs), lowersFloor: true, floorMs: oldest.ms };
  }
  const next: ViewGap[] = [];
  let changed = false;
  for (const gap of gaps) {
    const overlaps = oldest.ms <= gap.newerMs && newest.ms >= gap.olderMs;
    if (!overlaps) { next.push(gap); continue; }
    changed = true;
    const ids = new Set(stretch.map((row) => row.id));
    const asMessages = stretch.map((row) => ({ id: row.id, timestamp: new Date(row.ms).toISOString() }) as unknown as Message);
    if (ids.has(gap.olderId) || oldest.ms < gap.olderMs) {
      const narrowed = narrowGap(gap, "older", asMessages, false);
      if (narrowed) next.push({ ...narrowed, count: null, heightPx: null });
    } else if (ids.has(gap.newerId) || newest.ms > gap.newerMs) {
      const narrowed = narrowGap(gap, "newer", asMessages, false);
      if (narrowed) next.push({ ...narrowed, count: null, heightPx: null });
    } else {
      next.push({ ...gap, newerId: oldest.id, newerMs: oldest.ms, count: null, heightPx: null });
      next.push({ ...gap, olderId: newest.id, olderMs: newest.ms, count: null, heightPx: null });
    }
  }
  if (!changed) return unchanged;
  next.sort((a, b) => a.olderMs - b.olderMs);
  return { gaps: next, lowersFloor: false, floorMs: null };
}

/**
 * When message `id` was written, by its id alone, in ms, or null when the id says nothing of it: a
 * Slack id is its `ts` (seconds with a fraction), a Discord id a snowflake (ms since 2015 in its high
 * bits). Used only to put a link's box where its message will land before the message is held
 * (`#2426` R3); never as the message's time once it is.
 */
export function timeOfId(id: string): number | null {
  if (/^\d{9,11}\.\d{1,6}$/.test(id)) return Math.round(parseFloat(id) * 1000);
  if (/^\d{17,20}$/.test(id)) {
    try {
      return Number((BigInt(id) >> 22n) + 1420070400000n);
    } catch (_error) {
      return null;
    }
  }
  return null;
}

/**
 * The height a link's box takes (`#2426` R3): the median of the message rows measured around where
 * it stands, so the message, when it comes, lands with its top near the box's (a typical message
 * moves nothing). `fallback` when nothing is measured; never under it, never over `max`.
 */
export function typicalRowPx(heights: readonly number[], fallback: number, max: number): number {
  const sorted = heights.filter((h) => Number.isFinite(h) && h > 0).sort((a, b) => a - b);
  if (!sorted.length) return fallback;
  const mid = Math.floor(sorted.length / 2);
  const median = sorted.length % 2 ? (sorted[mid] as number) : ((sorted[mid - 1] as number) + (sorted[mid] as number)) / 2;
  return Math.round(Math.min(Math.max(median, fallback), Math.max(fallback, max)));
}

/**
 * What the seams below the reader add to "how much is below" (`#2426`, liaison 1150 (B)(a)): the
 * messages counted seams stand for, and whether any seam's count is unknown, so the jump to the newest
 * says "at least". `readingBottom` is the reading row's bottom; a seam whose top is at or under it is
 * below the reader.
 */
export function seamsBelow(seams: readonly { top: number; count: number | null }[], readingBottom: number): { count: number; open: boolean } {
  let count = 0;
  let open = false;
  for (const seam of seams) {
    if (seam.top < readingBottom - 1) continue;
    if (seam.count === null) open = true;
    else count += seam.count;
  }
  return { count, open };
}

/**
 * The words a gap says: "42 messages not loaded", "1 message not loaded", or "Messages not loaded" uncounted.
 * After its last fill failed, it says the seam is tappable too ("· tap to load"): the reader's tap
 * is then the way to ask again (liaison 1126).
 */
export function gapLine(gap: ViewGap, failed = false): string {
  const words = gap.count === null ? "Messages not loaded" : `${gap.count} ${gap.count === 1 ? "message" : "messages"} not loaded`;
  return failed ? `${words} · tap to load` : words;
}

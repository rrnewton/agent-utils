// Which held messages stay resident in one channel's store (`#2427 vibe-talk-windowed-default-on`,
// the interim held-rows cap; the residency layer of `#2426`'s loading program, which later steps
// extend with islands rather than replace).
//
// A long session used to hold every message it ever read or was sent, and draw them all: the
// owner's page held about 80 screens of rows. Residency keeps the PINNED rows, the NEWEST WINDOW
// of a view, and gives up the oldest run beyond it. The store gives the run up, every view at once
// (one store serves Main, All and each thread), and each view that lost a row says so in its cover
// (`more`, its floor raised), so a projection never claims what the store no longer holds.
//
// A run is given up only down to a PAGE BOUNDARY the page holds a cursor for: the oldest row of a
// page a read folded, whose `next_before` reads exactly the rows older than it. The view's walk back
// then starts there, so scrolling up brings the run back with no hole. v0 gives up runs only at the
// oldest edge, only while the reader stands at the newest with no search open, and only in Main and
// All; islands (`#2426` step S2) add runs in the middle.
//
// Pure: no DOM, no clock. The page asks for a plan and applies it through its own seams.

import type { Message } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { inViewOf, threadOf, timeOf, viewOfKey } from "./canon.ts";

/** Why a row may not be given up. */
export type PinReason =
  | "anchor"
  | "selection"
  | "speaking"
  | "jump"
  | "search"
  | "quote"
  | "reply"
  | "back"
  | "unread";

/** How much of a view stays resident (`core/settings` `residencyBudget`). */
export interface ResidencyBudgetShape {
  /** Rows of the view's newest window, always kept. */
  readonly newestRows: number;
  /** Rows of one island far back from the newest (`#2426` islands; not read by v0). */
  readonly islandRows: number;
  /** Bytes the held rows may cost (unused in v0: the count is the bound). */
  readonly maxBytes: number;
}

/** A page boundary the page holds a cursor for: `nextBefore` reads the rows older than `oldestId`. */
export interface PageBoundary {
  readonly oldestId: string;
  readonly oldestMs: number;
  readonly nextBefore: string;
}

/** One held row of the view, as residency needs it. */
export interface HeldRow {
  readonly id: string;
  readonly ms: number;
}

/** What one plan is asked over. */
export interface ResidencyInput {
  /** The view's held rows (its projection), oldest first. */
  readonly held: readonly HeldRow[];
  /** Ids no plan may give up. */
  readonly pins: ReadonlySet<string>;
  /** The row the reader is holding, or null when the reader stands at the foot. */
  readonly anchor: string | null;
  /** Whether the reader stands at the newest. */
  readonly atNewest: boolean;
  /** Whether a search is open (its placed and loaded hits are never given up). */
  readonly searchOpen: boolean;
  /** The page boundaries the page holds cursors for, in any order. */
  readonly boundaries: readonly PageBoundary[];
  /** What one row costs in bytes (unused in v0). */
  bytesOf(id: string): number;
}

/** A run of the oldest held rows given up: everything before the cut, which is a page boundary. */
export interface DiscardedSpan {
  readonly firstId: string;
  readonly lastId: string;
  /** The first KEPT row's time and id: every message ordered before it goes. */
  readonly cutMs: number;
  readonly cutId: string;
  readonly count: number;
  /** The run's drawn height where the page measured it (an island step's gap box), else null. */
  readonly heightPx: number | null;
  readonly edge: "oldest";
  /** The cursor that reads exactly the rows older than the first kept row. */
  readonly nextBefore: string;
}

/** What stays and what goes. `keep` and the spans' rows partition `held`. */
export interface ResidencyPlan {
  readonly keep: readonly string[];
  readonly discard: readonly DiscardedSpan[];
}

/**
 * The plan for `input` under `budget`. Total: any input gives a plan. Gives up at most one run, the
 * oldest, ending at a page boundary, never a pinned row, never one of the newest `newestRows`; and
 * nothing unless the reader stands at the foot of the newest with no search open.
 */
export function planResidency(input: ResidencyInput, budget: ResidencyBudgetShape): ResidencyPlan {
  const ids = input.held.map((row) => row.id);
  const all: ResidencyPlan = { keep: ids, discard: [] };
  if (!input.atNewest || input.searchOpen || input.anchor !== null) return all;
  const newest = Number.isFinite(budget.newestRows) ? Math.max(1, Math.floor(budget.newestRows)) : ids.length;
  if (ids.length <= newest) return all;
  // The cut is the index of the first kept row: rows before it go. At most this far, so the newest
  // window stays whole, and no further than the oldest pinned row.
  let limit = ids.length - newest;
  for (let index = 0; index < limit; index += 1) {
    if (input.pins.has(ids[index] as string)) {
      limit = index;
      break;
    }
  }
  const boundaries = new Map<string, PageBoundary>();
  for (const boundary of input.boundaries) boundaries.set(boundary.oldestId, boundary);
  for (let cut = limit; cut > 0; cut -= 1) {
    const boundary = boundaries.get(ids[cut] as string);
    if (!boundary) continue;
    const kept = input.held[cut] as HeldRow;
    return {
      keep: ids.slice(cut),
      discard: [{
        firstId: ids[0] as string,
        lastId: ids[cut - 1] as string,
        cutMs: kept.ms,
        cutId: kept.id,
        count: cut,
        heightPx: null,
        edge: "oldest",
        nextBefore: boundary.nextBefore,
      }],
    };
  }
  return all;
}

/** Whether `message` is ordered before the cut (`cutMs`, then `cutId`). */
function beforeCut(message: Message, cutMs: number, cutId: string): boolean {
  const ms = timeOf(message, "timestamp");
  if (!Number.isFinite(ms)) return false;
  if (ms !== cutMs) return ms < cutMs;
  return String(message.id) < cutId;
}

/** What giving up a run needs from the page. */
export interface DiscardFacts {
  /** The key of the view the run was planned over: its walk back starts at the span's cursor. */
  readonly viewKey: string;
  /** Ids never given up, in any view. */
  readonly pins: ReadonlySet<string>;
  /** Whether `message` is still to do over the Done set `done` (as the saved copy's trim asks). */
  stillToDo(message: Message, done: ReadonlySet<string>): boolean;
}

/** What giving up a run did: the messages the store gave up, and each view key whose cover it raised. */
export interface DiscardResult {
  readonly removed: readonly Message[];
  readonly raised: readonly string[];
}

/**
 * Give up `span` from `canon`: every message ordered before its cut, in any view, except pinned ones.
 * Each view that lost a row says there is more below it and raises its floor to the cut; the view
 * the plan was made over walks back from the span's cursor, every other one re-reads for its cursor
 * as after a reload. Thread summaries (the cards) are kept. A reply given up that was read counts as
 * one more its thread's store lacks for being old (`#222 unread-replies`), as the saved copy's trim
 * counts it, so its chip neither gains nor loses "new" replies.
 */
export function applyDiscard(canon: ChannelCanon, span: DiscardedSpan, facts: DiscardFacts): DiscardResult {
  const removed: Message[] = [];
  const kept: Message[] = [];
  for (const message of canon.messages) {
    if (beforeCut(message, span.cutMs, span.cutId) && !facts.pins.has(String(message.id))) removed.push(message);
    else kept.push(message);
  }
  if (!removed.length) return { removed, raised: [] };
  canon.messages = kept;
  const done = canon.dismissed;
  for (const message of removed) {
    const thread = threadOf(message);
    if (!thread || (message.thread as { is_root?: unknown } | null | undefined)?.is_root === true) continue;
    const gap = canon.replyGaps.get(thread);
    if (!Number.isInteger(gap) || facts.stillToDo(message, done)) continue;
    canon.replyGaps.set(thread, (gap as number) + 1);
    const settled = canon.settledByDone.get(thread);
    if (Number.isInteger(settled)) canon.settledByDone.set(thread, (settled as number) + 1);
  }
  const raised: string[] = [];
  for (const [key, cover] of canon.views) {
    const { view, thread } = viewOfKey(key);
    if (view === "threads") continue;
    if (!removed.some((message) => inViewOf(message, view, thread, canon.scope))) continue;
    cover.more = true;
    cover.floor = cover.floor === null ? span.cutMs : Math.max(cover.floor, span.cutMs);
    if (cover.dense !== null && cover.dense < span.cutMs) cover.dense = null;
    cover.cursor = key === facts.viewKey ? span.nextBefore : null;
    raised.push(key);
  }
  return { removed, raised };
}

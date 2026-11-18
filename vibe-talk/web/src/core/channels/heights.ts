// Height estimation for unmounted rows (`#1846 vibe-talk-windowed-rendering`, step 2; inert
// until step 3 mounts a window). The scroller needs a total height and per-row offsets for
// rows that are not mounted. Measured heights are kept per message id and width bucket (a
// phone, a desk, and a Discuss-narrowed list are different buckets; a bucket's measures are
// reused when the reader returns to that width). Unmeasured rows are estimated from the
// median measured height of their row class at that bucket, refined by body length until
// first measured. One number per row, computed once per entry — never per scroll frame.

import type { Message } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";

/** Height used before anything at a bucket has been measured (phone fixture median). */
export const DEFAULT_ROW_PX = 84;

/** The row classes estimates distinguish: shape moves height more than text alone does. */
export type RowClass = "plain" | "reply" | "root" | "collapsed";

interface HeightState {
  /** `id|bucket` -> measured px. */
  measured: Map<string, number>;
  /** `class|bucket` -> measured px samples (bounded; medians are taken over the last few). */
  samples: Map<string, number[]>;
}

const states = new WeakMap<ChannelCanon, HeightState>();
const SAMPLES_KEPT = 24;

function stateOf(canon: ChannelCanon): HeightState {
  let state = states.get(canon);
  if (!state) {
    state = { measured: new Map(), samples: new Map() };
    states.set(canon, state);
  }
  return state;
}

/** The class of a message for estimation: root, reply, collapsed (folded), else plain. */
export function rowClassOf(message: Message, folded: boolean): RowClass {
  if (folded) return "collapsed";
  const thread = message.thread as { is_root?: unknown } | null | undefined;
  if (thread && thread.is_root === true) return "root";
  if (thread) return "reply";
  return "plain";
}

/** Record a row's real height as it unmounts (or is measured in place). */
export function noteMeasured(canon: ChannelCanon, id: string, px: number, bucket: string, cls: RowClass): void {
  if (!Number.isFinite(px) || px <= 0) return;
  const state = stateOf(canon);
  state.measured.set(`${id}|${bucket}`, Math.round(px));
  const key = `${cls}|${bucket}`;
  const held = state.samples.get(key) || [];
  held.push(Math.round(px));
  state.samples.set(key, held.slice(-SAMPLES_KEPT));
}

function median(values: readonly number[]): number {
  if (!values.length) return DEFAULT_ROW_PX;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)] as number;
}

/**
 * The height to lay out for `message` at `bucket`: its measured height where one stands,
 * else its class median refined by body length (about 40 chars a line over the class's
 * single-line share), else the default. Pure over the index; safe per window change.
 */
export function heightFor(canon: ChannelCanon, message: Message, bucket: string, folded: boolean): number {
  const state = stateOf(canon);
  const measured = state.measured.get(`${String(message.id)}|${bucket}`);
  if (measured !== undefined) return measured;
  const cls = rowClassOf(message, folded);
  const base = median(state.samples.get(`${cls}|${bucket}`) || []);
  if (cls === "collapsed") return base;
  const lines = Math.max(1, Math.ceil((message.content?.length || 0) / 40));
  // The class median already carries its typical lines; only longer bodies add height.
  return lines <= 3 ? base : Math.round(base * (1 + (lines - 3) * 0.18));
}

/** The measured height standing for `id` at `bucket`, or null where none does. */
export function measuredHeight(canon: ChannelCanon, id: string, bucket: string): number | null {
  const held = stateOf(canon).measured.get(`${id}|${bucket}`);
  return held === undefined ? null : held;
}

/** Total estimated height of `messages[from..to)` at `bucket` (a spacer's height). */
export function rangeHeight(
  canon: ChannelCanon,
  messages: readonly Message[],
  from: number,
  to: number,
  bucket: string,
  folded: (message: Message) => boolean,
): number {
  let total = 0;
  for (let at = Math.max(0, from); at < Math.min(messages.length, to); at += 1) {
    total += heightFor(canon, messages[at] as Message, bucket, folded(messages[at] as Message));
  }
  return total;
}

/**
 * The scroll correction a newly measured row asks for: its real height less what the layout
 * assumed. The anchoring authority (step 3) applies it once per window transaction, in the
 * same frame, only for rows above the anchor.
 */
export function correctionFor(assumedPx: number, realPx: number): number {
  return Math.round(realPx) - Math.round(assumedPx);
}

/** Drop a removed message's measures. */
export function forgetHeight(canon: ChannelCanon, id: string): void {
  const state = stateOf(canon);
  for (const key of [...state.measured.keys()]) {
    if (key.startsWith(`${id}|`)) state.measured.delete(key);
  }
}

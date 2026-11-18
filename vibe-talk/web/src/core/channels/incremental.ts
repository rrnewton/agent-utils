// Incremental updates to the cached channel state (`#1846 vibe-talk-windowed-rendering`, step 1;
// owner thread 182: "much more incrementality in those computations. Incremental updates to a
// cached state."). Live events and page folds touch the store BY MESSAGE ID: only the entries
// an event names change object, advance in version, or move in the order; everything else keeps
// its object, its version, and its place. No event recomputes the whole list — there is no
// full sort, no full scan for a handful of ids, and no array replacement when nothing changed,
// so a renderer keyed on object identity (rows.ts patchRows) has nothing to redraw. Off-screen
// changes therefore cost a map write and a splice, never DOM work; that DOM half is step 3.

import type { Message, ThreadSummary } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { sameMessage, timeOf } from "./canon.ts";

/** Per-store incremental bookkeeping, kept beside the store it describes (never saved). */
interface IncrementalState {
  /** Monotonic clock: one tick per entry actually changed or removed. */
  clock: number;
  /** By prefixed id (`m:<id>` / `t:<id>`), the clock tick of its last real change. */
  versions: Map<string, number>;
  /** By prefixed id, the clock tick at which it was removed (tombstones for since-queries). */
  gone: Map<string, number>;
  /** Message id -> the store's object, valid only while `indexed` is the store's array. */
  index: Map<string, Message>;
  indexed: Message[] | null;
}

const states = new WeakMap<ChannelCanon, IncrementalState>();

function stateOf(canon: ChannelCanon): IncrementalState {
  let state = states.get(canon);
  if (!state) {
    state = { clock: 0, versions: new Map(), gone: new Map(), index: new Map(), indexed: null };
    states.set(canon, state);
  }
  return state;
}

const messageKey = (id: string): string => `m:${id}`;
const threadKey = (id: string): string => `t:${id}`;

function bump(state: IncrementalState, key: string): void {
  state.clock += 1;
  state.versions.set(key, state.clock);
  state.gone.delete(key);
}

function tombstone(state: IncrementalState, key: string): void {
  state.clock += 1;
  state.gone.set(key, state.clock);
  state.versions.delete(key);
}

/** The store's clock now: hand it to `changedIdsSince` after an event to learn what it touched. */
export function clockOf(canon: ChannelCanon): number {
  return stateOf(canon).clock;
}

/** The tick of message `id`'s last real change, or 0 when it never changed in this page's life. */
export function versionOf(canon: ChannelCanon, id: string): number {
  return stateOf(canon).versions.get(messageKey(String(id))) || 0;
}

/** The tick of thread summary `id`'s last real change, or 0. */
export function threadVersionOf(canon: ChannelCanon, id: string): number {
  return stateOf(canon).versions.get(threadKey(String(id))) || 0;
}

/**
 * Ids (unprefixed message ids) whose entry changed or was removed after tick `since`. This is
 * the step-3 renderer's question — "which rows could possibly need work" — answered from the
 * cache without looking at the DOM. Removals report the id too: an unmount is work of a kind.
 */
export function changedIdsSince(canon: ChannelCanon, since: number): string[] {
  const state = stateOf(canon);
  const changed: string[] = [];
  for (const [key, at] of state.versions) if (at > since && key.startsWith("m:")) changed.push(key.slice(2));
  for (const [key, at] of state.gone) if (at > since && key.startsWith("m:")) changed.push(key.slice(2));
  return changed;
}

/**
 * The store's messages by id. Rebuilt only when the store's array itself was replaced (a page
 * fold that still assigns wholesale, a restore); incremental upserts keep it current in place,
 * so the live path never pays the rebuild.
 */
export function messageIndex(canon: ChannelCanon): Map<string, Message> {
  const state = stateOf(canon);
  if (state.indexed !== canon.messages) {
    state.index = new Map(canon.messages.map((message) => [String(message.id), message]));
    state.indexed = canon.messages;
  }
  return state.index;
}

/** First index in `items` (sorted by `field`) whose time is after `at`; `items.length` if none. */
function upperBound<T>(items: readonly T[], at: number, field: string): number {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (timeOf(items[mid], field) <= at) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** First index in `items` whose time is at or after `at`. */
function lowerBound<T>(items: readonly T[], at: number, field: string): number {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (timeOf(items[mid], field) < at) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * Put `item` into sorted `items` at its time: after equals by default (the stable order
 * `inTimeOrder` gives a kept-then-arriving merge), before them with `afterEquals` false (an
 * arriving-then-kept merge). Mutates and returns the insertion index. Callers that cannot
 * promise sorted input with finite times fall back to `inTimeOrder`, as before.
 */
export function insertSorted<T>(items: T[], item: T, field: string, afterEquals = true): number {
  // No readable time: the order given is the order kept (`inTimeOrder`'s fallback), i.e. last.
  if (!Number.isFinite(timeOf(item, field))) {
    items.push(item);
    return items.length - 1;
  }
  const at = afterEquals
    ? upperBound(items, timeOf(item, field), field)
    : lowerBound(items, timeOf(item, field), field);
  items.splice(at, 0, item);
  return at;
}

/** Record that message `id` really changed, for folds that manage the array themselves. */
export function noteMessageChanged(canon: ChannelCanon, id: string): void {
  bump(stateOf(canon), messageKey(String(id)));
}

/** Record that message `id` was removed, for folds that manage the array themselves. */
export function noteMessageRemoved(canon: ChannelCanon, id: string): void {
  tombstone(stateOf(canon), messageKey(String(id)));
}

/** Record that thread summary `id` really changed. */
export function noteThreadChanged(canon: ChannelCanon, id: string): void {
  bump(stateOf(canon), threadKey(String(id)));
}

/** Forget the by-id index (the store's array was replaced wholesale); it rebuilds on demand. */
export function reindexCanon(canon: ChannelCanon): void {
  stateOf(canon).indexed = null;
}

/**
 * Apply one arriving copy of a message to the store, incrementally. A copy the store already
 * holds word for word keeps the store's object and advances nothing — the no-change case that
 * must cost no row work downstream. A new or changed copy replaces/inserts at its time by
 * binary search (one splice, no sort). Answers the object the store kept.
 */
export function upsertMessage(canon: ChannelCanon, arriving: Message): Message {
  const state = stateOf(canon);
  const index = messageIndex(canon);
  const id = String(arriving.id);
  const held = index.get(id);
  if (held && sameMessage(held, arriving)) return held;
  if (held) {
    const at = canon.messages.indexOf(held);
    if (at >= 0 && timeOf(held, "timestamp") === timeOf(arriving, "timestamp")) {
      canon.messages[at] = arriving;
    } else {
      if (at >= 0) canon.messages.splice(at, 1);
      insertSorted(canon.messages, arriving, "timestamp");
    }
  } else {
    insertSorted(canon.messages, arriving, "timestamp");
  }
  index.set(id, arriving);
  state.indexed = canon.messages;
  bump(state, messageKey(id));
  return arriving;
}

/** Remove message `id` if held. True when it was: a tombstone is recorded either way it goes. */
export function removeMessage(canon: ChannelCanon, id: string): boolean {
  const state = stateOf(canon);
  const index = messageIndex(canon);
  const held = index.get(String(id));
  if (!held) return false;
  const at = canon.messages.indexOf(held);
  if (at >= 0) canon.messages.splice(at, 1);
  index.delete(String(id));
  state.indexed = canon.messages;
  tombstone(state, messageKey(String(id)));
  return true;
}

/** Apply one arriving thread summary, as `upsertMessage` does for messages. */
export function upsertThread(canon: ChannelCanon, arriving: ThreadSummary): ThreadSummary {
  const state = stateOf(canon);
  const id = String(arriving.id);
  const at = canon.threads.findIndex((held) => String(held.id) === id);
  const held = at >= 0 ? canon.threads[at] : undefined;
  if (held && JSON.stringify(held) === JSON.stringify(arriving)) return held;
  if (held && timeOf(held, "updated_at") === timeOf(arriving, "updated_at")) {
    canon.threads[at] = arriving;
  } else {
    if (held) canon.threads.splice(at, 1);
    insertSorted(canon.threads, arriving, "updated_at");
  }
  bump(state, threadKey(id));
  return arriving;
}

/**
 * Bytes one cached message costs, estimated from its wire form. An estimate on purpose: the
 * reach-back cap (settings/windowed.ts) needs a budget number per message, not an audit.
 * Measured on text fixtures at ~1.5 KB; attachments and summaries run larger, which the cap's
 * byte half (not just its count half) exists to catch.
 */
export function estimateMessageBytes(message: Message): number {
  try {
    return JSON.stringify(message).length;
  } catch (_error) {
    return 0;
  }
}

/** Estimated bytes of the whole cached message data for `canon` (summaries excluded). */
export function cacheDataBytes(canon: ChannelCanon): number {
  let total = 0;
  for (const message of canon.messages) total += estimateMessageBytes(message);
  return total;
}

/**
 * Local search/filter over the CACHED DATA (`#1846`, owner thread 186): matches message text
 * (and author, where present) without touching the DOM, so a desktop cache holding thousands
 * answers instantly and mounting stays the render window's business (step 3 caps it). Returns
 * the matching store objects in store order; an empty needle matches nothing.
 */
export function filterCachedMessages(canon: ChannelCanon, needle: string): Message[] {
  const wanted = needle.trim().toLowerCase();
  if (!wanted) return [];
  return canon.messages.filter((message) =>
    message.content.toLowerCase().includes(wanted) || message.author.toLowerCase().includes(wanted));
}

/** Drop the incremental bookkeeping for `canon` (a store being discarded wholesale). */
export function forgetCanon(canon: ChannelCanon): void {
  states.delete(canon);
}

// What one channel's store holds of every thread's replies, and what their counts say (`#222
// unread-replies`, `#231 two-column-polish`; `#624 voice-page-modularity`, moved from the legacy
// script; plan step 10).

import type { Message, MessageThread, ThreadSummary } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { threadOf } from "./canon.ts";
import { exactCount } from "./fold.ts";

/** What the store holds of every thread's replies. */
export interface ReplyTally {
  /** Each thread's replies the store holds, by thread id: how many, and how many are unread. */
  readonly threads: Map<string, { held: number; unread: number }>;
  /** The thread record of every message the store holds with one, by message id. */
  readonly records: Map<string, MessageThread>;
  /** The thread summaries the store holds, by thread id. */
  readonly summaries: Map<string, ThreadSummary>;
  /** The same summaries, by their root's message id. */
  readonly byRoot: Map<string, ThreadSummary>;
}

/** A tally of nothing: for a store that is not the channel on screen, or a service with no threads. */
export function emptyTally(): ReplyTally {
  return { threads: new Map(), records: new Map(), summaries: new Map(), byRoot: new Map() };
}

/**
 * What store `canon` holds of every thread's replies (`ReplyTally`), a reply unread by `unread`
 * (`stillToDo` over the store's read state). One walk over the store, for a
 * pass over the rows that asks after every thread on screen.
 */
export function replyTallyOf(canon: ChannelCanon, unread: (message: Message, done: Set<string>) => boolean): ReplyTally {
  const tally = emptyTally();
  for (const message of canon.messages) {
    const id = threadOf(message);
    if (!id) continue;
    const thread = message.thread as MessageThread;
    tally.records.set(String(message.id), thread);
    if (thread.is_root === true || id === canon.scope) continue;
    const entry = tally.threads.get(id) || { held: 0, unread: 0 };
    entry.held += 1;
    if (unread(message, canon.dismissed)) entry.unread += 1;
    tally.threads.set(id, entry);
  }
  for (const summary of canon.threads) {
    tally.summaries.set(String(summary.id), summary);
    if (summary.root) tally.byRoot.set(String(summary.root.id), summary);
  }
  return tally;
}

/**
 * The most replies thread `id`, headed by `message`, is reported to have, by an exact count: the
 * summary's or the root's record's, whichever is higher, because either may be the older — a summary
 * from a thread read an hour ago, a root as Main last read it. A read that carries the root puts the
 * summary to its count (`noteReplyGaps`), so a count that dropped — a reply deleted upstream — is the
 * count from the next read of Main or All on. Null when neither is exact (Discord's own threads give
 * an estimate, from which no reply can be inferred).
 */
export function exactReplyCount(
  id: string,
  message: Message | null,
  tally: ReplyTally,
  summary: ThreadSummary | null = null,
): number | null {
  // The store's copy of the root before the row's own, which can be older: a view held since before
  // the last read (`#222 unread-replies`, a count that dropped kept its old value on the row).
  const record = message ? tally.records.get(String(message.id)) || message.thread : null;
  const counts = [tally.summaries.get(id), record, summary].map(exactCount).filter((count) => count !== null);
  return counts.length ? Math.max(...counts) : null;
}

/**
 * The thread record the store holds for `message`, by `tally`: `heldThreadRecord`'s answer — the
 * store's copy's, the row's own, or the summary of the thread it starts — without a walk per row.
 */
export function tallyRecord(message: Message, tally: ReplyTally): MessageThread | null {
  const id = String(message.id);
  const record = tally.records.get(id) || (threadOf(message) ? (message.thread as MessageThread) : null);
  if (record) return record;
  const summary = tally.byRoot.get(id);
  return summary ? {
    id: String(summary.id), root_message_id: id, is_root: true,
    reply_count: summary.reply_count, reply_count_exact: summary.reply_count_exact,
  } as MessageThread : null;
}

/**
 * How many of thread `id`'s replies were posted since the last read that brought its newest, and
 * are not held: its exact count, less the replies held and the ones it lacks for being old
 * (`noteReplyGaps`). Not one of them is known to be read, so each may be unread. Zero without an
 * exact count. `message` is its root, when held; `summary` one more summary to take a count from.
 */
export function unseenRepliesIn(
  canon: ChannelCanon,
  id: string,
  message: Message | null,
  tally: ReplyTally,
  summary: ThreadSummary | null = null,
): number {
  const count = exactReplyCount(id, message, tally, summary);
  if (count === null) return 0;
  const entry = tally.threads.get(id);
  return Math.max(0, count - (entry ? entry.held : 0) - (canon.replyGaps.get(id) || 0));
}

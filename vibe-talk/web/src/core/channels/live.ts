// What a live arrival or an acknowledged send does to a channel's store (`#624 voice-page-modularity`,
// moved from the legacy script; plan step 10).

import type { Message, MessageThread } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { inTimeOrder, threadOf, timeOf } from "./canon.ts";
import { upsertThreadSummary } from "./fold.ts";
import type { CopyOrder } from "./order.ts";

/** What `foldLiveIn` asks of the page. */
export interface LiveFoldDependencies {
  /** The copy-order ledger. */
  readonly ledger: CopyOrder;
  /** Where a message that goes into the store now, other than from a read, stands (`orderNow`). */
  orderNow(): number;
  /** The next arrival's number in the order the stream left messages unplaced (`unplacedArrivals`). */
  nextArrival(): number;
}

/** `message`'s thread record where it names the thread (`#528`), else null. */
export function namedRecord(message: unknown): MessageThread | null {
  const record = message ? (message as { thread?: MessageThread | null }).thread : undefined;
  return record && record.is_root === true && typeof record.display_name === "string" && record.display_name.trim()
    ? record : null;
}

/**
 * A live arrival or an acknowledged send: every view of store `canon` it belongs to has it from now on.
 * Answers the copy the store kept, which is the one to draw.
 *
 * A COPY WITHOUT A THREAD RECORD NEVER REPLACES ONE WITH. No provider moves a message between
 * threads, so a held record is still true; what changed is only that this copy came through a
 * path that does not carry it. The Google Chat bridge's stream is such a path, and its echo of a
 * reply this page had just posted into a thread used to overwrite the acknowledgement's record —
 * after which All drew the reply as a main-channel message and a reply to it went there too.
 *
 * `live` is a copy from the stream. One without a record, of a message no page has placed, is
 * `unplaced` until one does; an acknowledged send is the server's own answer and is placed.
 *
 * A NEW REPLY IS ONE MORE ON ITS THREAD'S COUNTS. `#231 two-column-polish`. A reply the store did
 * not hold, from the stream as it was posted or acknowledged as sent from here, is counted on its
 * thread's summary — when that summary's last activity is older than the reply, so its count did not
 * count it — and on the store's copy of the root, as the next read that brings either will count it.
 * Main's Thread(N), a root's N replies and a card's count used to wait up to a minute for that read,
 * while the unread chip beside them counted the reply at once: they gave the reported count, or the
 * replies held where those were more, and on a thread whose older replies are not held — the usual
 * case behind Main — they never were. Not a `replayed` copy: the stream replays what it had before it
 * attached, and a read since may have counted it already. Not a root's count of none, which needs
 * nothing: the reply is all its thread's replies, and is held.
 *
 * `read`, given only for the copy a reply's read of All brought of a message the stream left unplaced
 * (`placeReplyTarget`), is where that read was sent (`sentNumberOf`). The copy stands there
 * (`copyOrder`), and says what the read found, not the message as it was posted (`postedCopies`).
 * `#316 late-counts-resurrect`: taken for the stream's copy, built on its bare one, it let the read of
 * All for Main's counts, sent before the reply's read and landing after it, take its own older copy
 * over it. A bridge's finished answer, which the reply's read had brought, went back to the text it
 * was pushed with, saved, and shown in its thread or in All until a read of either landed. Nor does that
 * copy add to its root's count of replies (`#328 removal-outside-window`).
 */
export function foldLiveIn(
  canon: ChannelCanon,
  message: Message,
  live: boolean,
  replayed: boolean,
  read: number | null,
  deps: LiveFoldDependencies,
): Message {
  const id = String(message.id);
  const held = canon.messages.find((candidate) => String(candidate.id) === id);
  let kept: Message = !threadOf(message) && threadOf(held) ? { ...message, thread: (held as Message).thread } : message;
  // `#219 emoji-reactions`. NOR DOES A COPY WITHOUT REACTIONS REPLACE ONE WITH THEM, for the same
  // reason: the stream's copy cannot see them, and the bridge's echo of a message lands after the
  // read that brought its acknowledgements. Absent is "not seen"; only a read says "none".
  if (held && Array.isArray(held.reactions) && !Array.isArray(kept.reactions)) {
    kept = { ...kept, reactions: held.reactions };
  }
  // `#320 slack-edited-mark`. Nor does a copy that does not say when the message was edited take the
  // mark away: an edit is never undone, and the stream's copy may come by a path that cannot see it.
  if (held && held.edited_timestamp && !kept.edited_timestamp) {
    kept = { ...kept, edited_timestamp: held.edited_timestamp };
  }
  // `#408 copy-upstream-link`. Nor does a copy with no upstream address take one away: absent is
  // "this copy came by a path that formed none", and the address of a message does not change.
  if (held && held.upstream_url && !kept.upstream_url) {
    kept = { ...kept, upstream_url: held.upstream_url };
  }
  // `#528 thread-card-markdown-and-cap`. Nor does a root's copy whose record names nothing take its
  // thread's name and sentence away: the stream's copies are never named, only a page's are.
  const named = namedRecord(held);
  if (named && kept.thread && kept.thread.is_root === true && String(kept.thread.id) === String(named.id) &&
      kept.thread.display_name === undefined && kept.thread.summary === undefined) {
    kept = { ...kept, thread: { ...kept.thread, display_name: named.display_name, summary: named.summary } };
  }
  canon.messages = inTimeOrder(
    [...canon.messages.filter((candidate) => String(candidate.id) !== id), kept], "timestamp");
  // Where it stands in the order of reads (`copyOrder`, `#310 stale-live-read-flicker`): a copy a read
  // brought (`read`) where that read was sent. A message the store did not hold is newer than every read
  // sent before it arrived, and one it held is as new as the store's copy was (`copyBuiltOn`). Either
  // says the message as it was posted (`postedCopies`).
  if (read !== null) deps.ledger.place(kept, read);
  else if (!held) deps.ledger.orderCopy(kept, deps.orderNow());
  else if (held !== kept) deps.ledger.buildCopyOn(kept, held);
  if (read === null && held !== kept) deps.ledger.markPosted(kept);
  if (!live || threadOf(kept)) canon.unplaced.delete(id);
  else if (!held) {
    canon.unplaced.add(id);
    canon.unplacedSince.set(id, deps.nextArrival());
    canon.unplacedAnswered.delete(id);
  }
  const thread = threadOf(kept);
  // `#328 removal-outside-window`. Never the copy a reply's read brought (`read`): the store held that
  // message once, unplaced, and a newest page took it out, so the counts of that page or a later one
  // have it already. Counted again, a root said one reply more than its thread had.
  const fresh = !held && !replayed && read === null && thread !== null && (kept.thread as MessageThread).is_root !== true &&
    thread !== canon.scope;
  /** A summary's or a root's thread record's count with this reply, or null. */
  const withReply = (record: { reply_count?: number | null }) => (fresh && typeof record.reply_count === "number" && record.reply_count > 0
    ? record.reply_count + 1 : null);
  const summary = thread && canon.threads.find((candidate) => String(candidate.id) === thread);
  if (summary && timeOf(kept, "timestamp") > timeOf(summary, "updated_at")) {
    const count = withReply(summary);
    upsertThreadSummary(canon, { ...summary, updated_at: kept.timestamp, ...(count === null ? {} : { reply_count: count }) });
  }
  const root = fresh
    ? canon.messages.find((candidate) => threadOf(candidate) === thread && (candidate.thread as MessageThread).is_root === true)
    : undefined;
  const count = root ? withReply(root.thread as MessageThread) : null;
  if (root && count !== null) {
    const counted = { ...root, thread: { ...(root.thread as MessageThread), reply_count: count } };
    // As new as the root's copy, the count aside (`copyBuiltOn`).
    deps.ledger.buildCopyOn(counted, root);
    canon.messages = canon.messages.map((candidate) => (candidate === root ? counted : candidate));
  }
  return kept;
}

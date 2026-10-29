// Done, or put back, in a channel's store at once, ahead of the write that tells the server, and what
// a write that fails takes back (`#222 unread-replies`; `#624 voice-page-modularity`, moved from the
// legacy script; plan step 11). The store's read state, and the reply gaps Done on a root settles.

import type { Message, MessageThread } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { threadOf } from "./canon.ts";
import { exactReplyCount, replyTallyOf } from "./tally.ts";

/** What a Done or a put back changed in the store, for the write's settling to take back if it fails. */
export interface HeldDoneMark {
  /** The messages written. */
  readonly ids: string[];
  /** Done, or put back. */
  readonly done: boolean;
  /** The store changed, or null. */
  readonly canon: ChannelCanon | null;
  /** The ids whose state in that store this changed. */
  readonly flipped: string[];
  /** The threads whose unknown replies Done on their root settled, with the gap that set. */
  readonly settled: Array<[string, number]>;
  /** The threads whose settling putting their root back took back, with the gap it took. */
  readonly unsettled: Array<[string, number]>;
}

/**
 * Done, or put back, here, in store `canon` (null where the page holds none for the channel on screen):
 * the store's read state for `ids` follows at once, as `archivedIds` does,
 * so a reply dealt with from All, a thread or the Pinned list counts as read on Main straight away;
 * and no read sent before the write settles undoes it (`readSpeaksFor`). Answers what to give
 * `settleHeldDone` when the write has settled.
 *
 * `settles`: Done issued from Main's list, where a root's count of new replies is drawn, settles the
 * replies of its roots no read has placed (`settleUnknownReplies`). Done in All, in a thread or in the
 * Pinned list, where nothing says a root has new replies, does not. A put back, from anywhere, takes
 * back what Done settled (`unsettleReplies`).
 */
export function markDoneIn(
  canon: ChannelCanon | null,
  marked: string[],
  done: boolean,
  settles: boolean,
  unread: (message: Message, done: Set<string>) => boolean,
): HeldDoneMark {
  const flipped = canon ? marked.filter((id) => canon.dismissed.has(id) !== done) : [];
  for (const id of flipped) {
    if (done) (canon as ChannelCanon).dismissed.add(id);
    else (canon as ChannelCanon).dismissed.delete(id);
  }
  const settled = canon && done && settles ? settleUnknownReplies(canon, marked, unread) : [];
  const unsettled = canon && !done ? unsettleReplies(canon, marked) : [];
  return { ids: marked, done, canon, flipped, settled, unsettled };
}

/**
 * The write `mark` stands for has settled, `ok` or not (the read-state ledger told already, by the page). Failed, exactly what it changed in the store
 * goes back — never a copy from before it, which would undo whatever reads folded meanwhile, and in
 * another channel's store would save this one's state into it.
 */
export function settleDoneIn(mark: HeldDoneMark, ok: boolean): void {
  if (ok || mark.canon === null) return;
  const canon = mark.canon;
  for (const id of mark.flipped) {
    if (mark.done) canon.dismissed.delete(id);
    else canon.dismissed.add(id);
  }
  // Still Done's, lowered or not: not once a read has set the gap since.
  for (const [id] of mark.settled) {
    if (!canon.settledByDone.has(id)) continue;
    if (canon.replyGaps.get(id) === canon.settledByDone.get(id)) canon.replyGaps.delete(id);
    canon.settledByDone.delete(id);
  }
  for (const [id, gap] of mark.unsettled) {
    if (canon.replyGaps.has(id)) continue;
    canon.replyGaps.set(id, gap);
    canon.settledByDone.set(id, gap);
  }
}

/**
 * Done on a root settles the replies of its thread that no read has said anything of: they are read
 * as old, and stop counting as new. `#222 unread-replies`. The gap is unknown only for a thread no
 * read has placed — Main opened cold, before All; or, on Slack, a thread past the ten All brings
 * replies for (`passedOver`), which no later read of All brings either while nothing new happens
 * in it. Such a root said "N new" until the reader opened its thread, and Done on the root did not
 * clear it; Done is the reader saying they have dealt with it. A read that brings the replies later
 * places them again, unread ones included; putting the root back, or Undo, takes the settling back
 * (`settledByDone`, `unsettleReplies`).
 *
 * WHAT DONE ON A ROOT DOES NOT SETTLE, and why it differs from what it does: Done on a root is Done on
 * that message, and it never stands for a reply the page knows may be unread. A reply the page holds
 * keeps its own read state, which the server keeps and other devices share, until it is marked
 * itself ("1 unread" stays). Replies its count reports beyond a known gap stay new ("1 new" stays):
 * they were posted since a read that brought the thread's newest, or they were unread when cut from
 * the device's copy (`trimCanonField`), so nothing suggests the reader has seen them. Only replies the
 * page cannot place at all — old and seen, or new, it cannot tell — are taken as the reader says.
 * Answers the threads settled, with the gap each was given, for a write that fails to take back. A
 * reply unread by `unread` (`stillToDo` over the store's read state).
 */
export function settleUnknownReplies(
  canon: ChannelCanon,
  ids: readonly string[],
  unread: (message: Message, done: Set<string>) => boolean,
): Array<[string, number]> {
  const wanted = new Set(ids);
  const roots = canon.messages.filter((message) => wanted.has(String(message.id)) && threadOf(message) &&
    (message.thread as MessageThread).is_root === true && threadOf(message) !== canon.scope &&
    !canon.replyGaps.has(threadOf(message) as string));
  if (roots.length === 0) return [];
  const tally = replyTallyOf(canon, unread);
  const settled: Array<[string, number]> = [];
  for (const root of roots) {
    const id = String(threadOf(root));
    const count = exactReplyCount(id, root, tally);
    const entry = tally.threads.get(id);
    const gap = count === null ? 0 : Math.max(0, count - (entry ? entry.held : 0));
    if (gap === 0) continue;
    canon.replyGaps.set(id, gap);
    canon.settledByDone.set(id, gap);
    settled.push([id, gap]);
  }
  return settled;
}

/**
 * Putting a root back takes back what Done on it settled (`settleUnknownReplies`): its thread's gap
 * is unknown again, so its unplaced replies count as new as they did before the Done, and the root is
 * back under Hide read. Not once a read has set the gap (`noteReplyGaps` forgets Done's then): that
 * gap is the read's, and stands. Answers the threads it took back, with the gap each had, for a write
 * that fails to put back.
 */
export function unsettleReplies(canon: ChannelCanon, ids: readonly string[]): Array<[string, number]> {
  if (canon.settledByDone.size === 0) return [];
  const wanted = new Set(ids);
  const unsettled: Array<[string, number]> = [];
  for (const message of canon.messages) {
    const id = threadOf(message);
    if (!id || (message.thread as MessageThread).is_root !== true || !wanted.has(String(message.id))) continue;
    const gap = canon.settledByDone.get(id);
    if (gap === undefined) continue;
    canon.settledByDone.delete(id);
    if (canon.replyGaps.get(id) !== gap) continue;
    canon.replyGaps.delete(id);
    unsettled.push([id, gap]);
  }
  return unsettled;
}

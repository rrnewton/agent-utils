// Where a message the stream left unplaced is, as the next read says (`#185 reply-new-thread`, `#308
// live-reply-in-main`, `#316 late-counts-resurrect`; `#624 voice-page-modularity`, moved from the legacy
// script; plan step 10), and which newer pages speak for a message.

import type { Message, MessageThread } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { inTimeOrder, threadOf } from "./canon.ts";
import type { CopyOrder } from "./order.ts";

/** The thread a reply to `message` belongs in once it is placed: a reply's thread, else none. */
export function answeredThread(message: Message): string | null {
  const thread = threadOf(message);
  return thread && (message.thread as MessageThread).is_root !== true ? thread : null;
}

/**
 * Whether a newest page of `view` — of thread `thread`, for the thread view — sent after the read
 * numbered `number` (`sentNumberOf`) has landed in store `canon`: that page is the newer, whichever of
 * the two landed first. `#308 live-reply-in-main`.
 */
export function newerPage(canon: ChannelCanon, view: string, number: number, thread: string | null = null): boolean {
  const cover = canon.views.get(view === "thread" ? `thread:${thread}` : view);
  return Boolean(cover && (cover.fullSent || 0) > number);
}

/**
 * Whether a newest page that speaks for `message` — one of its thread, for a thread reply, and of Main
 * for any other message — sent after the read numbered `number` (`sentNumberOf`) has landed in store
 * `canon` (`newerPage`): that page carried the message, or says it is gone, whichever of the two
 * landed first. `#316 late-counts-resurrect`.
 *
 * A THREAD'S OWN PAGE TOO. Asking only of Main, a late read of All for Main's counts put back a reply
 * a newer newest page of its thread had taken out, removed upstream by a provider that pushes no
 * removals — or one removed before that thread was first read, never held — into the thread and the
 * saved store, where the polls after, deltas from that page's cursor, never took it out again.
 */
export function newerPageSpeaksFor(canon: ChannelCanon, message: Message, number: number): boolean {
  const thread = answeredThread(message);
  return thread === null ? newerPage(canon, "main", number) : newerPage(canon, "thread", number, thread);
}

/**
 * What `payload`, an answer of All that is not folded, says of the messages the stream left unplaced in
 * store `canon`: see "an answer of All that is not folded" in web/voice.js. Each it carries is placed — the store's
 * copy takes the thread record the answer's copy has, and nothing else of it, and one the answer carries
 * without a record is on the main channel — and one the store no longer holds is put back as the answer
 * carries it, unless a newest page sent after the answer (`number`, `sentNumberOf`) that speaks for it —
 * of All, or of its thread or Main (`newerPageSpeaksFor`) — has landed. True when it placed any.
 *
 * `held`, the store's copies as the read was sent (`heldNow`): a message whose copy in the store is
 * older than the answer in what it says (`copyOlderThan`) — the stream's bare copy always is
 * (`postedCopies`) — takes the answer's whole copy (`#316 late-counts-resurrect`). A bridge that streams
 * its answer by editing it, while the placing page was out, used to have its reply placed in its thread
 * with the text it was first pushed with, until the next read of that thread or of All. A copy newer
 * than the answer takes only the record, and stands where it stood (`copyBuiltOn`).
 */
export function placeFromPage(
  payload: { readonly messages?: readonly Message[] | null },
  canon: ChannelCanon,
  number: number,
  held: ReadonlyMap<string, Message> | null,
  ledger: CopyOrder,
): boolean {
  /** By id, the answer's copies of the messages left unplaced. */
  const carried = new Map<string, Message>();
  for (const message of payload.messages || []) {
    // Never one removed upstream (`noteRemoved`, `#315 removed-reply-target`).
    const id = String(message.id);
    if (canon.unplaced.has(id) && !canon.removed.has(id)) carried.set(id, message);
  }
  if (carried.size === 0) return false;
  let placed = false;
  canon.messages = canon.messages.map((stored) => {
    const id = String(stored.id);
    const copy = carried.get(id);
    if (!copy) return stored;
    carried.delete(id);
    canon.unplaced.delete(id);
    placed = true;
    if (held && ledger.copyOlderThan(stored, id, number, held, true)) {
      ledger.orderCopy(copy, number);
      return copy;
    }
    if (!threadOf(copy)) return stored;
    // Only the record, on a copy that stands where the store's did (`copyBuiltOn`).
    const recorded = { ...stored, thread: copy.thread };
    ledger.buildCopyOn(recorded, stored);
    return recorded;
  });
  // One the store no longer holds was taken out by a newest page that does not speak for it — one of
  // Main, which carries no thread reply, or one sent before it arrived — or by one sent after this
  // answer that does, and said it is gone: a page of All, of its thread for a thread reply, or of Main
  // for a message on the main channel (`newerPageSpeaksFor`).
  const back = newerPage(canon, "flat", number) ? []
    : [...carried.values()].filter((copy) => !newerPageSpeaksFor(canon, copy, number));
  if (back.length) {
    for (const copy of back) {
      canon.unplaced.delete(String(copy.id));
      ledger.orderCopy(copy, number);
    }
    canon.messages = inTimeOrder([...canon.messages, ...back], "timestamp");
    placed = true;
  }
  return placed;
}

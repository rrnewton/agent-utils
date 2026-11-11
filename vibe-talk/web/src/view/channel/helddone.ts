// Done, or put back, on held messages at once, the write's settling, and sending those
// writes in batches (`#624 voice-page-modularity`, moved from the legacy script). Words
// and order are as they were in the script. The store's part is core/channels'
// (`markDoneIn`, `settleDoneIn`); the read ledger is core/annotations', handed in.

import { markDoneIn, settleDoneIn, threadOf } from "../../core/channels/index.ts";
import type { ChannelStore, HeldDoneMark } from "../../core/channels/index.ts";
import type { ReadLedger } from "../../core/annotations/index.ts";
import type { Message } from "../../contract/index.ts";
import { DISMISS_BATCH } from "./channelrows.ts";

/** What marking and settling held Done asks of the page. */
export interface HeldDoneDependencies {
  readLedger: ReadLedger;
  providerFacts: { threading(): boolean };
  channelStore: ChannelStore;
  selectedChannelId(): unknown;
  selection(): { view: string };
  pinsView(): { only: boolean };
  stillToDo(message: Message, done: Set<string>): boolean;
  inboxPost(route: string, body: unknown): Promise<{ messages?: unknown[] } | null | undefined>;
}

/** Marking held messages Done, settling the write, and sending it in batches. */
export interface HeldDone {
  markHeldDone(ids: string[], done: boolean, settles?: boolean): HeldDoneMark;
  doneSettlesHere(): boolean;
  settleHeldDone(mark: HeldDoneMark, ok: boolean): void;
  heldDonePart(mark: HeldDoneMark, ids: string[]): HeldDoneMark;
  writeInBatches(route: string, ids: string[], landed: string[]): Promise<string[]>;
}

/** The held Done marking and settling over `deps`. */
export function createHeldDone(deps: HeldDoneDependencies): HeldDone {
  /**
   * Done, or put back, here: the store's read state for `ids` follows at once, as `doneState().archived` does,
   * so a reply dealt with from All, a thread or the Pinned list counts as read on Main straight away;
   * and no read sent before the write settles undoes it (`readSpeaksFor`). Answers what to give
   * `settleHeldDone` when the write has settled.
   *
   * `settles`: Done issued from Main's list, where a root's count of new replies is drawn, settles the
   * replies of its roots no read has placed (`settleUnknownReplies`). Done in All, in a thread or in the
   * Pinned list, where nothing says a root has new replies, does not. A put back, from anywhere, takes
   * back what Done settled (`unsettleReplies`).
   *
   */
  function markHeldDone(ids: string[], done: boolean, settles = false): HeldDoneMark {
    const marked = ids.map(String);
    // The epoch moves, and each has one more write on its way (`ReadLedger.writeStarted`).
    deps.readLedger.writeStarted(marked);
    const canon = deps.providerFacts.threading() && deps.channelStore.current().channel === String(deps.selectedChannelId())
      ? deps.channelStore.current() : null;
    // The store's part (web/src/core/channels `markDoneIn`): its read state, and the gaps Done settles.
    return markDoneIn(canon, marked, done, settles, deps.stillToDo);
  }

  /** Whether Done issued now is issued from Main's list, where roots say they have new replies (`markHeldDone`). */
  const doneSettlesHere = (): boolean => deps.providerFacts.threading() && deps.selection().view === "main" && !deps.pinsView().only;

  // `settleUnknownReplies` is web/src/core/channels's (`#624 voice-page-modularity`).

  // `unsettleReplies` is web/src/core/channels's (`#624 voice-page-modularity`).

  /**
   * The write `mark` stands for has settled, `ok` or not. Failed, exactly what it changed in the store
   * goes back — never a copy from before it, which would undo whatever reads folded meanwhile, and in
   * another channel's store would save this one's state into it.
   *
   */
  function settleHeldDone(mark: HeldDoneMark, ok: boolean): void {
    // The epoch moves, and each settles there once none is left on its way (`ReadLedger.writeSettled`).
    deps.readLedger.writeSettled(mark.ids, ok && mark.done);
    // Failed, exactly what it changed in the store goes back (web/src/core/channels `settleDoneIn`).
    settleDoneIn(mark, ok);
  }

  /**
   * The part of `mark` that names `ids`: for a write sent in batches, some of which the server took
   * before one failed. Each of Done's settlings goes with the part holding its thread's root.
   *
   */
  function heldDonePart(mark: HeldDoneMark, ids: string[]): HeldDoneMark {
    const wanted = new Set(ids);
    /** @param {[string, number]} entry */
    const rootIn = ([thread]: [string, number]): boolean => mark.canon !== null && mark.canon.messages.some((message) =>
      threadOf(message) === thread && (message as { thread?: { is_root?: unknown } }).thread?.is_root === true && wanted.has(String(message.id)));
    return {
      ...mark,
      ids: mark.ids.filter((id) => wanted.has(id)),
      flipped: mark.flipped.filter((id) => wanted.has(id)),
      settled: mark.settled.filter(rootIn),
      unsettled: mark.unsettled.filter(rootIn),
    };
  }

  /**
   * Send `ids` to a dismiss or restore `route` in batches the server accepts (`DISMISS_BATCH`), one
   * after another, adding the ids of each batch to `landed` as the server takes it: when one fails, the
   * batches before it stand on the server, and the caller takes back only the rest. Answers the ids
   * the server says it stored.
   *
   */
  async function writeInBatches(route: string, ids: string[], landed: string[]): Promise<string[]> {
    const stored = [];
    for (let start = 0; start < ids.length; start += DISMISS_BATCH) {
      const batch = ids.slice(start, start + DISMISS_BATCH);
      const part = await deps.inboxPost(route, { messages: batch });
      landed.push(...batch);
      stored.push(...((part && part.messages) || batch).map(String));
    }
    return stored;
  }

  return { markHeldDone, doneSettlesHere, settleHeldDone, heldDonePart, writeInBatches };
}

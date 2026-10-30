// Which replayed stream events still wait for a read to answer them (`#43 replay-burst-double-read`,
// `#203 incremental-refresh`; `#624 voice-page-modularity`, moved from the legacy script; plan step
// 14): how many timeline reads have gone on the wire, the newest of them that landed, the newest that
// could answer an edit or a removal, the count when the live stream last attached, and each waiting
// event with the count as it stood when the server had it. A read begun after that answers for it once
// it lands: one that failed saw nothing. Starting the reads the waiting events need stays with the page.

import { isMutationKey } from "./freshness.ts";

/** What a landed read carried, as far as answering replayed events needs it; null for a read that failed. */
export interface LandedPage {
  readonly messages?: readonly { readonly id: unknown }[];
  readonly threads?: readonly { readonly root?: { readonly id: unknown } | null }[];
  readonly delta?: { readonly complete?: boolean } | null;
}

/** The owner's read surface and named commands. */
export interface ReplayLedger {
  /** How many timeline reads have gone on the wire. */
  started(): number;
  /** One more goes on the wire now: its number. */
  start(): number;
  /** The count when the live stream last attached. */
  attachedAfter(): number;
  /** The live stream attached now. */
  noteAttached(): void;
  /**
   * Event `key`, which arrived after read `arrivedAfter`, from `channel`: whether a landed read already
   * answers for it, and otherwise it waits. An edit or removal is answered only by a read that carries
   * them: a newest page, or a complete delta.
   */
  awaitRead(key: string, channel: string, arrivedAfter: number): boolean;
  /**
   * Read number `read` landed with `page`, or with null if it failed, while `channel` is on screen:
   * forget the events it answers for. A read that started after an event arrived answers for it even
   * when its page lacks it; a read that failed answers for nothing.
   */
  settle(read: number, page: LandedPage | null, channel: string): void;
  /** How many events wait. */
  waiting(): number;
  /** Whether an edit or removal from the replay tail is waiting for a read to answer it. */
  mutationAwaits(): boolean;
}

/** No read on the wire, none landed, nothing waiting. */
export function createReplayLedger(): ReplayLedger {
  let timelineReadsStarted = 0;
  let timelineReadsLanded = 0;
  // The newest of them that could answer an edit or a removal: a newest page or a complete delta.
  let mutationReadsLanded = 0;
  let liveAttachReads = 0;
  const replaysAwaitingRead = new Map<string, { channel: string; arrivedAfter: number }>();
  return {
    started: () => timelineReadsStarted,
    start: () => ++timelineReadsStarted,
    attachedAfter: () => liveAttachReads,
    noteAttached() {
      liveAttachReads = timelineReadsStarted;
    },
    awaitRead(key, channel, arrivedAfter) {
      if ((isMutationKey(key) ? mutationReadsLanded : timelineReadsLanded) > arrivedAfter) return true;
      replaysAwaitingRead.set(key, { channel, arrivedAfter });
      return false;
    },
    settle(read, page, channel) {
      // A delta answers for what it carries: creations always, edits and removals only when complete.
      const answersMutations = Boolean(page) && (!page!.delta || page!.delta.complete === true);
      if (page) timelineReadsLanded = Math.max(timelineReadsLanded, read);
      if (answersMutations) mutationReadsLanded = Math.max(mutationReadsLanded, read);
      const onPage = new Set([
        ...((page && page.messages) || []).map((message) => String(message.id)),
        ...((page && page.threads) || []).map((thread) => String(thread.root && thread.root.id)),
      ]);
      for (const [key, waiting] of replaysAwaitingRead) {
        if (waiting.channel !== channel ||
            (page && (answersMutations || !isMutationKey(key)) &&
              (waiting.arrivedAfter < read || onPage.has(key)))) {
          replaysAwaitingRead.delete(key);
        }
      }
    },
    waiting: () => replaysAwaitingRead.size,
    mutationAwaits: () => [...replaysAwaitingRead.keys()].some(isMutationKey),
  };
}

// Which thread each send still in the outbox goes into, under the page's own names (`#624
// voice-page-modularity`, moved from the legacy script). The reckoning is core/outbox's
// (`createPendingThreads`), made in the script and handed in; these are the questions the
// page asks it, word for word as they were in the script.

/** The pending-threads reckoning, as this owner uses it (core/outbox `PendingThreads`). */
export interface PendingThreadsLike {
  threadOf(entry: unknown): string | null;
  into(thread: unknown): number;
  times(): Map<string, number>;
}

/** What asking after pending sends asks of the page. */
export interface PendingSendsDependencies {
  pendingThreads: PendingThreadsLike;
}

/** The page's questions after sends still in the outbox. */
export interface PendingSends {
  outgoingThreadOf(entry: unknown): string | null;
  pendingInto(thread: unknown): number;
  pendingThreadTimes(): Map<string, number>;
}

/** The pending sends' questions over `deps`. */
export function createPendingSends(deps: PendingSendsDependencies): PendingSends {
  /** The thread `entry` posts into in the channel on screen, or null for the main channel or one not known. */
  function outgoingThreadOf(entry: unknown): string | null {
    return deps.pendingThreads.threadOf(entry);
  }

  /** The sends in the outbox for the channel on screen that count in thread `thread`. */
  function pendingInto(thread: unknown): number {
    return deps.pendingThreads.into(thread);
  }

  /** The threads of the channel on screen sends in the outbox go into, each with when its newest was written. */
  function pendingThreadTimes(): Map<string, number> {
    return deps.pendingThreads.times();
  }

  return { outgoingThreadOf, pendingInto, pendingThreadTimes };
}

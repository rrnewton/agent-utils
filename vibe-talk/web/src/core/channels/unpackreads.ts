// The reads unpacking owes the Threads view's threads (`#404 threads-unpack-all`, `vt-thread-unpack-loop`;
// `#624 voice-page-modularity`, moved from the legacy script into core/channels): which threads a read is
// owed, queued, running, done or failed for, so each is read once; the reply count each done read's
// thread had when it landed, so a thread is read again only once it has grown past that; and starting
// queued reads, `UNPACK_READS_AT_ONCE` at a time. Watching a block near the screen, the read itself and
// drawing what it brought are the page's.

/** How many threads unpacking may be reading at once: enough that scrolling never waits on one. */
const UNPACK_READS_AT_ONCE = 3;

/** A thread as unpacking reads it: its id, and its reply count where known. */
export interface UnpackedThread {
  readonly id: string;
  readonly count?: unknown;
}

/** What the reads ask of the page. */
export interface UnpackReadsDependencies {
  /** Ask the server for thread `id`'s newest page; rejects where it failed. */
  fetch(id: string): Promise<unknown>;
  /**
   * Fold what `fetch` brought for thread `id` into the store, in the same turn as the read ends: true where
   * it landed for the channel it was asked for, false where the reader had moved on.
   */
  land(id: string, fetched: unknown): boolean;
  /** A read has ended, landed, moved past or failed: draw again what it may have brought. */
  ended(id: string): void;
  /** Run the read, its failures caught quietly (`guardQuietly`). */
  guard(run: () => Promise<void>): void;
}

/** The reads' commands and questions. */
export interface UnpackReads {
  /**
   * Whether thread `choice` still owes unpacking a read: `replies`, the replies held, short of its count,
   * and `cover`, the thread's cover in the store, not saying it is all held.
   */
  owed(choice: UnpackedThread, replies: number, cover: { readonly more?: unknown } | undefined): boolean;
  /** Queue thread `id`'s newest page for its block, `UNPACK_READS_AT_ONCE` at a time. */
  queue(id: string | null | undefined): void;
  /** Forget every read: they were the last channel's. Reads on the wire still end. */
  forget(): void;
  /** Where thread `id`'s read stands: "queued", "reading", "done", "failed", or undefined with none. */
  state(id: string): string | undefined;
}

/** No read owed, queued or running. */
export function createUnpackReads(deps: UnpackReadsDependencies): UnpackReads {
  /** Threads an unpack read is owed, running, done or failed for, so each is read once. */
  const unpackReads = new Map<string, string>();

  /**
   * `vt-thread-unpack-loop`. The reply count each done unpack read's thread had when the read landed, by
   * thread: a thread is read again only once its count has grown past that. A thread longer than one
   * page never holds all its replies, so "fewer held than its count" alone asked for its newest page
   * again every time its rows were drawn, and drawing them is what a landed read does.
   */
  const unpackReadCounts = new Map<string, unknown>();

  /** Reads under way for unpacked blocks: `UNPACK_READS_AT_ONCE` at most. */
  let unpackReadsRunning = 0;

  /** Start queued unpack reads while fewer than `UNPACK_READS_AT_ONCE` are on the wire. */
  function pumpUnpackReads(): void {
    for (const [id, state] of unpackReads) {
      if (unpackReadsRunning >= UNPACK_READS_AT_ONCE) return;
      if (state !== "queued") continue;
      unpackReads.set(id, "reading");
      unpackReadsRunning += 1;
      deps.guard(() => readUnpackThread(id));
    }
  }

  /** One thread's newest page, folded into the store, its block drawn again from what it brought. */
  async function readUnpackThread(id: string): Promise<void> {
    try {
      const fetched = await deps.fetch(id);
      if (!deps.land(id, fetched)) {
        unpackReads.delete(id);
        unpackReadCounts.delete(id);
        return;
      }
      unpackReads.set(id, "done");
    } catch (_error) {
      unpackReads.set(id, "failed");
    } finally {
      unpackReadsRunning -= 1;
      deps.ended(id);
      pumpUnpackReads();
    }
  }

  return {
    owed(choice, replies, cover) {
      const state = unpackReads.get(choice.id);
      if (state === "queued" || state === "reading" || state === "failed") return false;
      if (state === "done") {
        // Read once, and again only when the thread has grown past what that read brought: its count
        // above the one it had when the read landed (`unpackReadCounts`), as well as above what is held.
        if (!unpackReadCounts.has(choice.id)) unpackReadCounts.set(choice.id, choice.count);
        const landed = unpackReadCounts.get(choice.id);
        // A read that landed knowing no count leaves one learnt since to say what is owed.
        if (typeof choice.count === "number" && replies < choice.count &&
            (typeof landed !== "number" || choice.count > landed)) {
          unpackReads.delete(choice.id);
          unpackReadCounts.delete(choice.id);
        } else return false;
      }
      if (cover && !cover.more) return false;
      if (typeof choice.count === "number") return replies < choice.count;
      return !cover && replies === 0;
    },
    queue(id) {
      if (!id || unpackReads.has(id)) return;
      unpackReads.set(id, "queued");
      pumpUnpackReads();
    },
    forget() {
      unpackReads.clear();
      unpackReadCounts.clear();
    },
    state: (id) => unpackReads.get(id),
  };
}

// What the page and the tests use of it.
export { UNPACK_READS_AT_ONCE };

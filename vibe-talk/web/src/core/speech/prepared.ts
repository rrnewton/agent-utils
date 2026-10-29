// The read-aloud tickets the server has prepared (`#191 read-aloud-first-lag`, `#290
// speech-ticket-refresh`, `#376 read-aloud-blocks`; `#624 voice-page-modularity`, moved from the legacy
// script; plan step 13). Message id -> a URL that streams that message's audio, the blocks it will be
// read as, and when its ticket may have ended, on two clocks. Why both clocks, and why the URL carries
// its own authority, is told where the legacy script prepares them (`preparedSpeech`).

/** A moment on both clocks: the monotonic one, and the wall clock, in milliseconds. */
export interface SpeechMoment {
  readonly mono: number;
  readonly wall: number;
}

/** One block of a message as the server will read it, in drawing order (`#376 read-aloud-blocks`). */
export interface PreparedBlock {
  readonly kind: string;
  readonly spoken?: boolean;
  readonly [field: string]: unknown;
}

/** One prepared message. */
export interface PreparedEntry {
  readonly url: string;
  /** When the ticket may have ended, on both clocks: past either is past. */
  readonly until: SpeechMoment;
  /** The blocks the server will read the message as, when it is more than one; else null. */
  readonly blocks: readonly PreparedBlock[] | null;
}

/** What the owner asks of the page. */
export interface PreparedSpeechDependencies {
  /** Now, on both clocks (`speechDeadlineNow`). */
  now(): SpeechMoment;
}

/** The owner's read surface and named commands. */
export interface PreparedSpeech {
  /**
   * The prepared entry for message `id` while its ticket may still be good, else null: an entry past
   * its deadline is dropped, and the row is one with nothing prepared.
   */
  entry(id: unknown): PreparedEntry | null;
  /** What is kept for message `id`, past its deadline or not. */
  get(id: unknown): PreparedEntry | undefined;
  /** Whether anything is kept for message `id`. */
  has(id: unknown): boolean;
  /** How many messages have something kept. */
  size(): number;
  /** Message `id` is prepared as `entry`. */
  keep(id: string, entry: PreparedEntry): void;
  /** Message `id` is to be prepared again. */
  forget(id: string): void;
  /** Nothing is prepared: the pace changed, or the server forgot its tickets. */
  clear(): void;
}

/** How long `entry`'s ticket has left at `now`, on the nearer of its two deadlines. */
export function ticketLeftMs(entry: PreparedEntry, now: SpeechMoment): number {
  return Math.min(entry.until.mono - now.mono, entry.until.wall - now.wall);
}

/** An owner with nothing prepared. */
export function createPreparedSpeech(deps: PreparedSpeechDependencies): PreparedSpeech {
  const preparedSpeech = new Map<string, PreparedEntry>();
  return {
    entry(id) {
      const entry = preparedSpeech.get(String(id));
      if (!entry) return null;
      const now = deps.now();
      if (now.mono < entry.until.mono && now.wall < entry.until.wall) return entry;
      preparedSpeech.delete(String(id));
      return null;
    },
    get: (id) => preparedSpeech.get(String(id)),
    has: (id) => preparedSpeech.has(String(id)),
    size: () => preparedSpeech.size,
    keep(id, entry) {
      preparedSpeech.set(id, entry);
    },
    forget(id) {
      preparedSpeech.delete(id);
    },
    clear() {
      preparedSpeech.clear();
    },
  };
}

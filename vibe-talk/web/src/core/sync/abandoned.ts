// Reads the page gave up on (`remote-ops-audit` W7; `#624 voice-page-modularity`, moved from
// the legacy script into core/sync; mu-readcore C2): which view a read was for, whether its
// poll still waits, what a live event does to its hold, and whether its late answer may still
// be used. The holds themselves are holds.ts's (`createReadHolds`) and the live counts
// live.ts's (`createLiveEvents`); this owner only binds the page's facts to them. Landing a
// late answer — folding, drawing — is the page's (`landAbandonedTimelineRead` stays there).

import type { LiveEvents } from "./live.ts";
import type { ReadHold, ReadHolds } from "./holds.ts";

/** The store a read was read for, as far as `stands` reads it: which channel it holds. */
export interface AbandonedStore {
  readonly channel: string;
}

/**
 * One read given up on, as it was sent: the view it read (`holdKey`), its channel, when it
 * was sent, and the live counts then. `number` is how many timeline reads had been sent with
 * it (the order's `asked()`), on a channel with threads; `generation` answers for it on one
 * without. Callers keep their own further fields on the same object (the view, the column,
 * what it held); this owner reads only these, and reads `number`/`generation` as they stand
 * when the answer settles, not as they stood when `abandon` was called — a caller sets
 * `number` just after handing the read over, before the wait can be given up.
 */
export interface AbandonedRead {
  readonly key: string;
  readonly channel: string;
  readonly sentAt: number;
  readonly live: number;
  readonly news: number;
  readonly canon: AbandonedStore | null;
  number?: number;
  generation?: number;
}

/** A read's options, as far as asking whether it is held reads them: only its reasons. */
export interface HeldOptions {
  readonly reason?: string | readonly (string | undefined)[] | undefined;
}

/** What abandoning reads asks of the page, each asked when it is needed. */
export interface AbandonedReadsDependencies {
  /** Views held unpolled while a read given up on may still run (holds.ts, shared). */
  readonly holds: ReadHolds;
  /** Live events counted per channel (live.ts, shared). */
  readonly live: LiveEvents;
  /** Whether the channel on screen has threads (`providerFacts.threading()`). */
  threading(): boolean;
  /** Whether read the page hides is on (`readModes.hiding()`). */
  hiding(): boolean;
  /** The context of the view on screen (`channelContextKey()`). */
  contextKey(): string;
  /** The channel on screen, as reads name it (`String(selectedChannelId())`). */
  selectedChannel(): string;
  /** The store the page holds now (`channelStore.current()`). */
  currentStore(): AbandonedStore;
  /** How many timeline reads have been sent (`readOrder.asked()`). */
  asked(): number;
  /** The active column's read generation (`loadState().generation`). */
  generation(): number;
}

/** The abandoned reads' named commands. Their closures keep no `this`. */
export interface AbandonedReads {
  /** The view a read of the column being worked on is for: its context, and the to-do list apart. */
  holdKey(): string;
  /**
   * Whether a read of the view `key` names, with `options`, must not be sent now: only a read
   * whose every reason the hold puts off (holds.ts: only the poll).
   */
  held(key: string, options?: HeldOptions | null): boolean;
  /**
   * A live event for `channel` arrived. One out of the replay tail (`fromTail`) is old news;
   * any other says the service is answering, and ends the holds on the channel's views.
   * Either may be newer than the answer of a read given up on.
   */
  noteLiveEvent(channel: unknown, fromTail: boolean): void;
  /**
   * The read `read` describes was given up on with `answer` still on the wire: hold its view,
   * and use the answer if it lands and nothing newer has replaced it (`stands`), through
   * `land`. No hold when news arrived for the channel after the read was sent. The hold goes
   * when the answer settles, before `land` is called. `settled`, when given, is told after
   * the answer is used, with whether it landed and the answer if it landed unused.
   */
  abandon<T>(answer: Promise<T>, read: AbandonedRead, land: (payload: T) => void,
    settled?: ((landed: boolean, unused: T | null) => void) | null): void;
  /**
   * Whether the late answer of a read given up on may still be used: the same channel and
   * store, no live event for the channel since it was sent, and no read of the channel sent
   * after it. On a channel without threads, `generation` answers instead of `number`.
   */
  stands(read: AbandonedRead): boolean;
}

/** Abandoned reads over `deps`, over the shared holds and live counts. */
export function createAbandonedReads(deps: AbandonedReadsDependencies): AbandonedReads {
  const { holds, live } = deps;
  // A read of view `key`, in `channel`, sent at `sentAt`, was given up on: hold the view to
  // the server's bound. The hold set, for the read to take away when it answers.
  const holdView = (key: string, channel: string, sentAt: number): ReadHold =>
    holds.hold(key, channel, sentAt);
  function stands(read: AbandonedRead): boolean {
    if (deps.selectedChannel() !== read.channel) return false;
    if (live.seen(read.channel) !== read.live) return false;
    if (deps.threading()) {
      const current = deps.currentStore();
      return read.canon === current && current.channel === read.channel &&
        read.number === deps.asked();
    }
    return read.generation === deps.generation() && !deps.hiding();
  }
  return {
    holdKey() {
      return !deps.threading() && deps.hiding() ? `${deps.contextKey()} todo` : deps.contextKey();
    },
    held(key, options) {
      return holds.held(key, options?.reason);
    },
    noteLiveEvent(channel, fromTail) {
      if (channel === undefined || channel === null) return;
      const id = String(channel);
      live.noteEvent(id);
      if (fromTail) return;
      live.noteNews(id);
      holds.liftChannel(id);
    },
    abandon(answer, read, land, settled = null) {
      const answering = live.news(read.channel) !== read.news;
      const hold = answering ? null : holdView(read.key, read.channel, read.sentAt);
      const release = (): void => {
        holds.release(read.key, hold);
      };
      answer.then((payload) => {
        release();
        const ok = stands(read);
        if (ok) land(payload);
        if (settled) settled(true, ok ? null : payload);
      }, () => {
        release();
        if (settled) settled(false, null);
      });
    },
    stands,
  };
}

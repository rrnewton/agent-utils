// Views held unpolled while a read of them the page gave up on may still be running at the server
// (`remote-ops-audit` W7; `#624 voice-page-modularity`, moved from the legacy script; plan step 14).
// Why the poll waits, and why the read given up on is not aborted, is told where the legacy script
// gives a read up (`abandonRead`).

/**
 * The server's own bound on a read of a slow chat service, from when the read was sent: the longest a
 * view whose read the page gave up on goes unpolled.
 */
const SERVER_READ_BOUND_MS = 50000;

/**
 * The reasons a view's hold puts off: only the poll. Coming back to the page, a stream reset and the
 * replay tail's edits each stand for something the read given up on may not carry, and a live message
 * ends the hold before its own read is asked for.
 */
const HELD_REASONS: readonly string[] = ["poll"];

/** One view's hold: the channel it is in, and until when, by the clock. */
export interface ReadHold {
  readonly channel: string;
  readonly until: number;
}

/** What the owner asks of the page. */
export interface ReadHoldsDependencies {
  /** The wall clock, in milliseconds. */
  now(): number;
}

/** The owner's named commands. */
export interface ReadHolds {
  /** A read of view `key`, in `channel`, sent at `sentAt`, was given up on: hold the view to the server's bound. */
  hold(key: string, channel: string, sentAt: number): ReadHold;
  /** That read answered: take away `hold`, if it is still the view's. */
  release(key: string, hold: ReadHold | null): void;
  /**
   * Whether a read of view `key` for `reason` must not be sent now: only a read whose every reason is
   * held, so a queued read that also stands for any other trigger goes. A hold past its bound ends.
   */
  held(key: string, reason: string | readonly (string | undefined)[] | undefined): boolean;
  /** Live news for `channel` says the service is answering: every hold on its views ends. */
  liftChannel(channel: string): void;
  /** Whether a view of `channel` is held still; holds past their bound end as they are found. */
  channelWaits(channel: string): boolean;
}

/** No view held. */
export function createReadHolds(deps: ReadHoldsDependencies): ReadHolds {
  /** By view, until when a read of it the page gave up on may still be running, and its channel. */
  const readHolds = new Map<string, ReadHold>();
  return {
    hold(key, channel, sentAt) {
      const hold = { channel, until: sentAt + SERVER_READ_BOUND_MS };
      readHolds.set(key, hold);
      return hold;
    },
    release(key, hold) {
      if (hold && readHolds.get(key) === hold) readHolds.delete(key);
    },
    held(key, reason) {
      const hold = readHolds.get(key);
      if (!hold) return false;
      if (deps.now() >= hold.until) {
        readHolds.delete(key);
        return false;
      }
      const reasons = Array.isArray(reason) ? reason as readonly (string | undefined)[] : [reason as string | undefined];
      return reasons.every((one) => typeof one === "string" && HELD_REASONS.includes(one));
    },
    liftChannel(channel) {
      for (const [key, hold] of [...readHolds]) {
        if (hold.channel === channel) readHolds.delete(key);
      }
    },
    channelWaits(channel) {
      const now = deps.now();
      let waits = false;
      for (const [key, hold] of [...readHolds]) {
        if (hold.channel !== channel) continue;
        if (now >= hold.until) readHolds.delete(key);
        else waits = true;
      }
      return waits;
    },
  };
}

export { SERVER_READ_BOUND_MS };

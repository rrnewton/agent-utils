// Reads one at a time (`#624 voice-page-modularity`, moved from the legacy script into core/sync): the
// channel a read of some kind is on the wire for, or none, so that a second read of that kind waits for it
// (the thread directory, the counts read of All, the read that places what the stream left unplaced); and
// how many messages the stream has left unplaced in this page, the clock a placing read is timed against
// (`unplacedSince`). Each reading, and what waits for what, are the page's.

/** One kind of read, on the wire for one channel at a time. */
export interface ChannelInFlight {
  /** The channel the read on the wire is for, or null. */
  channel(): string | null;
  /** Whether a read is on the wire. */
  busy(): boolean;
  /** Whether the read on the wire is for `channel`. */
  isFor(channel: string): boolean;
  /** A read for `channel` goes on the wire. */
  begin(channel: string): void;
  /** The read on the wire has ended; with `channel`, only where it was that channel's. */
  end(channel?: string): void;
}

/** None on the wire. */
export function createChannelInFlight(): ChannelInFlight {
  let inFlight: string | null = null;
  return {
    channel: () => inFlight,
    busy: () => inFlight !== null,
    isFor: (channel) => inFlight === channel,
    begin(channel) {
      inFlight = channel;
    },
    end(channel) {
      if (channel === undefined || inFlight === channel) inFlight = null;
    },
  };
}

/** How many messages the stream has left unplaced in this page, in any channel. */
export interface ArrivalCount {
  count(): number;
  /** Another arrived unplaced: the count now. */
  next(): number;
}

/** None yet. */
export function createArrivalCount(): ArrivalCount {
  let unplacedArrivals = 0;
  return {
    count: () => unplacedArrivals,
    next: () => {
      unplacedArrivals += 1;
      return unplacedArrivals;
    },
  };
}

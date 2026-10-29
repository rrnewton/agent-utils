// Pinned messages (`#206 pin-message`; `#624 voice-page-modularity`, moved from the legacy script; plan
// step 11): the server's list for the channel on screen, with whatever this page has asked to change
// and not yet heard back about laid over it, so a tap shows at once and a list read meanwhile cannot
// take it back. Private to the owner, reached by its named commands; the reads and writes themselves,
// and what is drawn and said, stay with the page.

import type { Pin, PinsResponse } from "../../contract/index.ts";

/** One read of a channel's list in flight, and the newest revision a channel read noted meanwhile. */
export interface PinsRead {
  readonly channel: string;
  noted: number | null;
}

/** The owner's named commands. */
export interface Pins {
  /** The channel the held list is for; "" before any list. */
  channel(): string;
  /** The revision the held list came with, or null until one has been read for its channel. */
  revision(): number | null;
  /** How many pins the server keeps in one channel, as it last said; 0 before it has. */
  limit(): number;
  /** Why the list could not be read; "" when it could. */
  problem(): string;
  /** Whether message `id` is pinned in channel `channel`. */
  isPinned(channel: string, id: unknown): boolean;
  /** The pin held for message `id`, if any. */
  held(id: string): Pin | undefined;
  /** Every pin held, by message id, in the order held. */
  entries(): [string, Pin][];
  /** How many pins are held. */
  size(): number;
  /** Forget the held list: another channel, or another credential. */
  forget(channel?: string): void;
  /** The read in flight, if any. */
  inFlight(): PinsRead | null;
  /** A read of `channel`'s list starts; answers it. */
  startRead(channel: string): PinsRead;
  /** Whether `read` is still the read in flight. */
  isCurrent(read: PinsRead): boolean;
  /** `read` ended: no read is in flight, if it was still the one. */
  endRead(read: PinsRead): void;
  /** A list the server sent, taken as the truth, with this page's unanswered changes laid over it. */
  apply(payload: PinsResponse): void;
  /** The list could not be read, for `problem`. */
  failed(problem: string): void;
  /** A change asked for and not yet answered: message `id` will be `pin`, or unpinned for null. */
  stage(id: string, pin: Pin | null): void;
  /** The server answered the change of message `id`: `pin`, or none; and the pins it `unpinned`. */
  answered(id: string, pin: Pin | null | undefined, unpinned: readonly string[]): void;
  /** A change of message `id` refused: what was held `before` it, back. */
  rollback(id: string, before: Pin | undefined): void;
}

/** An owner holding no list. */
export function createPins(): Pins {
  /** This channel's pins as the server listed them, by message id, with `pinsPending` laid over. */
  let pins = new Map<string, Pin>();
  let pinsChannel = "";
  let pinsRevision: number | null = null;
  let pinsLimit = 0;
  let pinsProblem = "";
  let pinsInFlight: PinsRead | null = null;
  /** Changes asked for and not yet answered: id -> the pin it will be, or null for an unpin. */
  const pinsPending = new Map<string, Pin | null>();
  return {
    channel: () => pinsChannel,
    revision: () => pinsRevision,
    limit: () => pinsLimit,
    problem: () => pinsProblem,
    isPinned: (channel, id) => pinsChannel === channel && pins.has(String(id)),
    held: (id) => pins.get(id),
    entries: () => [...pins],
    size: () => pins.size,
    forget(channel = "") {
      pins = new Map();
      pinsChannel = channel;
      pinsRevision = null;
      pinsProblem = "";
      pinsInFlight = null;
      pinsPending.clear();
    },
    inFlight: () => pinsInFlight,
    startRead(channel) {
      const read = { channel, noted: null };
      pinsInFlight = read;
      return read;
    },
    isCurrent: (read) => pinsInFlight === read,
    endRead(read) {
      if (pinsInFlight === read) pinsInFlight = null;
    },
    apply(payload) {
      pins = new Map(payload.pins.map((pin) => [String(pin.message_id), pin]));
      for (const [id, pending] of pinsPending) {
        if (pending) pins.set(id, pending);
        else pins.delete(id);
      }
      pinsRevision = payload.revision;
      pinsLimit = payload.limit;
      pinsProblem = "";
    },
    failed(problem) {
      pinsProblem = problem;
    },
    stage(id, pin) {
      pinsPending.set(id, pin);
      if (pin) pins.set(id, pin);
      else pins.delete(id);
    },
    answered(id, pin, unpinned) {
      pinsPending.delete(id);
      if (pin) pins.set(id, pin);
      else pins.delete(id);
      for (const gone of unpinned) pins.delete(String(gone));
    },
    rollback(id, before) {
      pinsPending.delete(id);
      if (before) pins.set(id, before);
      else pins.delete(id);
    },
  };
}

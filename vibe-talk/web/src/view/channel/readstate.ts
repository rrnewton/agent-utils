// What reads say about read state, and where each copy of a message stands in the order of
// reads (`#624 voice-page-modularity`, moved from the legacy script). Words and order are
// as they were in the script. The ledgers themselves are core's (annotations' read ledger,
// channels' copy order), made in the script and handed in.

import { noteReplyGapsIn } from "../../core/channels/index.ts";
import type { ChannelStore, CopyOrder } from "../../core/channels/index.ts";
import type { ReadLedger } from "../../core/annotations/index.ts";
import type { Message } from "../../contract/index.ts";

/** A page's say about read state, as `foldReadState` reads it. */
export interface ReadStatePage {
  dismissed?: unknown[];
  messages?: { id: unknown }[];
  reply_dismissals?: { messages: unknown[]; from: unknown } | null;
}

/** A page as the reply-gap count reads it (core/channels `GapPage`, by shape). */
type GapPage = Parameters<typeof noteReplyGapsIn>[1];

/** What folding read state and ordering copies asks of the page. */
export interface ReadStateDependencies {
  channelStore: ChannelStore;
  readLedger: ReadLedger;
  copyLedger: CopyOrder;
  readOrder: { sentNumberOf(payload: object): number };
  queueReadAllForCounts(): void;
}

/** Folding reads' read state and ordering the store's copies. */
export interface ReadState {
  atOrAfter(id: string, from: string): boolean;
  foldReadState(payload: ReadStatePage, view: string): boolean;
  noteReplyGaps(payload: unknown, view: string, older: boolean, delta: boolean, before: Map<string, Message>): void;
  readSpeaksFor(payload: unknown, id: unknown): boolean;
  sentEpochOf(payload: unknown): number;
  sentNumberOf(payload: unknown): number;
  orderCopy(copy: object, at: number): void;
  buildCopyOn(copy: object, from: object): void;
  orderCopiesRead(messages: readonly { id: unknown }[], number: number): void;
  copyOlderThan(copy: object, id: string, number: number, held: ReadonlyMap<string, object>, says?: boolean): boolean;
}

/** The read state's folding and copy ordering over `deps`. */
export function createReadState(deps: ReadStateDependencies): ReadState {
  /**
   * Whether message id `id` is at or after position `from`. Both are ids in the order the server keeps
   * them in, decimal numbers of any length (`MessageId::numeric`): longer is later, and at one length
   * the text compares as the number does. False for anything that is not such a number.
   */
  function atOrAfter(id: string, from: string): boolean {
    if (!/^[0-9]+$/.test(id) || !/^[0-9]+$/.test(from)) return false;
    const [a, b] = [id.replace(/^0+(?=.)/, ""), from.replace(/^0+(?=.)/, "")];
    return a.length !== b.length ? a.length > b.length : a >= b;
  }

  /**
   * What a page says about read state, into the store. `#222 unread-replies`.
   *
   * A page is the server's answer for every message on it, Done or not, whichever view it was read for
   * — so a read of All landing while the reader is in Main still tells the store which replies are
   * Done. Main's newest page and its deltas say more (`reply_dismissals`): every mark from a position
   * up, complete there, so every message the store holds from that position is Done exactly when it is
   * named, the replies Main never draws among them.
   *
   * Answers whether that changed anything the store held (`#226 page-energy-profile`).
   */
  function foldReadState(payload: ReadStatePage, view: string): boolean {
    let changed = false;
    const mark = (id: string, done: boolean): void => {
      if (deps.channelStore.current().dismissed.has(id) === done) return;
      if (done) deps.channelStore.current().dismissed.add(id);
      else deps.channelStore.current().dismissed.delete(id);
      changed = true;
    };
    const done = new Set((payload.dismissed || []).map(String));
    for (const message of payload.messages || []) {
      const id = String(message.id);
      if (!readSpeaksFor(payload, id)) continue;
      mark(id, done.has(id));
    }
    const marks = payload.reply_dismissals;
    if (view !== "main" || !marks) return changed;
    const named = new Set(marks.messages.map(String));
    const from = String(marks.from);
    for (const message of deps.channelStore.current().messages) {
      const id = String(message.id);
      if (!atOrAfter(id, from) || !readSpeaksFor(payload, id)) continue;
      mark(id, named.has(id));
    }
    return changed;
  }

  /**
   * What one page says of the replies the store lacks for being old, by thread (web/src/core/channels
   * `noteReplyGapsIn`, `#222 unread-replies`), into the selected channel's store; a newest read of
   * Main may then be followed by one of All, behind it (`deps.queueReadAllForCounts`).
   */
  function noteReplyGaps(payload: unknown, view: string, older: boolean, delta: boolean, before: Map<string, Message>): void {
    noteReplyGapsIn(deps.channelStore.current(), payload as GapPage, view, older, delta, before, deps.queueReadAllForCounts);
  }


  /** Whether `payload`'s read state for message `id` is newer than every local write to it. */
  function readSpeaksFor(payload: unknown, id: unknown): boolean {
    return deps.readLedger.speaksFor(payload as object, id);
  }

  /**
   * When `payload`'s read was sent, as the epoch then. Now for a payload no read sent — the store drawn
   * as a page, say — which holds the local state already.
   */
  function sentEpochOf(payload: unknown): number {
    return deps.readLedger.sentEpochOf(payload as object);
  }

  /**
   * Where `payload`'s read was sent in the order of every timeline read (`readOrder`). Now — after every
   * read sent so far — for a payload no read sent. `#308 live-reply-in-main`: of two newest pages of a
   * view, the one sent later is the newer, whichever lands first.
   */
  function sentNumberOf(payload: unknown): number {
    return deps.readOrder.sentNumberOf(payload as object);
  }

  /** Copy `copy`, the store's, stands at `at`, or later (`CopyOrder.orderCopy`). */
  function orderCopy(copy: object, at: number): void {
    deps.copyLedger.orderCopy(copy, at);
  }

  /** Copy `copy`, put together here from `from`, stands where `from` stood (`CopyOrder.buildCopyOn`). */
  function buildCopyOn(copy: object, from: object): void {
    deps.copyLedger.buildCopyOn(copy, from);
  }

  /** The store's copies of `messages` stand where the read numbered `number` was sent (`CopyOrder.orderCopiesRead`). */
  function orderCopiesRead(messages: readonly { id: unknown }[], number: number): void {
    deps.copyLedger.orderCopiesRead(deps.channelStore.current(), messages, number);
  }

  /** Whether the store's `copy` of message `id` is older than the read numbered `number` (`CopyOrder.copyOlderThan`). */
  function copyOlderThan(copy: object, id: string, number: number, held: ReadonlyMap<string, object>, says = false): boolean {
    return deps.copyLedger.copyOlderThan(copy, id, number, held, says);
  }

  return {
    atOrAfter, foldReadState, noteReplyGaps, readSpeaksFor, sentEpochOf, sentNumberOf,
    orderCopy, buildCopyOn, orderCopiesRead, copyOlderThan,
  };
}

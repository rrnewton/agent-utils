// Where each copy of a message in a channel's store stands in the order of reads (`#310
// stale-live-read-flicker`; `#624 voice-page-modularity`, moved from the legacy script; plan step
// 10): the copy-order ledger, its facts private, kept by object identity so a copy replaced in the
// store takes its facts with it.
//
// BY COPY of a message in the store, where it stands in that same order (`sentNumberOf`): a copy a
// read brought, or found word for word, stands where that read was sent, and a message from the
// stream or the reader's own send where it went into the store, after every read sent by then and
// before the next. A copy put together here from the store's own — a count, a thread record, the
// stream's copy of a message the store held — stands where that one stood (`buildCopyOn`). A page
// landing after a copy went in takes the message out only where that copy is the older
// (`copyOlderThan`, `pageFold`), and takes its own copy over it by the same test, except where the
// copy says the message as it was posted (`markPosted`).
//
// BUILT ON, NOT WHERE IT WENT INTO THE STORE. A copy put together here from the store's copy of the
// message — a root's count moved on by a live reply (`foldLiveMessage`), a thread record from the
// stream (`takeThreadRecord`) or from an answer of All that is not folded (`placeFromPage`), the
// stream's copy of a message the store held — says nothing newer than that copy of whether the
// message is there, nor of what it says, the stream carrying no edits (`placeFromEdit`); a count or a
// thread record is all that changed. Stamped as new as the moment it went in, it made a newest page
// sent before then refuse its own copy of the message, though that page, answered later, carried the
// newer text: a root's edit, if a live reply had moved its count on, or a bridge's finished answer,
// if the stream had placed it. The polls after, deltas from that page's cursor, already past the
// edit, never brought it either, and the old text stayed shown and saved until the next newest page,
// up to `TIMELINE_RECONCILE_MS` later on a server whose deltas carry edits. And a root removed
// upstream stayed as long, behind a page sent before a live reply moved its count on, which then did
// not take it out.
//
// POSTED: the copies in the store that say a message as it was posted — the stream's copy of a
// message (the stream carries no edits, and a copy it replays is the message as it was published),
// the reader's own send as the server took it, and a copy put together on one of those. A read that
// carries the message has that or newer, wherever it stands in the order, so a page that carries the
// message takes its own copy over one of these (`pageFold`, `placeFromPage`). A message the stream
// delivered while a newest page was out, and a bot then edited from its placeholder to its answer,
// kept the placeholder, shown and saved, until the next newest page fifteen minutes on.

import type { ChannelCanon } from "./canon.ts";

/** A copy of a message, known to the ledger only by identity. */
type Copy = object;

/** The ledger's named commands. */
export interface CopyOrder {
  /** Copy `copy` stands exactly at `at`: a copy a read brought, where that read was sent. */
  place(copy: Copy, at: number): void;
  /**
   * Copy `copy`, the store's, stands at `at`, or later: a read that brings a copy the store holds word
   * for word keeps the store's object, which is then as new as that read.
   */
  orderCopy(copy: Copy, at: number): void;
  /**
   * Copy `copy`, put together here from `from`, the store's copy of the message, stands where `from`
   * stood, and says the message as it was posted if `from` does.
   */
  buildCopyOn(copy: Copy, from: Copy): void;
  /** Copy `copy` says the message as it was posted. */
  markPosted(copy: Copy): void;
  /** Whether copy `copy` says the message as it was posted. */
  isPosted(copy: Copy): boolean;
  /**
   * The copies in `canon` of `messages`, which the read numbered `number` brought, or found word for
   * word, stand where that read was sent, or later: one put together here as well, which the read
   * speaks for from now on. One walk over the store.
   */
  orderCopiesRead(canon: ChannelCanon, messages: readonly { id: unknown }[], number: number): void;
  /**
   * Whether `copy`, the store's copy of message `id`, is older than the read numbered `number`, sent
   * when the store's copies were `held` (`heldNow`): it stands before that read, or the copy it was
   * built on does. A copy with no place — one saved on the device, which every read since the reload
   * is newer than — is older exactly when the store held it, or a copy built on it, as the read was
   * sent. `says`: older in what it says, for a page that carries the message and would take its own
   * copy over it — which a copy saying the message as it was posted always is.
   *
   * NOT "UNCHANGED SINCE IT WAS SENT". A copy replaced while the read was out may have been replaced by
   * an OLDER read: the read of All for Main's counts, sent first and answered after an edit, landing
   * while Main's newer newest page was out. Taken for newer, it made that page refuse its own copy of
   * the next edit, and its removal of the message, until the next newest page, up to
   * `TIMELINE_RECONCILE_MS` later on a server whose deltas carry edits and deletions.
   */
  copyOlderThan(copy: Copy, id: string, number: number, held: ReadonlyMap<string, Copy>, says?: boolean): boolean;
}

/** An empty ledger. */
export function createCopyOrder(): CopyOrder {
  const copyOrder = new WeakMap<Copy, number>();
  const copyBuiltOn = new WeakMap<Copy, Copy>();
  const postedCopies = new WeakSet<Copy>();
  function orderCopy(copy: Copy, at: number): void {
    const was = copyOrder.get(copy);
    if (was === undefined || was < at) copyOrder.set(copy, at);
  }
  return {
    place(copy, at) {
      copyOrder.set(copy, at);
    },
    orderCopy,
    buildCopyOn(copy, from) {
      copyBuiltOn.set(copy, copyBuiltOn.get(from) ?? from);
      if (postedCopies.has(from)) postedCopies.add(copy);
    },
    markPosted(copy) {
      postedCopies.add(copy);
    },
    isPosted: (copy) => postedCopies.has(copy),
    orderCopiesRead(canon, messages, number) {
      if (messages.length === 0) return;
      const ids = new Set(messages.map((message) => String(message.id)));
      for (const copy of canon.messages) {
        if (!ids.has(String(copy.id))) continue;
        const from = copyBuiltOn.get(copy);
        if (from) {
          copyBuiltOn.delete(copy);
          const at = copyOrder.get(from);
          if (at !== undefined) orderCopy(copy, at);
        }
        orderCopy(copy, number);
      }
    },
    copyOlderThan(copy, id, number, held, says = false) {
      if (says && postedCopies.has(copy)) return true;
      const from = copyBuiltOn.get(copy) ?? copy;
      const at = copyOrder.get(from);
      if (at !== undefined) return at < number;
      const was = held.get(id);
      return was !== undefined && (copyBuiltOn.get(was) ?? was) === from;
    },
  };
}

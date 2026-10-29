// Which reads still speak for a message's read state (`#222 unread-replies`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 11).
//
// DONE HERE RACES THE READS ON THE WIRE. A read sent before a Done or a put back reached the server
// answers with the read state from before it, and folding that would undo the reader's own act until
// the next read: a reply dealt with in its thread counted as unread on Main again. So each local write
// is remembered by message id until a read sent after it settled can speak for that message: while the
// write is on its way no read does, and once it is settled only a read sent after. The epoch moves at
// every write's start and end; a read records it when sent.

/** The ledger's named commands. */
export interface ReadLedger {
  /** The epoch now. */
  epoch(): number;
  /** Move the epoch on, as every write's start and end does; answers the new one. */
  bump(): number;
  /** Read `payload` was sent at epoch `sent`. */
  noteSent(payload: object, sent: number): void;
  /**
   * When `payload`'s read was sent, as the epoch then. Now for a payload no read sent — the store drawn
   * as a page, say — which holds the local state already.
   */
  sentEpochOf(payload: object): number;
  /** Whether `payload`'s read state for message `id` is newer than every local write to it. */
  speaksFor(payload: object, id: unknown): boolean;
  /** Whether this page has written message `id`'s read state since it was loaded. */
  written(id: string): boolean;
  /** Every message this page has written the read state of since it was loaded, in the order first written. */
  writtenIds(): IterableIterator<string>;
  /** A write of `ids`' read state starts: the epoch moves, and each has one more write on its way. */
  writeStarted(ids: readonly string[]): void;
  /** A write of `ids`' read state ended: the epoch moves, and each settles there once none is left on its way. */
  writeSettled(ids: readonly string[]): void;
  /** Message `id`'s read state is the page's own as of `epoch`, with no write on its way. */
  seed(id: string, epoch: number): void;
}

/** A ledger at epoch 0, with nothing written. */
export function createReadLedger(): ReadLedger {
  let readStateEpoch = 0;
  const localReadWrites = new Map<string, { pending: number; from: number }>();
  const readSentEpochs = new WeakMap<object, number>();
  const sentEpochOf = (payload: object): number => {
    const sent = readSentEpochs.get(payload);
    return typeof sent === "number" ? sent : readStateEpoch;
  };
  return {
    epoch: () => readStateEpoch,
    bump: () => ++readStateEpoch,
    noteSent(payload, sent) {
      readSentEpochs.set(payload, sent);
    },
    sentEpochOf,
    speaksFor(payload, id) {
      const write = localReadWrites.get(String(id));
      if (!write) return true;
      if (write.pending > 0) return false;
      return sentEpochOf(payload) >= write.from;
    },
    written: (id) => localReadWrites.has(id),
    writtenIds: () => localReadWrites.keys(),
    writeStarted(ids) {
      readStateEpoch += 1;
      for (const id of ids) {
        const write = localReadWrites.get(id) || { pending: 0, from: 0 };
        write.pending += 1;
        localReadWrites.set(id, write);
      }
    },
    writeSettled(ids) {
      readStateEpoch += 1;
      for (const id of ids) {
        const write = localReadWrites.get(id);
        if (!write) continue;
        write.pending = Math.max(0, write.pending - 1);
        if (write.pending === 0) write.from = readStateEpoch;
      }
    },
    seed(id, epoch) {
      localReadWrites.set(id, { pending: 0, from: epoch });
    },
  };
}

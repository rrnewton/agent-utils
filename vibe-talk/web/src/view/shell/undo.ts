// The order undoable acts were done in (`#443 undo-key-general`; `#624 voice-page-modularity`, moved from the
// legacy script into view/shell): every offer an Undo chip makes — a Done, a send dismissed from the outbox, a
// bookmark taken away — is stamped from this as it is made, and a stamp is never reused, so two offers up at
// once always have an order, and the one undo key takes the newest back first. Gathering the offers is the
// page's.

/** The stamps' one command. */
export interface UndoStamps {
  /** The next undo stamp: later acts stamp higher. */
  next(): number;
}

/** No act stamped yet. */
export function createUndoStamps(): UndoStamps {
  let undoSeq = 0;
  return {
    next() {
      undoSeq += 1;
      return undoSeq;
    },
  };
}

// The order timeline reads were sent in (`remote-ops-audit` W7, `#308 live-reply-in-main`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 14): how many the page has sent, of
// any view, by any path, and where each answer's read stood among them. Of two newest pages of a view,
// the one sent later is the newer, whichever lands first; and a read given up on can tell whether any
// was sent after it.

/** The owner's read surface and named commands. */
export interface ReadOrder {
  /** How many timeline reads the page has sent. */
  asked(): number;
  /** A timeline read is being sent now: counted as it is sent, before its answer is awaited. */
  noteAsked(): void;
  /** `payload` answers the read sent as number `number` (`asked()` as it was sent). */
  noteSent(payload: object, number: number): void;
  /** Where `payload`'s read was sent among every read; for a payload no read sent, after every one so far. */
  sentNumberOf(payload: object): number;
  /** Where something that goes into the store now, other than from a read, stands: between reads. */
  now(): number;
}

/** No read sent yet. */
export function createReadOrder(): ReadOrder {
  let timelineAnswersAsked = 0;
  /** By timeline answer, its read's place in the order reads were sent. */
  const readSentNumbers = new WeakMap<object, number>();
  return {
    asked: () => timelineAnswersAsked,
    noteAsked() {
      timelineAnswersAsked += 1;
    },
    noteSent(payload, number) {
      readSentNumbers.set(payload, number);
    },
    sentNumberOf(payload) {
      const number = readSentNumbers.get(payload);
      return typeof number === "number" ? number : timelineAnswersAsked;
    },
    now: () => timelineAnswersAsked + 0.5,
  };
}

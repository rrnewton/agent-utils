// What the thread list's cards were last drawn from (`#281 view-keys`, `#532 shift-o-latency`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): the signature of the last pass,
// so a pass that changes nothing draws nothing; the elements it drew, which the next pass under the same
// whole keeps where it can; how many cards there are and how many the search and the read mode leave; and
// whether the cards are being opened. Building the cards, and what each signature says, are the page's
// (`renderSideCards`, `filterSideCards`).

/**
 * What the last pass over the cards drew: the whole list's signature, each thread's card and block by its
 * id, and each run of Main's messages by its messages.
 */
export interface KeptCards<Thread, Run> {
  readonly whole: string;
  readonly threads: Map<string, Thread>;
  readonly main: Map<string, Run>;
}

/** The cards' commands and questions. */
export interface CardsDrawn<Thread, Run> {
  /** Whether a pass with `signature` would draw something: it is not what the cards were last drawn from. */
  changed(signature: string): boolean;
  /**
   * A pass draws from `signature` under `whole`: whether it may keep what the last pass drew. Kept only from
   * a pass under the same whole, and never where the cards were asked to be drawn anew (`forget`: something
   * the signatures do not say has changed).
   */
  redraw(signature: string, whole: string): boolean;
  /** The cards are to be drawn anew at the next pass, whatever its signature. */
  forget(): void;
  /** What the last pass drew. */
  kept(): KeptCards<Thread, Run>;
  /** The pass has drawn `drawn`. */
  keep(drawn: KeptCards<Thread, Run>): void;
  /** The cards there are, and how many the search and the read mode leave, as the last count found. */
  count(): { readonly cards: number; readonly shown: number };
  setCount(cards: number, shown: number): void;
  /** True while the cards are being opened, which places the reader once they are drawn. */
  opening(): boolean;
  /** Open the cards with `fn`, and stop opening however it returns. */
  whileOpening(fn: () => void): void;
}

/** Nothing drawn, nothing counted. */
export function createCardsDrawn<Thread, Run>(): CardsDrawn<Thread, Run> {
  let drawnFrom = "";
  let keptCards: KeptCards<Thread, Run> = { whole: "", threads: new Map(), main: new Map() };
  let counted = { cards: 0, shown: 0 };
  let cardsOpening = false;

  return {
    changed: (signature) => signature !== drawnFrom,
    redraw(signature, whole) {
      const keep = drawnFrom !== "" && keptCards.whole === whole;
      drawnFrom = signature;
      return keep;
    },
    forget() {
      drawnFrom = "";
    },
    kept: () => keptCards,
    keep(drawn) {
      keptCards = drawn;
    },
    count: () => counted,
    setCount(cards, shown) {
      counted = { cards, shown };
    },
    opening: () => cardsOpening,
    whileOpening(fn) {
      cardsOpening = true;
      try {
        fn();
      } finally {
        cardsOpening = false;
      }
    },
  };
}

// The unmatched runs the reader has opened (`#439 search-unmatched-separators`; `#624 voice-page-modularity`,
// moved from the legacy script into core/search): in each view, as the ids of their messages, and among the
// thread cards, as thread ids and the ids of Main's messages among them. Held under the searches that chose
// the rows and forgotten the moment any filter changes: an opened run is an act on THIS search's list, and a
// new word typed is a new list. Leaving the state behind a mismatched key would bring its runs back when the
// reader typed their way back to the same words, never having opened them under this search. What the keys
// say, and drawing the runs, are the page's (`openedGapIds`, `openedCardGaps`).

/** The opened runs among the cards. */
export interface OpenedCardRuns {
  readonly key: string;
  readonly threads: Set<string>;
  readonly ids: Set<string>;
}

/** The opened runs' questions, each making the set it is asked for where `create` says to. */
export interface OpenedGaps {
  /**
   * View `view`'s set of opened messages under `searches`, or null when it has none and `create` does not
   * ask for one. Every view's sets are forgotten as `searches` changes.
   */
  ids(searches: string, view: string, create: boolean): Set<string> | null;
  /**
   * The opened card runs under `key` (the searches, and the channel), or null when there are none and
   * `create` does not ask for them. Those under another key are forgotten.
   */
  cards(key: string, create: boolean): OpenedCardRuns | null;
}

/** Nothing opened. */
export function createOpenedGaps(): OpenedGaps {
  /** The opened runs of every view, under the searches as they stood when each was opened. */
  const opened = new Map<string, Set<string>>();
  /** The searches the opened runs belong to: a change of any filter forgets them all. */
  let openedFor = "";
  let openedCards: OpenedCardRuns | null = null;

  return {
    ids(searches, view, create) {
      if (searches !== openedFor) {
        opened.clear();
        openedFor = searches;
      }
      const held = opened.get(view) || null;
      if (held || !create) return held;
      const made = new Set<string>();
      opened.set(view, made);
      return made;
    },
    cards(key, create) {
      if (openedCards && openedCards.key !== key) openedCards = null;
      if (!openedCards) {
        if (!create) return null;
        openedCards = { key, threads: new Set(), ids: new Set() };
      }
      return openedCards;
    },
  };
}

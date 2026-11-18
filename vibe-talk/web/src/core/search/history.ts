// The search terms the reader has run before (`#1816 vibe-talk-search-history-up-arrow`): newest
// first, bounded, de-duplicated, kept on the device so a reload recalls them too. Up and Down in
// the field walk the list like shell history: Up from what is typed holds that text as the draft,
// and Down past the newest term gives the draft back word for word. The field and its keys are
// the page's (`view/channel/filterchips.ts`); this owner holds only the list and where the walk
// stands in it.

import type { StoragePort } from "../../ports/index.ts";

/** Where the device keeps the terms. */
const SEARCH_HISTORY_KEY = "vibe-talk.search-history";

/** How many terms are kept; past it the oldest are let go. */
const SEARCH_HISTORY_LIMIT = 50;

/** What the history asks of the page. */
export interface SearchHistoryDependencies {
  readonly storage: StoragePort;
  /** Where the device keeps the terms; the search history's key unless said. */
  readonly key?: string;
  /** How many terms are kept; `SEARCH_HISTORY_LIMIT` unless said. */
  readonly limit?: number;
}

/** The history's commands and questions. */
export interface SearchHistory {
  /** The terms held, newest first. */
  entries(): string[];
  /**
   * A search ran with `term`: it stands newest now, once, however it stood before. Blank words
   * are no search and are never held. The walk is over: the next Up starts from the newest.
   */
  remember(term: string): void;
  /**
   * Up: the term one step older than where the walk stands, or null when there is none. The
   * first Up of a walk holds `current` (what the field holds now) as the draft Down returns to.
   */
  older(current: string): string | null;
  /**
   * Down: the term one step newer, the draft itself once the walk steps past the newest term,
   * or null when no walk stands (Down then is the caret's own key, untouched).
   */
  newer(): string | null;
  /** The walk is over (the reader typed, the bar closed, a search ran): the next Up starts anew. */
  reset(): void;
}

/** No terms remembered, no walk standing. The device's list is read on creation. */
export function createSearchHistory(deps: SearchHistoryDependencies): SearchHistory {
  const storageKey = deps.key || SEARCH_HISTORY_KEY;
  const limit = Math.max(1, deps.limit ?? SEARCH_HISTORY_LIMIT);
  let terms: string[] = [];
  /** Where the walk stands in `terms`, or -1 while no walk stands. */
  let at = -1;
  /** What the field held when the walk began; Down past the newest term gives it back. */
  let draft = "";

  try {
    const saved: unknown = JSON.parse(deps.storage.getItem(storageKey) || "[]");
    if (Array.isArray(saved)) {
      for (const value of saved) {
        if (typeof value === "string" && value.trim() && !terms.includes(value)) terms.push(value);
        if (terms.length >= limit) break;
      }
    }
  } catch (_error) {
    // Storage may be absent or hold an interrupted write; the history then lives in this page.
  }

  function save(): void {
    try {
      deps.storage.setItem(storageKey, JSON.stringify(terms));
    } catch (_error) {
      // A refused write loses the terms at the next reload, never the search in this one.
    }
  }

  return {
    entries: () => [...terms],
    remember(term) {
      const text = term.trim();
      at = -1;
      if (!text) return;
      terms = [text, ...terms.filter((held) => held !== text)].slice(0, limit);
      save();
    },
    older(current) {
      if (!terms.length) return null;
      if (at < 0) {
        draft = current;
        at = 0;
      } else if (at < terms.length - 1) {
        at += 1;
      }
      return terms[at] ?? null;
    },
    newer() {
      if (at < 0) return null;
      at -= 1;
      if (at < 0) return draft;
      return terms[at] ?? null;
    },
    reset() {
      at = -1;
    },
  };
}

// What the page and the tests use of it.
export { SEARCH_HISTORY_KEY, SEARCH_HISTORY_LIMIT };

// The search field's state (`#129 message-search`, `#249 search-highlight`; `#624 voice-page-modularity`,
// moved from the legacy script into core/search): whether the field is open, what is in it, the text whose
// hits are drawn, and marking those hits once the typing pauses. Filtering the rows, drawing the marks and
// the field itself are the page's; why it filters rather than fetches is told where the legacy script
// opens the section.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long after the last keystroke the hits are marked. */
const SEARCH_MARK_DELAY_MS = 120;

/** What the field asks of the page. */
export interface SearchFieldDependencies {
  readonly clock: ClockPort;
  /**
   * Mark the hits of what is typed now: `apply` takes the query as the marked text, and the page draws
   * both columns' lists again around it, holding the reader where they were.
   */
  redraw(apply: () => void): void;
}

/** The field's commands and questions. */
export interface SearchField {
  /** Whether the field is open. Closing it takes the filter off; it is not a separate switch. */
  open(): boolean;
  setOpen(open: boolean): void;
  /** What is in the field. Read from here rather than from the input, so the filter has one source. */
  query(): string;
  setQuery(text: string): void;
  /**
   * The text whose hits are drawn (`#249 search-highlight`): the query once the typing pauses.
   *
   * The rows follow the field at every keystroke, as they always have; marking where each hit is
   * splits text in every row shown, and done at every keystroke a word typed at speed is marked and
   * unmarked letter by letter in every match. So the marks follow `SEARCH_MARK_DELAY_MS` after the last
   * key — at once on Enter, and at once when the field is cleared, so no mark outlives its words.
   */
  marked(): string;
  setMarked(text: string): void;
  /** Mark the hits of what is typed once the typing pauses: each key puts it off again. */
  markSoon(): void;
  /**
   * Mark the hits of what is typed now: a redraw of both columns' lists that chooses the same rows, so
   * not a filter change.
   */
  markNow(): void;
}

/** Closed and empty. */
export function createSearchField(deps: SearchFieldDependencies): SearchField {
  let searchOpen = false;
  let searchQuery = "";
  let markedQuery = "";
  /** The marking a keystroke put off, or null. */
  let searchMarkTimer: TimerHandle | null = null;

  function markSearchNow(): void {
    if (searchMarkTimer !== null) deps.clock.clearTimeout(searchMarkTimer);
    searchMarkTimer = null;
    if (markedQuery === searchQuery) return;
    deps.redraw(() => {
      markedQuery = searchQuery;
    });
  }

  return {
    open: () => searchOpen,
    setOpen(open) {
      searchOpen = open;
    },
    query: () => searchQuery,
    setQuery(text) {
      searchQuery = text;
    },
    marked: () => markedQuery,
    setMarked(text) {
      markedQuery = text;
    },
    markSoon() {
      if (searchMarkTimer !== null) deps.clock.clearTimeout(searchMarkTimer);
      searchMarkTimer = markedQuery === searchQuery ? null : deps.clock.setTimeout(markSearchNow, SEARCH_MARK_DELAY_MS);
    },
    markNow: markSearchNow,
  };
}

// What the page and the tests use of it.
export { SEARCH_MARK_DELAY_MS };

// The query parsed, and the words to mark, for the last text asked about (`#606 server-side-search`;
// `#624 voice-page-modularity`, moved from the legacy script into core/search): one parse per change of
// the text, however many rows ask about it.

import { searchDayText, searchPositiveTerms, searchQueryParse } from "./language.ts";
import type { SearchDay, SearchNode } from "./language.ts";

/** A query parsed: the text, its tree (null for no search), and its words to mark. */
export interface ParsedQuery {
  readonly query: string | null;
  readonly node: SearchNode | null;
  readonly terms: string[];
}

/** The parse's one question. */
export interface ParsedQueries {
  /** `query` parsed by the shared language: parsed again only when it differs from the text asked last. */
  parsed(query: string): ParsedQuery;
}

/**
 * Nothing parsed yet. `today` gives the reader's calendar day (`#2425`), against which relative
 * days (`after:7d`) are read; a query is parsed again when the day turns, so "the last week"
 * moves with it. Without it, relative days are words.
 */
export function createParsedQueries(today?: () => SearchDay): ParsedQueries {
  let searchParse: ParsedQuery = { query: null, node: null, terms: [] };
  let parsedOn = "";
  return {
    parsed(query) {
      const day = today ? today() : null;
      const on = day ? searchDayText(day) : "";
      if (searchParse.query !== query || parsedOn !== on) {
        const node = searchQueryParse(query, day);
        searchParse = { query, node, terms: searchPositiveTerms(node) };
        parsedOn = on;
      }
      return searchParse;
    },
  };
}

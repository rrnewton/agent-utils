// The query parsed, and the words to mark, for the last text asked about (`#606 server-side-search`;
// `#624 voice-page-modularity`, moved from the legacy script into core/search): one parse per change of
// the text, however many rows ask about it.

import { searchPositiveTerms, searchQueryParse } from "./language.ts";
import type { SearchNode } from "./language.ts";

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

/** Nothing parsed yet. */
export function createParsedQueries(): ParsedQueries {
  let searchParse: ParsedQuery = { query: null, node: null, terms: [] };
  return {
    parsed(query) {
      if (searchParse.query !== query) {
        const node = searchQueryParse(query);
        searchParse = { query, node, terms: searchPositiveTerms(node) };
      }
      return searchParse;
    },
  };
}

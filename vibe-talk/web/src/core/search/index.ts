// The search language (`#521 server-side-search`): its public interface. Everything else in this
// package is private to it. `#624 voice-page-modularity`.

export {
  searchFold,
  searchPositiveTerms,
  searchQueryMatches,
  searchQueryParse,
  searchQueryTree,
  searchSubject,
  searchUpstreamPlan,
} from "./language.ts";
export type {
  SearchDay,
  SearchNode,
  SearchScanReason,
  SearchSubject,
  SearchSubjectInput,
  SearchTree,
  SearchUpstreamPlan,
} from "./language.ts";

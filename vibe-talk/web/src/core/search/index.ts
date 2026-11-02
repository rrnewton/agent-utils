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
export { SERVER_SEARCH_DELAY_MS, createServerSearch } from "./server.ts";
export type { ServerSearch, ServerSearchDependencies, ServerSearchState } from "./server.ts";
export { SEARCH_FURTHER_POLL_MS, createGathering } from "./gather.ts";
export type { Gathering, GatheringDependencies, GatheringState } from "./gather.ts";
export { SEARCH_MARK_DELAY_MS, createSearchField } from "./field.ts";
export type { SearchField, SearchFieldDependencies } from "./field.ts";

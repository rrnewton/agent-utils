// Sync: which read a refresh makes and how fresh it is, and in time its scheduling. Its public
// interface. `#624 voice-page-modularity`.

export {
  STALE_AS_OF_MS,
  TIMELINE_DELTA_PAGES,
  TIMELINE_RECONCILE_MS,
  VISIBLE_REFRESH_MS,
  deltaCursor,
  isCursorRefusal,
  isMutationKey,
  staleAsOf,
} from "./freshness.ts";
export type { CursorCover, CursorFacts } from "./freshness.ts";

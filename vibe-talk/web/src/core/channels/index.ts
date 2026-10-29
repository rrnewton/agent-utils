// One store per channel, and the views projected from it. Its public interface.
// `#624 voice-page-modularity`.

export { emptyCanon, inTimeOrder, inViewOf, sameMessage, threadOf, timeOf, viewOfKey } from "./canon.ts";
export type { ChannelCanon, ViewCover } from "./canon.ts";
export {
  allPageAnswersFor,
  deltaSideState,
  exactCount,
  foldDeltaIn,
  foldPageIn,
  goneByAbsence,
  heldCopies,
  heldNow,
  mergeIntoCanon,
  pageCopyNewer,
  pageFoldIn,
  refreshFailed,
  removedSince,
  upsertThreadSummary,
} from "./fold.ts";
export type { DeltaFoldDependencies, PageFold, PageFoldDependencies, PageFoldInDependencies } from "./fold.ts";
export { serverVersions } from "./versions.ts";
export type { ServerVersions } from "./versions.ts";
export { coverViewOf, projectionOf } from "./views.ts";
export { restoreCanon, savedTimelineOf } from "./saved.ts";
export type { SavedCover, SavedTimeline } from "./saved.ts";
export { allReachesThread, noteReplyGapsIn } from "./replies.ts";
export type { GapPage } from "./replies.ts";
export { createCopyOrder } from "./order.ts";
export type { CopyOrder } from "./order.ts";
export { createRemovals, forgetUnplaced } from "./removals.ts";
export type { Removals } from "./removals.ts";
export { foldLiveIn, namedRecord } from "./live.ts";
export type { LiveFoldDependencies } from "./live.ts";
export { answeredThread, newerPage, newerPageSpeaksFor, placeFromPage as placeFromPageIn } from "./placement.ts";

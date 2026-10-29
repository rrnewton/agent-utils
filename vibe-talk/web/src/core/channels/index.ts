// One store per channel, and the views projected from it. Its public interface.
// `#624 voice-page-modularity`.

export { emptyCanon, inTimeOrder, inViewOf, sameMessage, threadOf, timeOf, viewOfKey } from "./canon.ts";
export type { ChannelCanon, ViewCover } from "./canon.ts";
export {
  allPageAnswersFor,
  deltaSideState,
  exactCount,
  goneByAbsence,
  heldCopies,
  heldNow,
  mergeIntoCanon,
  pageCopyNewer,
  refreshFailed,
  removedSince,
  serverVersions,
  upsertThreadSummary,
} from "./fold.ts";
export type { ServerVersions } from "./fold.ts";
export { coverViewOf, projectionOf } from "./views.ts";
export { restoreCanon, savedTimelineOf } from "./saved.ts";
export type { SavedCover, SavedTimeline } from "./saved.ts";

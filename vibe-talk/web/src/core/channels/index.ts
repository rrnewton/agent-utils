// One store per channel, and the views projected from it. Its public interface.
// `#624 voice-page-modularity`.

export { emptyCanon, inTimeOrder, inViewOf, sameMessage, threadOf, timeOf, viewOfKey } from "./canon.ts";
export type { ChannelCanon, ViewCover } from "./canon.ts";
export {
  allPageAnswersFor,
  exactCount,
  goneByAbsence,
  heldCopies,
  mergeIntoCanon,
  pageCopyNewer,
  refreshFailed,
  serverVersions,
  upsertThreadSummary,
} from "./fold.ts";
export type { ServerVersions } from "./fold.ts";

// One store per channel, and the views projected from it. Its public interface.
// `#624 voice-page-modularity`.

export {
  createChannelStore,
  emptyCanon,
  inTimeOrder,
  inViewOf,
  sameMessage,
  threadOf,
  timeOf,
  viewOfKey,
} from "./canon.ts";
export type { ChannelCanon, ChannelStore, ViewCover } from "./canon.ts";
export { DISCORD_PAGE_LIMIT, OLDER_TRIGGER_PX, createOlderPager } from "./paging.ts";
export type { OlderPager } from "./paging.ts";
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
export { MESSAGE_CACHE_CHARS, MESSAGE_CACHE_KEY, MESSAGE_CACHE_ROWS, MESSAGE_CACHE_SCOPES, MESSAGE_CACHE_VERSION, createMessageCache, tokenFingerprint } from "./cache.ts";
export type { CacheEntry, CacheShell, MessageCache, MessageCacheDependencies, MessageCacheEnvelope, SavedPage } from "./cache.ts";
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
export { emptyTally, exactReplyCount, replyTallyOf, tallyRecord, unseenRepliesIn } from "./tally.ts";
export type { ReplyTally } from "./tally.ts";
export { markDoneIn, settleDoneIn, settleUnknownReplies, unsettleReplies } from "./done.ts";
export type { HeldDoneMark } from "./done.ts";
export { AUTHORS_KEY, BUCKETS, LEGACY_AUTHORS_KEY, SELF_ID_KEY, codingAgentsFrom, createIdentities } from "./identities.ts";
export type { CodingAgents, DescribedAccounts, Identities, IdentitiesDependencies, IdentityReader, SeenAuthor } from "./identities.ts";
export { SUMMARY_IN_FLIGHT, createSummaries, summaryKey } from "./summaries.ts";
export type { HeldSummary, Summaries, SummariesDependencies, SummaryAnswer, SummaryTarget } from "./summaries.ts";
export { createChannelDirectory } from "./directory.ts";
export type { ChannelDirectory, DeploymentCapabilities } from "./directory.ts";
export { answerUnplaced, heldAsSent, unplacedToAsk } from "./unplaced.ts";
export { UNPACK_READS_AT_ONCE, createUnpackReads } from "./unpackreads.ts";
export { isThreadRoot, readLook, unpackHeldMessages, unpackMessages, unpackReadSignature } from "./unpackheld.ts";
export type { UnpackChoice, UnpackFilter } from "./unpackheld.ts";
export type { UnpackReads, UnpackReadsDependencies, UnpackedThread } from "./unpackreads.ts";
export { createTodoView } from "./todo.ts";
export type { TodoRead, TodoState, TodoView } from "./todo.ts";
export { createPlainReach } from "./plainreach.ts";
export type { PlainReach } from "./plainreach.ts";
export { channelPostsOnce, createProviderFacts } from "./providerfacts.ts";
export type { DescribedProvider, FactsConfig, ProviderFacts } from "./providerfacts.ts";
export { createThreadPresence } from "./threadpresence.ts";
export type { ThreadPresence } from "./threadpresence.ts";

// What the legacy page script (web/voice.js) takes from the modules, and nothing else.
//
// `#624 voice-page-modularity`. The page's code moves out of web/voice.js into these modules a
// piece at a time; what is left there is still a classic script, and still calls what moved. The
// page build (web/build.mjs) bundles this file into a strict prelude that runs just before
// web/voice.js and declares exactly these names for it, so the legacy script reads them as it read
// its own functions before. Name only what voice.js really uses: a name the legacy script no longer
// needs comes off this list, and the list is empty when the legacy script is gone.
// web/legacy-imports.d.ts declares the same names for `tsc --checkJs` over voice.js; a test holds
// the two lists equal.

export {
  searchPositiveTerms,
  searchQueryMatches,
  searchQueryParse,
  searchQueryTree,
  searchSubject,
} from "../core/search/index.ts";
export { readPieces, sentenceEnds, speechSegments } from "../core/speech/index.ts";
export { SLOW_WORDS, createApi, isSlowFailure, providerWords, readableFailure } from "../core/api/index.ts";
export {
  allPageAnswersFor,
  allReachesThread,
  answeredThread,
  coverViewOf,
  createChannelStore,
  createCopyOrder,
  createRemovals,
  deltaSideState,
  emptyCanon,
  emptyTally,
  exactReplyCount,
  exactCount,
  foldDeltaIn,
  foldLiveIn,
  foldPageIn,
  forgetUnplaced,
  goneByAbsence,
  heldCopies,
  heldNow,
  inTimeOrder,
  inViewOf,
  mergeIntoCanon,
  namedRecord,
  newerPage,
  newerPageSpeaksFor,
  noteReplyGapsIn,
  pageCopyNewer,
  pageFoldIn,
  placeFromPageIn,
  projectionOf,
  refreshFailed,
  removedSince,
  replyTallyOf,
  restoreCanon,
  sameMessage,
  savedTimelineOf,
  tallyRecord,
  serverVersions,
  threadOf,
  timeOf,
  unseenRepliesIn,
  upsertThreadSummary,
  viewOfKey,
} from "../core/channels/index.ts";
export { createNavigation } from "../core/navigation/index.ts";
export {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_READING_CH,
  DEVICE_SETTINGS,
  IDENTITY_KEY,
  MAX_CARD_MAX_LINES,
  MAX_COLLAPSED_LINES,
  MAX_MSG_SCALE,
  MAX_READING_CH,
  MAX_READ_SPEED,
  MIN_CARD_MAX_LINES,
  MIN_COLLAPSED_LINES,
  MIN_MSG_SCALE,
  MIN_READING_CH,
  MIN_READ_SPEED,
  clampCardMaxLines,
  clampCollapsedLines,
  clampMsgScale,
  clampReadSpeed,
  clampReadingWidth,
  createCredentials,
  createDeviceSettings,
  createSettingsSync,
} from "../core/settings/index.ts";
export { createWebPorts } from "../platform/web/index.ts";
export {
  DEEP_LINK_PART_MAX,
  SPACE_EM,
  WIDE_SCRIPT,
  deepLinkRefFrom,
  deepLinkRefText,
  glyphEm,
  linkKind,
  messageLinkRef,
  monoEm,
  sentenceStart,
} from "../foundation/index.ts";

import { decode, decodeTagged, is, roots } from "../contract/index.ts";

/**
 * The wire contract's validators as the legacy script has always called them: one frozen object
 * with `roots`, `is`, `decode` and `decodeTagged`, the shape web/contract.js publishes as a global.
 * The prelude's constant of this name is what voice.js reads; web/contract.d.ts types it.
 */
export const VibeTalkContract = Object.freeze({ roots, is, decode, decodeTagged });

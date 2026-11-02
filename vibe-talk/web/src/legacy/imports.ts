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
  createGathering,
  createServerSearch,
  searchPositiveTerms,
  searchQueryMatches,
  searchQueryParse,
  searchQueryTree,
  searchSubject,
} from "../core/search/index.ts";
export { speechHoldFrom } from "../core/speech/index.ts";
export { createApi, isSlowFailure, readableFailure } from "../core/api/index.ts";

export { allReachesThread, answeredThread, coverViewOf, createChannelDirectory, createChannelStore, createCopyOrder, createIdentities, createRemovals, createSummaries, emptyTally, exactReplyCount, foldDeltaIn, foldLiveIn, foldPageIn, heldNow, inTimeOrder, inViewOf, markDoneIn, namedRecord, newerPage, newerPageSpeaksFor, noteReplyGapsIn, pageFoldIn, placeFromPageIn, projectionOf, refreshFailed, replyTallyOf, restoreCanon, savedTimelineOf, settleDoneIn, summaryKey, tallyRecord, threadOf, timeOf, unseenRepliesIn, upsertThreadSummary, viewOfKey } from "../core/channels/index.ts";
export { anchorOfMessage, createBookmarks, createPins, createReadLedger, isBookmarkState, sameAnchor } from "../core/annotations/index.ts";
export { DEEP_LINK_VIEWS, REOPENED_VIEWS, UI_STATE_CHARS, UI_STATE_KEY, UI_STATE_VERSION, createNavigation, createUiState, deepLinkParams, deepLinkPart, deepLinkQuery, placeIsOver, visitKey } from "../core/navigation/index.ts";
export {
  DISCORD_POLL_MS,
  LIVE_RETRY_MS,
  STALE_AS_OF_MS,
  TIMELINE_DELTA_PAGES,
  TIMELINE_RECONCILE_MS,
  VISIBLE_REFRESH_MS,
  createChannelFollow,
  createLiveEvents,
  createReadHolds,
  createReadOrder,
  createReplayLedger,
  createStandInRetries,
  createStreamGeneration,
  deltaCursor,
  isCursorRefusal,
  joinQueuedReads,
  readEventStream as readEventFrames,
  staleAsOf as syncStaleAsOf,
} from "../core/sync/index.ts";
export {
  DISMISSED_SENDS_DAYS,
  OWN_POST_GRACE_MS,
  SENT_RECEIPT_MS,
  createChannelDrafts,
  createDismissedSends,
  createOutboxStore,
  createOwnPosts,
  nextRetryAt,
  editableSend as outboxEditable,
  outgoingFromSaved as entryFromSaved,
  retriesItself as outboxRetriesItself,
  sendSnippet,
  sentReceiptRetires as receiptRetires,
  underDraft,
  unsentText,
  waitingItsTurn as outboxWaitingItsTurn,
} from "../core/outbox/index.ts";
export { DEFAULT_CARD_MAX_LINES, DEFAULT_COLLAPSED_LINES, DEFAULT_QUOTE_READ_PERCENT, DEVICE_SETTINGS, IDENTITY_KEY, MAX_CARD_MAX_LINES, MAX_COLLAPSED_LINES, MAX_QUOTE_READ_PERCENT, MIN_CARD_MAX_LINES, MIN_COLLAPSED_LINES, MIN_QUOTE_READ_PERCENT, clampCardMaxLines, clampCollapsedLines, clampQuoteReadPercent, createCredentials, createDeviceSettings, createSettingsSync } from "../core/settings/index.ts";
export { createWebPorts } from "../platform/web/index.ts";
export {
  answersEnter,
  childByClass,
  classWords,
  elementIn,
  focusMinimally,
  isLink,
  isTextField,
  keepsOwnKeys,
  nodeWithin,
  pressedBySpace,
  scrollsItself,
  setAttributeIfChanged,
  setClassNameIfChanged,
  setClassWord,
  setHiddenIfChanged,
  setTextIfChanged,
  shownNode,
  tagNameOf,
} from "../view/dom/index.ts";
export {
  COMPOSER_FIELDS,
  COMPOSER_GROW_BOXES,
  COMPOSER_GROW_IDS,
  NEWLINE_KEYS,
  createComposerGrow,
  createComposerKeys,
  focusAtEnd,
  isComposer,
  watchBoxInsertions as watchInsertions,
  wireNewlineKey,
} from "../view/composer/index.ts";
export { HTTP_ADDRESS, createSerialQueue, deepLinkRefFrom, linkKind, linksIn, messageLinkRef, sentenceStart } from "../foundation/index.ts";

export { briefAge, dayClock, dayKeyOf, editedTitle, fullLocalAt, fullLocalTime, messageDate, messageTimeLabel, shortLocalTime, stamp } from "../present/time/index.ts";

export { doneReadWords, failureWords, fmtDuration, messagesWords, reachBackWords, reachClause, readableCause, refusalSentence, refusalWords, searchMomentWords, searchWalkProgress, serviceReadWords, } from "../present/notices/index.ts";
export { AUTHOR_FILTERS, BLANK_TEXT, CARD_STATES, LINK_KINDS, LINK_KINDS_KEY, READ_MODES, READ_MODE_KEY, READ_MODE_LABELS, READ_MODE_SAID, READ_MODE_TITLES, READ_MODE_VERBS, SUMMARY_FAILED_MARK, SUMMARY_MARK, SUMMARY_MODE_OFF, SUMMARY_MODE_ON, SUMMARY_WAITING, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY, THREAD_ORDERS, THREAD_ORDER_KEY, THREAD_ORDER_NAMES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID, authorEmptyWords, bookmarkSpot, cardGapWords, cardMaxLinesWords, cardProxyLines, cardRunText, collapsedLinesWords, combinedContent, countOf, createCardPacking, createLinkKinds, createReadMode, createReadRuns, createThreadOrder, editedDrawn, firstLineOf, foldedCardText, forgetElisionCounts, gapWords, hiddenKindWords, inThreadOrder, isNoise, joinsGroup, linkKindCountText, mainRunsAmong, messageLook, newestThreadHues, nextAuthorFilter, nextReadMode, proxyLinesOf, quoteBlock, quoteMarksRead, quoteShare, rawTextOf, reactionText, reactionsDrawn, readRunText, renderedHtmlOf, replyMarkerName, rowEdited, rowReactions, savedReadMode, seamPlan, spreadThreadHue, summaryFailureWords, summaryNoteWords, threadChoice, threadFacts, threadOrderAfter, undoLabel, undoName } from "../present/rows/index.ts";
export { DAY_TITLED, EDITED_MARK, PULL_WAIT_MS, QUOTE_REPLY_TITLE, RESTATED_CLASS, SWIPE_COMMIT_PX, anchorOfRow, anchorsAmong, authorLabel, authorPasses, cardGapGroups, cardGapTally, cardMarkTarget, cardRunIds, cardStubText, cardUnits, createAuthorFilter, createBookmarkLines, createColumnLinks, createPullToRefresh, createQuoting, createRowGestures, createSeamLines, doneButtonOf, fillCardText, findableParts, foldedTextLine, freshenDayTitle, fromGatheredThread, gutterPiece, idsOf, isMessageRow, keepCardUnits, markCardLayout, messageTop, notNoiseButtonOf, pointerOn, quoteLabelFits, renderedText, rowIn, rowMessages, rowNewestMs, rowSentMs, rowShown, rowTime, rowTimeTabStop, setArrowReach, setMarkerLift, shownRowFor, stackUnder, standingRow, textHits, textRunsOf, threadCardId, timeNode } from "../view/channel/index.ts";

import { decode, decodeTagged, is, roots } from "../contract/index.ts";

/**
 * The wire contract's validators as the legacy script has always called them: one frozen object
 * with `roots`, `is`, `decode` and `decodeTagged`, the shape web/contract.js publishes as a global.
 * The prelude's constant of this name is what voice.js reads; web/contract.d.ts types it.
 */
export const VibeTalkContract = Object.freeze({ roots, is, decode, decodeTagged });

export { createVoiceCall, createVoiceTranscript, createReadAloud } from "../view/voice/index.ts";

export {
  HELP_TOPICS,
  STRAY_SERVICE_FACTS,
  addChannelForm,
  allChannelsFacts,
  emptyServiceWords,
  serviceFacts,
  writeSourceWords,
} from "../present/settings/index.ts";
export {
  mountBarPlacement,
  mountChannelDirectory,
  mountChannelHistory,
  mountDismissedSends,
  mountHelpFolds,
  mountIdentityList,
  mountPairing,
  mountReadingWidth,
  mountServerCache,
  mountSettingsFolds,
  mountTextSize,
  mountUpstreamClear,
} from "../view/settings/index.ts";

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
  createOpenedGaps,
  createParsedQueries,
  createRepliesWalk,
  createSearchField,
  createServerSearch,
  searchQueryMatches,
  searchQueryTree,
  searchSubject,
} from "../core/search/index.ts";
export { speechHoldFrom } from "../core/speech/index.ts";
export { createApi, createServerFacts, isSlowFailure, readableFailure } from "../core/api/index.ts";

export { allReachesThread, answerUnplaced, answeredThread, coverViewOf, createChannelDirectory, createChannelStore, createCopyOrder, createIdentities, createPlainReach, createProviderFacts, createRemovals, createSummaries, createThreadPresence, createTodoView, createUnpackReads, emptyTally, exactReplyCount, foldDeltaIn, foldLiveIn, foldPageIn, heldAsSent, heldNow, inTimeOrder, inViewOf, markDoneIn, namedRecord, newerPage, newerPageSpeaksFor, noteReplyGapsIn, pageFoldIn, placeFromPageIn, projectionOf, refreshFailed, replyTallyOf, restoreCanon, savedTimelineOf, settleDoneIn, summaryKey, tallyRecord, threadOf, timeOf, unplacedToAsk, unseenRepliesIn, upsertThreadSummary, viewOfKey } from "../core/channels/index.ts";
export { anchorOfMessage, createBookmarkUndo, createBookmarkWrites, createBookmarks, createLegacyMarker, createPins, createReadLedger, isBookmarkState, sameAnchor } from "../core/annotations/index.ts";
export { REOPENED_VIEWS, UI_STATE_CHARS, UI_STATE_KEY, UI_STATE_VERSION, createBackStack, createDeepLinks, createLayoutChoice, createLinkJumps, createNavigation, createUiState, createViewPlaces, createVisits, deepLinkQuery, visitKey } from "../core/navigation/index.ts";
export {
  DISCORD_POLL_MS,
  STALE_AS_OF_MS,
  TIMELINE_DELTA_PAGES,
  TIMELINE_RECONCILE_MS,
  VISIBLE_REFRESH_MS,
  createArrivalCount,
  createAwayClock,
  createChannelFollow,
  createChannelInFlight,
  createChannelPoll,
  createChannelReads,
  createHiddenStream,
  createLiveEvents,
  createMutationRefresh,
  createReadHolds,
  createReadOrder,
  createReopenRead,
  createReplayLedger,
  createScreenIdentity,
  createStandInRetries,
  createStreamGeneration,
  createWatchAll,
  deltaCursor,
  isCursorRefusal,
  staleAsOf as syncStaleAsOf,
} from "../core/sync/index.ts";
export {
  DISMISSED_SENDS_DAYS,
  OWN_POST_GRACE_MS,
  QUICK_DRAFTS_KEY,
  SENT_RECEIPT_MS,
  createChannelDrafts,
  createDismissedSends,
  createDraftLedger,
  createDraftRound,
  createOutboxPump,
  createOutboxStore,
  createOwnPosts,
  createPendingThreads,
  createReplyDrafts,
  createSendNumbers,
  createSendUndo,
  createUnconfirmedSends,
  draftIdentity,
  editableSend as outboxEditable,
  nextRetryAt,
  outgoingFromSaved as entryFromSaved,
  retriesItself as outboxRetriesItself,
  sendSnippet,
  sentReceiptRetires as receiptRetires,
  underDraft,
  unsentText,
  waitingItsTurn as outboxWaitingItsTurn,
} from "../core/outbox/index.ts";
export { DEFAULT_CARD_MAX_LINES, DEFAULT_COLLAPSED_LINES, DEVICE_SETTINGS, IDENTITY_KEY, MAX_COLLAPSED_LINES, MIN_COLLAPSED_LINES, createCardMaxLines, createCollapsedLines, createCredentials, createDeviceSettings, createEnterSends, createInAppLinks, createKeptSwitch, createQuoteReadPercent, createSettingsSync, createSingleKeys } from "../core/settings/index.ts";
export { createWebPorts } from "../platform/web/index.ts";
export {
  answersEnter,
  childByClass,
  classWords,
  copyText,
  createOncePerFrame,
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
export { SCREENS, createActionMenu, createChannelPick, createChipsOverComposer, createHighlightCopy, createKeyDispatch, createKeyHints, createPromptsTray, createReplySight, createRowShortcuts, createScreens, createScrollKeys, createShellNotices, createUndoOffer, createUndoStamps, modHeld, pageStep, swallow, typedKey } from "../view/shell/index.ts";
export {
  COMPOSER_FIELDS,
  COMPOSER_GROW_BOXES,
  COMPOSER_GROW_IDS,
  NEWLINE_KEYS,
  createComposerGrow,
  createComposerKeys,
  createDictation,
  createOnScreenKeyboard,
  focusAtEnd,
  isComposer,
  watchBoxInsertions as watchInsertions,
  wireNewlineKey,
} from "../view/composer/index.ts";
export { HTTP_ADDRESS, createSerialQueue, linkKind, linksIn, messageLinkRef, sentenceStart } from "../foundation/index.ts";

export { briefAge, dayClock, dayKeyOf, editedTitle, fullLocalAt, fullLocalTime, messageDate, messageTimeLabel, shortLocalTime, stamp } from "../present/time/index.ts";

export { doneReadWords, failureWords, fmtDuration, messagesWords, reachBackWords, reachClause, readableCause, refusalSentence, refusalWords, repliesWalkWords, searchMomentWords, searchWalkProgress, searchWalkWords, serviceReadWords } from "../present/notices/index.ts";
export { AUTHOR_FILTERS, BLANK_TEXT, CARD_STATES, LINK_KINDS, LINK_KINDS_KEY, READ_MODES, READ_MODE_KEY, READ_MODE_LABELS, READ_MODE_SAID, READ_MODE_TITLES, READ_MODE_VERBS, SUMMARY_FAILED_MARK, SUMMARY_MARK, SUMMARY_MODE_OFF, SUMMARY_MODE_ON, SUMMARY_WAITING, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY, THREAD_ORDERS, THREAD_ORDER_KEY, THREAD_ORDER_NAMES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID, authorEmptyWords, bookmarkSpot, cardGapWords, cardProxyLines, cardRunText, collapsedLinesWords, combinedContent, countOf, createCardPacking, createLinkKinds, createReadMode, createReadRuns, createThreadOrder, editedDrawn, firstLineOf, foldedCardText, forgetElisionCounts, gapWords, hiddenKindWords, inThreadOrder, isNoise, joinsGroup, linkKindCountText, mainRunsAmong, messageLook, nextAuthorFilter, nextReadMode, proxyLinesOf, quoteBlock, quoteMarksRead, quoteShare, rawTextOf, reactionText, reactionsDrawn, readRunText, renderedHtmlOf, replyMarkerName, rowEdited, rowReactions, savedReadMode, seamPlan, summaryFailureWords, summaryNoteWords, threadChoice, threadFacts, threadOrderAfter, undoLabel, undoName, upstreamAppUrlOf, upstreamUrlOf } from "../present/rows/index.ts";
export { DAY_TITLED, EDITED_MARK, PULL_WAIT_MS, QUOTE_REPLY_TITLE, RESTATED_CLASS, SWIPE_COMMIT_PX, anchorOfRow, anchorsAmong, authorLabel, authorPasses, cardGapGroups, cardGapTally, cardMarkTarget, cardRunIds, cardStubText, cardUnits, createArming, createAuthorFilter, createBookmarkLines, createCardFlight, createCardPlaces, createCardsDrawn, createColumnLinks, createColumnWork, createFilterChanges, createFootRooms, createInputKinds, createLastPane, createLayoutShown, createPackSlides, createPinSync, createPullToRefresh, createQuoting, createReplyState, createRowDrawing, createRowGestures, createRowMenus, createScrollKeeping, createSeamLines, createSummaryWatch, createThreadHues, createUnderLists, createUnpackWatch, doneButtonOf, fillCardText, findableParts, foldedTextLine, freshenDayTitle, fromGatheredThread, gutterPiece, idsOf, isMessageRow, keepCardUnits, markCardLayout, messageTop, neighbourStayed, notNoiseButtonOf, pointerOn, quoteLabelFits, renderedText, rowIn, rowMessages, rowNeighbours, rowNewestMs, rowSentMs, rowShown, rowTime, rowTimeTabStop, sameCardPlace, setArrowReach, setMarkerLift, shownRowFor, stackUnder, standingRow, textHits, textRunsOf, threadCardId, timeNode } from "../view/channel/index.ts";

import { decode, decodeTagged, is, roots } from "../contract/index.ts";

/**
 * The wire contract's validators as the legacy script has always called them: one frozen object
 * with `roots`, `is`, `decode` and `decodeTagged`, the shape web/contract.js publishes as a global.
 * The prelude's constant of this name is what voice.js reads; web/contract.d.ts types it.
 */
export const VibeTalkContract = Object.freeze({ roots, is, decode, decodeTagged });

export { createReadAloud, createReadingJump, createVoiceCall, createVoiceTranscript } from "../view/voice/index.ts";

export { HELP_TOPICS } from "../present/settings/index.ts";
export { CANNED_PROMPTS, MIC_TOGGLES, mountAgentPrefix, mountAgentSettings, mountAudioDevices, mountBarPlacement, mountCannedPrompts, mountChannelDirectory, mountChannelHistory, mountChannelSettings, mountConnectionReadouts, mountDismissedSends, mountHelpFolds, mountIdentityList, mountMessagePreferences, mountMicrophoneSettings, mountNoiseRules, mountOwnReadApply, mountPairing, mountReadingWidth, mountResumePreference, mountSaveTokenButton, mountServerCache, mountSettingsFolds, mountSettingsReadouts, mountSettingsSyncView, mountTextSize, mountUpstreamClear } from "../view/settings/index.ts";
export { blankResumeLast, createAudioRoute, createCallResume, createVoiceProvider } from "../core/call/index.ts";

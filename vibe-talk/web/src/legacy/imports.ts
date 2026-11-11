export { createApplication, createKeyboardComposition, createServerAccess, createPageGuards } from "../app/index.ts";
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
export { createServerFacts, isSlowFailure, readableFailure } from "../core/api/index.ts";

export { channelPostsOnce, allReachesThread, createChannelDirectory, createChannelStore, createCopyOrder, createIdentities, createProviderFacts, createThreadPresence, createUnpackReads, exactReplyCount, heldNow, newerPage, refreshFailed, threadOf, timeOf, upsertThreadSummary } from "../core/channels/index.ts";
export { createReadLedger } from "../core/annotations/index.ts";
export { REOPENED_VIEWS, UI_STATE_CHARS, UI_STATE_KEY, UI_STATE_VERSION, createBackStack, createDeepLinks, createLayoutChoice, createLinkJumps, createNavigation, createUiState, createViewPlaces, createVisits, deepLinkQuery, visitKey } from "../core/navigation/index.ts";
export {
  DISCORD_POLL_MS,
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
  isCursorRefusal,
} from "../core/sync/index.ts";
export {
  DISMISSED_SENDS_DAYS,
  QUICK_DRAFTS_KEY,
  createChannelDrafts,
  createDraftLedger,
  createDraftRound,
  createInboxWriteLedger,
  createOutboxComposition,
  createOwnPosts,
  createPendingThreads,
  createReplyDrafts,
  createUnconfirmedSends,
  retryingWrite,
  sendSnippet,
  sentReceiptRetires as receiptRetires,
  underDraft,
  unsentText,
} from "../core/outbox/index.ts";
export { DEFAULT_COLLAPSED_LINES, DEFAULT_TWO_COLUMN_SPLIT, DEVICE_SETTINGS, IDENTITY_KEY, MAX_COLLAPSED_LINES, MIN_COLLAPSED_LINES, clampTwoColumnSplit, createCardMaxLines, createCollapsedLines, createEnterSends, createInAppLinks, createKeptSwitch, createQuoteReadPercent, createSettingsSync } from "../core/settings/index.ts";
export {
  childByClass,
  createOncePerFrame,
  focusLost,
  focusMinimally,
  isTextField,
  keepsOwnKeys,
  nodeWithin,
  pressedBySpace,
  scrollsItself,
  setHiddenIfChanged,
  setTextIfChanged,
  shownNode,
} from "../view/dom/index.ts";
export { SCREEN_TITLES, createActionMenu, createChannelDirectoryView, createDeploymentWords, createChannelSelection, createChipsOverComposer, createControlBar, createPromptsTray, createReaderRow, createReplySight, createPageNavigation, createScreens, createScreenShowing, createShellNotices, createPageVisibility, createUndoOffer, createUndoStamps, modHeld, mountDocumentListeners, mountKeyListeners, mountMenuOpeners, mountPageListeners, pageStep, swallow } from "../view/shell/index.ts";
export {
  NEWLINE_KEYS,
  createComposerBoxes,
  createComposerKeys,
  createDictation,
  createKeyboardDebug,
  createOnScreenKeyboard,
  focusAtEnd,
  isComposer,
  mountComposerGrowListeners,
  mountKeyboardListeners,
  mountMessageBox,
  mountQuickReplyListeners,
  wireNewlineKey,
} from "../view/composer/index.ts";

export { briefAge, fullLocalAt, stamp } from "../present/time/index.ts";

export { doneReadWords, failureWords, localReadWords, messagesWords, reachBackWords, reachClause, readableCause, refusalSentence, refusalWords, searchMomentWords, serviceReadWords } from "../present/notices/index.ts";
export { LINK_KINDS_KEY, READ_MODES, READ_MODE_KEY, READ_MODE_SAID, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY, THREAD_ORDERS, THREAD_ORDER_KEY, THREAD_ORDER_NAMES, cardGapWords, combinedContent, countOf, createCardPacking, createLinkKinds, createReadMode, editedDrawn, gapWords, inThreadOrder, isNoise, mainRunsAmong, messageLook, quoteBlock, rawTextOf, reactionsDrawn, renderedHtmlOf, savedReadMode, undoLabel, undoName, upstreamUrlOf } from "../present/rows/index.ts";
export { PULL_WAIT_MS, REPLY_LANDING_GAP_PX, anchorOfRow, authorPasses, createCardFlight, createCardPlaces, createColumnSplitter, createColumnReplies, createColumnWork, createColumnStateAccess, createColumns, createElementLookup, createFootRooms, createLayoutShown, createJumpAnchor, createLinkedMark, createPackSlides, screenMotionMs, createPullToRefresh, createQuoting, createReadingColumn, createReplyArrows, createSentRead, createReplyGathering, createReplySession, createChannelViews, createChannelScope, createChannelLoading, createDayTap, createMessageDetails, createNoiseActs, createRichHeaders, createStartsRead, createFilterPlace, createFreshRead, createHeldDone, createReadState, createPendingSends, createOlderPages, createOpeningView, createPlacing, createReplyCounts, createDoneAct, createLiveState, createSideCards, createThreadEntry, createTwoColumns, createRowDrawing, createRowGestures, createScrollKeeping, createUnderLists, createUnpackWatch, FOLLOW_REST_PX, fillCardText, freshenDayTitle, freshComposerState, freshEntryState, freshDoneState, freshLoadState, freshOlderPages, freshPinsView, freshScrollPlace, freshScrollTools, freshSelection, freshTimelineRows, freshFilterState, freshGatherState, freshListState, freshReplyArrows, fromGatheredThread, idsOf, markCardLayout, messageTop, mountChannelViewControls, mountColumnSplitterControls, mountTextSelection, mountFilterChips, mountKeptSends, mountListControls, mountListGestures, mountListKeepers, mountListWatch, mountReadingJumps, mountReplyBox, mountSideControls, mountSideListControls, notNoiseButtonOf, quoteLabelFits, renderedText, rowIn, rowMessages, rowShown, rowTimeTabStop, sameCardPlace, shownRowFor, createChannelHead,
  createSearchOpenControl,
  createServiceWords,
  createOpenInView,
  } from "../view/channel/index.ts";

import { decode, decodeTagged, is, roots } from "../contract/index.ts";

/**
 * The wire contract's validators as the legacy script has always called them: one frozen object
 * with `roots`, `is`, `decode` and `decodeTagged`, the shape web/contract.js publishes as a global.
 * The prelude's constant of this name is what voice.js reads; web/contract.d.ts types it.
 */
export const VibeTalkContract = Object.freeze({ roots, is, decode, decodeTagged });

export { createAutoplay, createReadAloud, createReadingJump, createVoiceCall, createVoiceTranscript, createWatchFrame, mountCallComposer, mountCallControls, mountPageButtons, mountReadControls, createReadingModeActs } from "../view/voice/index.ts";

export {
  CANNED_PROMPTS,
  mountAgentPrefix,
  mountAgentSettings,
  mountAudioDevices,
  mountBarPlacement,
  mountCannedPrompts,
  mountChannelBoxListeners,
  createClientConfig,
  createSettingsSyncGlue,
  createStoredScreen,
  createSignIn,
  mountChannelDirectory,
  mountChannelHistory,
  mountChannelSettings,
  mountConnectionReadouts,
  mountDismissedSends,
  mountHelpFolds,
  mountHelpLinks,
  mountHelpScreen,
  mountIdentityList,
  mountMessagePreferences,
  mountMicrophoneSettings,
  mountNoiseRules,
  mountOwnReadApply,
  mountPairing,
  mountPreferenceSwitches,
  mountReadingWidth,
  mountResumePreference,
  mountSaveTokenButton,
  mountScreenControls,
  mountServerCache,
  mountSettingsFolds,
  mountSettingsReadouts,
  mountSettingsSwitches,
  mountSettingsSyncView,
  mountSignInControls,
  mountTextSize,
  mountUpstreamClear,
  restoreStoredSettings,
} from "../view/settings/index.ts";
export { audioConstraints, blankResumeLast, createAudioRoute, createCallDevices, createCallResume, createVoiceProvider } from "../core/call/index.ts";

// read core (#624 mu-readcore)
export { createAbandonedReads, createTimelineReader } from "../core/sync/index.ts";
export { DISCORD_PAGE_LIMIT, OLDER_TRIGGER_PX } from "../core/channels/index.ts";
export { isThreadRoot, readLook, unpackHeldMessages, unpackMessages, unpackReadSignature } from "../core/channels/index.ts";
export { staleWords } from "../present/rows/index.ts";
export { tokenFingerprint } from "../core/channels/index.ts";
// view/documents (documents-view-client)
export { createDocumentsView, createViewSelector, DOC_STATES_KEY } from "../view/documents/index.ts";
export { createTimelineView } from "../view/channel/index.ts";
export { COLLAPSE_OVER_CHARS, FOLD_LESS, FOLD_MORE, createFolding } from "../view/channel/index.ts";
export { createChannelRows } from "../view/channel/index.ts";
export { createSearchRows } from "../view/channel/index.ts";
export { SEARCH_OLDER_HITS, SEARCH_OLDER_MS, SEARCH_OLDER_PAGES, SEARCH_OLDER_TAP_MS, createCardGaps, createChannelRefresh, createLiveStream, createMessageBodies, createOutboxRows, createPackControls, createRowDress, createUnpackBlock, mountOlderWalks } from "../view/channel/index.ts";
export { createRowLinks } from "../view/channel/index.ts";
export { headingLength, headingOf } from "../present/rows/index.ts";
export { createCardLines, createReadingBookmark, createSpeakers } from "../view/channel/index.ts";
export { createCardRefresh } from "../view/channel/index.ts";
export { createPinnedMessages, createServerSearchCopy } from "../view/channel/index.ts";

// outbox loads, drafts and sends (#624 mu-readcore O1)
export { createChannelSend } from "../view/composer/index.ts";
export { createDraftQueries, createOutboxLoads } from "../core/outbox/index.ts";
// outbox dispatch and take-back (#624 mu-readcore O2)
export { createOutboxDispatch } from "../core/outbox/index.ts";
export { createSendTakeBack } from "../view/channel/index.ts";
export { createThreadList } from "../view/channel/index.ts";
export { createOfflineCache, createThreadStrip } from "../view/channel/index.ts";
// scroll trace (#1144 scrollback-typing-jump part 2b)
export { createScrollTrace, createScrollTraceControls } from "../view/channel/index.ts";
// read state's acts beside Done's (#624 mu-keyread)
export { createReadActs } from "../view/channel/index.ts";
// one card highlighted (#624 mu-keyread)
export { createLitCards } from "../view/channel/index.ts";
// the card at the top of an open thread (#624 mu-keyread)
export { createThreadCardHead } from "../view/channel/index.ts";
// the main channel's messages among the cards (#624 mu-keyread)
export { createMainCards } from "../view/channel/index.ts";
// leaving the thread list and the card-place acts (#624 mu-keyread)
export { createCardPlaceActs } from "../view/channel/index.ts";
// the card moving up and the right column's thread (#624 mu-keyread)
export { createSideThreadActs } from "../view/channel/index.ts";
// the search over the cards, their read runs and the column links (#624 mu-keyread)
export { createCardRuns } from "../view/channel/index.ts";

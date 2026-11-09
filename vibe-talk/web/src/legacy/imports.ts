export { createApplication, createKeyboardComposition } from "../app/index.ts";
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

export { channelPostsOnce, allReachesThread, answerUnplaced, createChannelDirectory, createChannelStore, createCopyOrder, createIdentities, createPlainReach, createProviderFacts, createRemovals, createThreadPresence, createUnpackReads, emptyTally, exactReplyCount, heldAsSent, heldNow, inTimeOrder, markDoneIn, newerPage, noteReplyGapsIn, placeFromPageIn, refreshFailed, replyTallyOf, savedTimelineOf, settleDoneIn, tallyRecord, threadOf, timeOf, unplacedToAsk, unseenRepliesIn, upsertThreadSummary } from "../core/channels/index.ts";
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
export { DEFAULT_COLLAPSED_LINES, DEVICE_SETTINGS, IDENTITY_KEY, MAX_COLLAPSED_LINES, MIN_COLLAPSED_LINES, createCardMaxLines, createCollapsedLines, createCredentials, createDeviceSettings, createEnterSends, createInAppLinks, createKeptSwitch, createQuoteReadPercent, createSettingsSync } from "../core/settings/index.ts";
export { createWebPorts } from "../platform/web/index.ts";
export {
  childByClass,
  createOncePerFrame,
  elementIn,
  focusMinimally,
  isTextField,
  keepsOwnKeys,
  nodeWithin,
  pressedBySpace,
  scrollsItself,
  setHiddenIfChanged,
  setTextIfChanged,
  setClassWord,
  shownNode,
} from "../view/dom/index.ts";
export { SCREEN_TITLES, createActionMenu, createChipsOverComposer, createControlBar, createPromptsTray, createReaderRow, createReplySight, createPageNavigation, createScreens, createScreenShowing, createShellNotices, createUndoOffer, createUndoStamps, modHeld, mountDocumentListeners, mountKeyListeners, mountMenuOpeners, mountPageListeners, pageStep, swallow } from "../view/shell/index.ts";
export {
  NEWLINE_KEYS,
  createComposerBoxes,
  createComposerKeys,
  createDictation,
  createOnScreenKeyboard,
  focusAtEnd,
  isComposer,
  mountComposerGrowListeners,
  mountKeyboardListeners,
  mountMessageBox,
  mountQuickReplyListeners,
  wireNewlineKey,
} from "../view/composer/index.ts";
export { sentenceStart } from "../foundation/index.ts";

export { briefAge, fullLocalAt, fullLocalTime, stamp } from "../present/time/index.ts";

export { doneReadWords, failureWords, localReadWords, messagesWords, reachBackWords, reachClause, readableCause, refusalSentence, refusalWords, searchMomentWords, serviceReadWords } from "../present/notices/index.ts";
export { LINK_KINDS_KEY, READ_MODES, READ_MODE_KEY, READ_MODE_SAID, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY, THREAD_ORDERS, THREAD_ORDER_KEY, THREAD_ORDER_NAMES, cardGapWords, cardRunText, collapsedLinesWords, combinedContent, countOf, createCardPacking, createLinkKinds, createReadMode, createReadRuns, createThreadOrder, editedDrawn, gapWords, inThreadOrder, isNoise, mainRunsAmong, messageLook, quoteBlock, rawTextOf, reactionsDrawn, renderedHtmlOf, replyMarkerName, rowEdited, savedReadMode, threadOrderAfter, undoLabel, undoName, upstreamUrlOf } from "../present/rows/index.ts";
export { COLUMN_IDS, DAY_TITLED, PULL_WAIT_MS, QUOTE_REPLY_TITLE, REPLY_LANDING_GAP_PX, anchorOfRow, authorPasses, cardRunIds, createCardFlight, createCardPlaces, createColumnLinks, createColumnReplies, createColumnWork, createColumns, createFootRooms, createLayoutShown, createLinkedMark, createPackSlides, createPullToRefresh, createQuoting, createReadingColumn, createReplyArrows, createSentRead, createReplyGathering, createReplySession, createFilterPlace, createDoneAct, createLiveState, createSideCards, createThreadEntry, createTwoColumns, createRowDrawing, createRowGestures, createScrollKeeping, createThreadHues, createUnderLists, createUnpackWatch, fillCardText, freshenDayTitle, freshEntryState, freshDoneState, freshFilterState, freshGatherState, freshListState, freshReplyArrows, fromGatheredThread, idsOf, markCardLayout, messageTop, mountChannelViewControls, mountFilterChips, mountKeptSends, mountListControls, mountListGestures, mountListKeepers, mountListWatch, mountReadingJumps, mountReplyBox, mountSideControls, mountSideListControls, notNoiseButtonOf, pointerOn, quoteLabelFits, renderedText, rowIn, rowMessages, rowShown, rowTimeTabStop, sameCardPlace, shownRowFor, } from "../view/channel/index.ts";

import { decode, decodeTagged, is, roots } from "../contract/index.ts";

/**
 * The wire contract's validators as the legacy script has always called them: one frozen object
 * with `roots`, `is`, `decode` and `decodeTagged`, the shape web/contract.js publishes as a global.
 * The prelude's constant of this name is what voice.js reads; web/contract.d.ts types it.
 */
export const VibeTalkContract = Object.freeze({ roots, is, decode, decodeTagged });

export { createAutoplay, createReadAloud, createReadingJump, createVoiceCall, createVoiceTranscript, mountCallComposer, mountCallControls, mountPageButtons, mountReadControls } from "../view/voice/index.ts";

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
  createSignIn,
  mountChannelDirectory,
  mountChannelHistory,
  mountChannelSettings,
  mountConnectionReadouts,
  mountDismissedSends,
  mountHelpFolds,
  mountHelpLinks,
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
export { blankResumeLast, createAudioRoute, createCallResume, createVoiceProvider } from "../core/call/index.ts";

// read core (#624 mu-readcore)
export { createAbandonedReads, createTimelineReader } from "../core/sync/index.ts";
export { DISCORD_PAGE_LIMIT, OLDER_TRIGGER_PX, createOlderPager } from "../core/channels/index.ts";
export { isThreadRoot, readLook, unpackHeldMessages, unpackMessages, unpackReadSignature } from "../core/channels/index.ts";
export { staleWords } from "../present/rows/index.ts";
export { MESSAGE_CACHE_ROWS, tokenFingerprint } from "../core/channels/index.ts";
// view/documents (documents-view-client)
export { createDocumentsView, createViewSelector } from "../view/documents/index.ts";
export { createTimelineView } from "../view/channel/index.ts";
export { COLLAPSE_OVER_CHARS, FOLD_LESS, FOLD_MORE, createFolding } from "../view/channel/index.ts";
export { DISMISS_BATCH, createChannelRows } from "../view/channel/index.ts";
export { createSearchRows } from "../view/channel/index.ts";
export { SEARCH_OLDER_HITS, SEARCH_OLDER_MS, SEARCH_OLDER_PAGES, SEARCH_OLDER_TAP_MS, createCardGaps, createChannelRefresh, createLiveStream, createMessageBodies, createOutboxRows, createPackControls, createRowDress, createUnpackBlock, mountOlderWalks } from "../view/channel/index.ts";
export { createRowLinks } from "../view/channel/index.ts";
export { headingLength, headingOf } from "../present/rows/index.ts";
export { createCardLines, createReadingBookmark, createSpeakers } from "../view/channel/index.ts";
export { createPinnedMessages, createServerSearchCopy } from "../view/channel/index.ts";

// outbox loads, drafts and sends (#624 mu-readcore O1)
export { createChannelSend } from "../view/composer/index.ts";
export { createDraftQueries, createOutboxLoads } from "../core/outbox/index.ts";
// outbox dispatch and take-back (#624 mu-readcore O2)
export { createOutboxDispatch } from "../core/outbox/index.ts";
export { createSendTakeBack } from "../view/channel/index.ts";
export { createThreadList } from "../view/channel/index.ts";
export { createOfflineCache, createReadRunLines, createThreadStrip } from "../view/channel/index.ts";

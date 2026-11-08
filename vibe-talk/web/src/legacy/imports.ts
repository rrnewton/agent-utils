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

export { allReachesThread, answerUnplaced, answeredThread, createChannelDirectory, createChannelStore, createCopyOrder, createIdentities, createPlainReach, createProviderFacts, createRemovals, createThreadPresence, createTodoView, createUnpackReads, emptyTally, exactReplyCount, heldAsSent, heldNow, inTimeOrder, markDoneIn, namedRecord, newerPage, noteReplyGapsIn, placeFromPageIn, refreshFailed, replyTallyOf, savedTimelineOf, settleDoneIn, tallyRecord, threadOf, timeOf, unplacedToAsk, unseenRepliesIn, upsertThreadSummary } from "../core/channels/index.ts";
export { anchorOfMessage, createBookmarkUndo, createBookmarkWrites, createBookmarks, createLegacyMarker, createPins, createReadLedger, isBookmarkState, sameAnchor } from "../core/annotations/index.ts";
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
  OWN_POST_GRACE_MS,
  QUICK_DRAFTS_KEY,
  SENT_RECEIPT_MS,
  createChannelDrafts,
  createDismissedSends,
  createDraftLedger,
  createDraftRound,
  createInboxWriteLedger,
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
  retryingWrite,
  sendSnippet,
  sentReceiptRetires as receiptRetires,
  underDraft,
  unsentText,
  waitingItsTurn as outboxWaitingItsTurn,
} from "../core/outbox/index.ts";
export { DEFAULT_CARD_MAX_LINES, DEFAULT_COLLAPSED_LINES, DEVICE_SETTINGS, IDENTITY_KEY, MAX_COLLAPSED_LINES, MIN_COLLAPSED_LINES, createCardMaxLines, createCollapsedLines, createCredentials, createDeviceSettings, createEnterSends, createInAppLinks, createKeptSwitch, createQuoteReadPercent, createSettingsSync } from "../core/settings/index.ts";
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
  setAttributeIfChanged,
  setClassNameIfChanged,
  setClassWord,
  setHiddenIfChanged,
  setTextIfChanged,
  shownNode,
  tagNameOf,
} from "../view/dom/index.ts";
export { SCREEN_TITLES, createActionMenu, createChipsOverComposer, createPromptsTray, createReaderRow, createReplySight, createPageNavigation, createScreens, createScreenShowing, createShellNotices, createUndoOffer, createUndoStamps, modHeld, mountDocumentListeners, mountKeyListeners, mountMenuOpeners, mountPageListeners, pageStep, swallow } from "../view/shell/index.ts";
export {
  COMPOSER_FIELDS,
  NEWLINE_KEYS,
  createComposerGrow,
  createComposerKeys,
  createDictation,
  createOnScreenKeyboard,
  focusAtEnd,
  isComposer,
  mountComposerGrowListeners,
  mountKeyboardListeners,
  mountMessageBox,
  mountQuickReplyListeners,
  watchBoxInsertions as watchInsertions,
  wireNewlineKey,
} from "../view/composer/index.ts";
export { HTTP_ADDRESS, createSerialQueue, linkKind, linksIn, sentenceStart } from "../foundation/index.ts";

export { briefAge, fullLocalAt, fullLocalTime, stamp } from "../present/time/index.ts";

export { doneReadWords, failureWords, fmtDuration, localReadWords, messagesWords, reachBackWords, reachClause, readableCause, refusalSentence, refusalWords, repliesWalkWords, searchMomentWords, serviceReadWords } from "../present/notices/index.ts";
export { AUTHOR_FILTERS, CARD_STATES, LINK_KINDS, LINK_KINDS_KEY, READ_MODES, READ_MODE_KEY, READ_MODE_SAID, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEY, THREAD_ORDERS, THREAD_ORDER_KEY, THREAD_ORDER_NAMES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID, authorEmptyWords, bookmarkSpot, cardGapWords, cardProxyLines, cardRunText, collapsedLinesWords, combinedContent, countOf, createCardPacking, createLinkKinds, createReadMode, createReadRuns, createThreadOrder, editedDrawn, firstLineOf, foldedCardText, forgetElisionCounts, gapWords, hiddenKindWords, inThreadOrder, isNoise, linkKindCountText, mainRunsAmong, messageLook, nextAuthorFilter, proxyLinesOf, quoteBlock, quoteMarksRead, quoteShare, rawTextOf, reactionsDrawn, readRunText, renderedHtmlOf, replyMarkerName, rowEdited, savedReadMode, threadChoice, threadFacts, threadOrderAfter, undoLabel, undoName, upstreamUrlOf } from "../present/rows/index.ts";
export { DAY_TITLED, PULL_WAIT_MS, QUOTE_REPLY_TITLE, RESTATED_CLASS, anchorOfRow, authorLabel, authorPasses, cardGapGroups, cardGapTally, cardMarkTarget, cardRunIds, cardStubText, cardUnits, createArming, createAuthorFilter, createBookmarkLines, createCardFlight, createCardPlaces, createCardsDrawn, createColumnLinks, createColumnWork, createFilterChanges, createFootRooms, createLayoutShown, createLinkedMark, createPackSlides, createPinSync, createPullToRefresh, createQuoting, createReplyState, createRowDrawing, createRowGestures, createScrollKeeping, createSummaryWatch, createThreadHues, createUnderLists, createUnpackWatch, doneButtonOf, fillCardText, foldedTextLine, freshenDayTitle, fromGatheredThread, gutterPiece, idsOf, isMessageRow, keepCardUnits, markCardLayout, messageTop, mountChannelViewControls, mountFilterChips, mountKeptSends, mountListControls, mountListGestures, mountListKeepers, mountListWatch, mountReadingJumps, mountReplyBox, mountSideControls, mountSideListControls, notNoiseButtonOf, pointerOn, quoteLabelFits, renderedText, rowIn, rowMessages, rowNewestMs, rowSentMs, rowShown, rowTimeTabStop, sameCardPlace, setArrowReach, setMarkerLift, shownRowFor, stackUnder, standingRow } from "../view/channel/index.ts";

import { decode, decodeTagged, is, roots } from "../contract/index.ts";

/**
 * The wire contract's validators as the legacy script has always called them: one frozen object
 * with `roots`, `is`, `decode` and `decodeTagged`, the shape web/contract.js publishes as a global.
 * The prelude's constant of this name is what voice.js reads; web/contract.d.ts types it.
 */
export const VibeTalkContract = Object.freeze({ roots, is, decode, decodeTagged });

export { createAutoplay, createReadAloud, createReadingJump, createVoiceCall, createVoiceTranscript, mountCallComposer, mountCallControls, mountPageButtons, mountReadControls } from "../view/voice/index.ts";

export { CANNED_PROMPTS, mountAgentPrefix, mountAgentSettings, mountAudioDevices, mountBarPlacement, mountCannedPrompts, mountChannelBoxListeners, mountChannelDirectory, mountChannelHistory, mountChannelSettings, mountConnectionReadouts, mountDismissedSends, mountHelpFolds, mountHelpLinks, mountIdentityList, mountMessagePreferences, mountMicrophoneSettings, mountNoiseRules, mountOwnReadApply, mountPairing, mountPreferenceSwitches, mountReadingWidth, mountResumePreference, mountSaveTokenButton, mountScreenControls, mountServerCache, mountSettingsFolds, mountSettingsReadouts, mountSettingsSwitches, mountSettingsSyncView, mountSignInControls, mountTextSize, mountUpstreamClear, restoreStoredSettings } from "../view/settings/index.ts";
export { blankResumeLast, createAudioRoute, createCallResume, createVoiceProvider } from "../core/call/index.ts";

// read core (#624 mu-readcore)
export { createAbandonedReads, createTimelineReader } from "../core/sync/index.ts";
export { DISCORD_PAGE_LIMIT, OLDER_TRIGGER_PX, createOlderPager } from "../core/channels/index.ts";
export { isThreadRoot, readLook, unpackHeldMessages, unpackMessages, unpackReadSignature } from "../core/channels/index.ts";
export { staleWords } from "../present/rows/index.ts";
export { MESSAGE_CACHE_ROWS, createMessageCache, tokenFingerprint } from "../core/channels/index.ts";
// view/documents (documents-view-client)
export { createDocumentsView, createViewSelector } from "../view/documents/index.ts";
export { createTimelineView } from "../view/channel/index.ts";
export { COLLAPSE_OVER_CHARS, FOLD_LESS, FOLD_LISTS, FOLD_MORE, createFolding } from "../view/channel/index.ts";
export { DISMISS_BATCH, createChannelRows } from "../view/channel/index.ts";
export { SEARCH_OLDER_HITS, SEARCH_OLDER_MS, SEARCH_OLDER_PAGES, SEARCH_OLDER_TAP_MS, mountOlderWalks } from "../view/channel/index.ts";
export { createSearchRows } from "../view/channel/index.ts";

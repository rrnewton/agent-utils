// present/settings (`#624 voice-page-modularity`): how the Settings and Help screens word and arrange
// what they show. Presentation decisions with no DOM: web/src/view/settings draws what these say.

export { FOLDS_HOLDING_DISMISSED, HELP_TOPICS, SETTINGS_FOLDS, foldAllWords, helpEntrySlugs } from "./folds.ts";
export {
  CHANNEL_HISTORY_MODE_WORDS,
  CHANNEL_HISTORY_QUEUED_RECHECK_MS,
  CHANNEL_HISTORY_RECHECK_MS,
  historyChoiceOf,
  historyDefaultWords,
  historyPrioritizeOffered,
  historyPrioritizedWords,
  historyReadFailure,
  historyRecheckWait,
  historyWaitingWords,
} from "./history.ts";
export {
  UPSTREAM_CLEAR_MODE_WORDS,
  UPSTREAM_CLEAR_RECHECK_LIMIT_MS,
  UPSTREAM_CLEAR_RECHECK_MS,
  upstreamClearAllCommon,
  upstreamClearAllSavedWords,
  upstreamClearAllSentence,
  upstreamClearAttemptKey,
  upstreamClearPressSettled,
  upstreamClearPushingWords,
  upstreamClearReadFailure,
  upstreamClearUnansweredWords,
} from "./upstream.ts";
export type { UpstreamClearAllHeld, UpstreamClearPress } from "./upstream.ts";
export {
  CHANNEL_LABEL_MAX,
  DIRECTORY_PAGE_SIZE,
  DIRECTORY_SEARCH_DELAY_MS,
  DIRECTORY_SLOW_MS,
  DIRECTORY_TICK_MS,
  directoryAskingWords,
  directoryEntryTracked,
  directoryFailureIsFinal,
  directoryFailureSentence,
  directoryLabel,
  directoryPath,
  directorySummary,
  directoryWaitWords,
} from "./directory.ts";
export type { DirectoryEntry, DirectoryFailure } from "./directory.ts";
export { DISMISSED_LABEL_CHARS, dismissedListWords, dismissedRestoreLabel, dismissedRowLabel, dismissedSendMeta } from "./dismissed.ts";
export type { DismissedEntry } from "./dismissed.ts";
export { SERVER_CACHE_ASK, SERVER_CACHE_SENDING, serverCacheKeepsWords, serverCacheUnconfirmed, serverHoldsWholeHistory } from "./cache.ts";
export type { ServerCacheAct } from "./cache.ts";
export {
  BUCKET_LABELS,
  IDENTITY_PICK_ONE,
  IDENTITY_SAVED,
  identityChannelWords,
  identityNobodyWords,
  identityOrder,
  identityPickerLabel,
} from "./identities.ts";
export type { SeenAccount } from "./identities.ts";
export {
  PAIRING_EXPIRED,
  PAIRING_PARTIAL,
  PAIRING_UNREACHED,
  PAIRING_WRONG_LINK,
  pairingCodeShaped,
  pairingRefusedWords,
  pairingTransferAddress,
} from "./pairing.ts";
export { MSG_SCALE_STEP, gripKeysFor, msgScaleStepTarget, msgScaleWords, readingWidthWords } from "./screen.ts";
export { STRAY_SERVICE_FACTS, addChannelForm, allChannelsFacts, emptyServiceWords, serviceFacts, writeSourceWords } from "./channels.ts";
export type { ServiceDescription, WriteSource } from "./channels.ts";
export { DEFAULT_PLACEMENT, PLACEMENTS, placementOf, placementWords } from "./placement.ts";
export { VOICE_SEND_HOSTED, VOICE_SEND_STATE, agentPromptFollowsSwitch, cleanPostPrefix, promptChars, voiceInstructionsLine, voicePostPrefixLine, voicePromptDeliveryLine, voicePromptLastCallLine, voicePromptLock, voiceSendSavedClause, voiceSystemPromptLine } from "./agent.ts";
export type { AgentPromptFollowing, AgentPromptLastCall } from "./agent.ts";
export { MESSAGE_PREFERENCE_WORDS, cardMaxLinesSavedWords, collapsedLinesSavedWords, connectionDetailWords, liveStateWords, quoteReadPercentSavedWords, resumeStateWords, resumeUnavailableWords } from "./preferences.ts";
export type { LiveReadout, ResumeOutcome, ResumeReadout } from "./preferences.ts";

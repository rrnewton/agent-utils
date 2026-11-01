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

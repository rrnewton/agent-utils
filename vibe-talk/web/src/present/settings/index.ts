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

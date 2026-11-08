// present/notices (`#624 voice-page-modularity`): what the page says in its notices and status lines —
// counts and sizes, failures, the server's saved copy, marking read in the chat service, a channel's
// history, a search's reach, a message marked read. Presentation decisions with no DOM: the views put
// these words where they belong.

export { CHANNEL_HISTORY_HOLD_WORDS, channelHistoryDay, channelHistoryDuration, channelHistorySentence } from "./history.ts";
export { MEBIBYTE, cacheSize, fmtDuration, groupedCount, nounCount } from "./counts.ts";
export {
  SERVER_CACHE_NO_AGE_LIMIT,
  SERVER_CACHE_TROUBLE,
  serverCacheErases,
  serverCacheHistory,
  serverCacheLine,
} from "./cache.ts";
export { browserSpeechFailure, failureWords, readableCause, refusalSentence, refusalWords, serviceEndedSentence } from "./failures.ts";
export { upstreamClearClock, upstreamClearCursor, upstreamClearNowSentence, upstreamClearSentence } from "./upstream.ts";
export { doneReadWords, localReadWords, serviceReadWords } from "./marks.ts";
export type { ServiceReadMark } from "./marks.ts";
export { messagesWords, reachBackWords, reachClause, repliesWalkWords, searchMomentWords, searchWalkProgress, searchWalkWords } from "./search.ts";
export type { EndedWalk, HistoryReach, RepliesWalkFacts, WalkFacts } from "./search.ts";
export { outgoingStatusWords, sendTimingTextWords, waitingBehindWords } from "./outgoing.ts";
export type { OutgoingRetryHold, OutgoingStatusEntry } from "./outgoing.ts";

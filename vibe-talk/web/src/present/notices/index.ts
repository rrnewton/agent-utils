// present/notices (`#624 voice-page-modularity`): what the page says in its notices and status lines —
// counts and sizes, failures, the server's saved copy, marking read in the chat service, a channel's
// history. Presentation decisions with no DOM: the views put these words where they belong.

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

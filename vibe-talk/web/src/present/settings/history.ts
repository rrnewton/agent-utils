// What the channel box says about the channel's history on the server, and when it reads it again
// (`#602 history-settings-and-limits`; `#624 voice-page-modularity`, moved from the legacy script into
// present/settings). The box itself is web/src/view/settings's; the progress line is
// web/src/present/notices's (`channelHistorySentence`).

import type { ChannelHistoryState } from "../../contract/index.ts";

/** How often the box reads the channel's history again while Settings shows it. */
const CHANNEL_HISTORY_RECHECK_MS = 30000;
/** How often while the channel is at the front of the server's queue, when it moves fastest. */
const CHANNEL_HISTORY_QUEUED_RECHECK_MS = 5000;

/** The server's modes, as the default choice names them. */
const CHANNEL_HISTORY_MODE_WORDS: { readonly [mode: string]: string | undefined } =
  Object.freeze({ recent: "recent days", full: "whole history", off: "none" });

/** The choice the select shows for what the server holds: the channel's own mode, or the default. */
const historyChoiceOf = (view: ChannelHistoryState): string => view.mode_override || "default";

/** The default choice, naming the server's own mode. */
const historyDefaultWords = (view: ChannelHistoryState): string =>
  `Server default (${CHANNEL_HISTORY_MODE_WORDS[view.server_mode] || view.server_mode})`;

/**
 * Whether "Read older messages next" is offered: to a token that may change it, for a channel whose
 * history is kept, not yet whole, and not already at the front of the queue.
 */
const historyPrioritizeOffered = (view: ChannelHistoryState | null, writable: boolean): boolean =>
  writable && Boolean(view) && (view as ChannelHistoryState).mode !== "off" &&
  !(view as ChannelHistoryState).complete && (view as ChannelHistoryState).queue_position !== 0;

/** How long until the box reads the channel again: sooner while it is at the front of the queue. */
const historyRecheckWait = (view: ChannelHistoryState | null): number =>
  view && view.queue_position === 0 ? CHANNEL_HISTORY_QUEUED_RECHECK_MS : CHANNEL_HISTORY_RECHECK_MS;

/** The progress line while there is no answer: why, or that one is on its way. */
const historyWaitingWords = (failure: string): string => failure || "Checking with the server…";

/** Why the box has no answer for its channel. */
const historyReadFailure = (why: string | undefined): string => `Could not read this channel's history: ${why}`;

/** What "Read older messages next" came to, once the server answered. */
const historyPrioritizedWords = (view: ChannelHistoryState): string => view.complete
  ? "Nothing older to read: every message is saved."
  : "Its older messages are read next, within the chat service's limits.";

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
};

// A channel's history the server gathers in the background, in the reader's words (`#624
// voice-page-modularity`, moved from the legacy script into present/notices).

import type { ChannelHistoryState } from "../../contract/index.ts";
import { dayLabel } from "../time/index.ts";
import { nounCount } from "./counts.ts";

/** Why the server's reads of a chat service are paused, in words. */
const CHANNEL_HISTORY_HOLD_WORDS: Readonly<Record<string, string>> = {
  rate_limited: "the chat service asked for fewer requests",
  upstream_error: "the chat service failed a read",
  provider_signal: "the chat service is limiting requests",
  storage: "the saved copy is near its size limit",
};

/** A span of seconds as a reader says it: "a minute", "12 minutes", "about 3 hours", "about 2 days". */
function channelHistoryDuration(seconds: number): string {
  if (!(seconds >= 90)) return "a minute";
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return nounCount(minutes, "minute");
  const hours = Math.round(seconds / 3600);
  if (hours < 48) return `about ${nounCount(hours, "hour")}`;
  return `about ${nounCount(Math.round(hours / 24), "day")}`;
}

/** A server instant as a day, or "" when absent. */
function channelHistoryDay(ms: unknown): string {
  return typeof ms === "number" && Number.isFinite(ms) ? dayLabel(new Date(ms)) : "";
}

/**
 * What the line under the choice says: what is saved, how complete it is, how the walk into older
 * messages is going and when it should end, the queue, and why it is paused.
 */
function channelHistorySentence(view: ChannelHistoryState): string {
  if (view.mode === "off") return "This server gathers no history in the background.";
  const parts: string[] = [];
  if (view.messages === 0) {
    parts.push("Nothing of this channel is saved yet.");
  } else {
    const oldest = channelHistoryDay(view.oldest_ms);
    parts.push(`${nounCount(view.messages, "message")} saved${oldest ? `, back to ${oldest}` : ""}.`);
  }
  if (view.complete) parts.push("Complete: every message back to the channel's beginning.");
  if (view.walking) {
    const reached = channelHistoryDay(view.reached_ms);
    const pace = typeof view.pages_per_hour === "number" ? ` at ${view.pages_per_hour} pages an hour` : "";
    const left = typeof view.eta_s === "number" ? channelHistoryDuration(view.eta_s) : "";
    const eta = left
      ? ` ${left.charAt(0).toUpperCase()}${left.slice(1)} to go.`
      : view.complete ? "" : " How long it takes is known once a walk has reached the beginning.";
    parts.push(`Reading older messages${reached ? `, back to ${reached} so far` : ""}${pace}.${eta}`);
  } else if (view.mode === "full" && !view.complete) {
    parts.push("Older messages are read a page at a time while nothing else is reading.");
  }
  if (view.queue_position === 0) parts.push("Asked for: it is read next.");
  else if (typeof view.queue_position === "number") {
    parts.push(`Asked for: ${nounCount(view.queue_position, "channel")} ahead of it.`);
  }
  if (view.held_reason) {
    const why = CHANNEL_HISTORY_HOLD_WORDS[view.held_reason] || "it is holding off";
    const left = typeof view.held_until_ms === "number"
      ? ` for ${channelHistoryDuration(Math.max(0, (view.held_until_ms - view.now_ms) / 1000))}`
      : "";
    parts.push(`Paused${left}: ${why}.`);
  }
  return parts.join(" ");
}

export { CHANNEL_HISTORY_HOLD_WORDS, channelHistoryDay, channelHistoryDuration, channelHistorySentence };

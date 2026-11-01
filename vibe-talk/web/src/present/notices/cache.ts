// The server's saved copy of messages, in the reader's words (`#624 voice-page-modularity`, moved
// from the legacy script into present/notices): the line under the switch, and what erasing erases.

import type { MessageCacheStatus } from "../../contract/index.ts";
import { dayLabel } from "../time/index.ts";
import { cacheSize, nounCount } from "./counts.ts";

/** The server's codes for a cache that should be open and is not, in the reader's words. */
const SERVER_CACHE_TROUBLE: Readonly<Record<string, string>> = {
  switch_unreadable: "the switch it keeps beside its store could not be read",
  switch_unwritable: "the switch it keeps beside its store could not be written",
  sqlite_unsupported: "its SQLite lacks something the file needs",
  foreign_file: "a file it did not make is where the file should be",
  locked: "another process holds the file",
  filesystem: "the file or its directory could not be made, or is not private",
  sqlite: "SQLite could not open the file",
};

/** `max_days` when the server keeps every day: a full history with no age limit. `#595 cache-full-history`. */
const SERVER_CACHE_NO_AGE_LIMIT = 65535;

/**
 * What the line under the switch says of how much history the server gathers: with a full history,
 * how many channels it holds whole, how far back the walk still under way has reached, and that it
 * waits when the size limit is near. Empty for the recent days alone. `#595 cache-full-history`.
 */
function serverCacheHistory(status: MessageCacheStatus): string {
  const history = status.history;
  if (!history || history.mode !== "full") return "";
  const whole = history.channels > 0 && history.complete === history.channels
    ? " Complete history of every channel."
    : ` Complete history of ${history.complete} of ${nounCount(history.channels, "channel")}.`;
  const reached = history.reached ? new Date(history.reached) : null;
  const going = reached && !Number.isNaN(reached.getTime())
    ? ` Gathering older messages, back to ${dayLabel(reached)} so far.`
    : "";
  const paused = history.paused === "storage" ? " Gathering waits: the size limit is nearly reached." : "";
  return `${whole}${going}${paused}`;
}

/**
 * The one line under the switch: what the server holds, or why it holds nothing.
 */
function serverCacheLine(status: MessageCacheStatus): string {
  if (status.reason === "config_disabled") {
    return "Off in this server's configuration; only its operator can turn it on.";
  }
  if (!status.enabled) {
    return status.reason === "switched_off"
      ? "Off: you turned it off, and its file was erased. Turn it on to start again with an empty file."
      : "Off.";
  }
  if (!status.available) {
    const why = SERVER_CACHE_TROUBLE[String(status.reason)];
    return `On, but no file is open, so nothing is being saved${why ? `: ${why}` : ""}.`;
  }
  const limits = status.max_days === SERVER_CACHE_NO_AGE_LIMIT
    ? `It keeps every day, up to ${status.max_megabytes} MB.`
    : `It keeps at most ${nounCount(status.max_days, "day")} and ${status.max_megabytes} MB.`;
  if (status.messages === 0) return `On, and nothing is saved yet. ${limits}`;
  const held =
    `On: ${nounCount(status.messages, "message")} from ${nounCount(status.channels, "channel")}, ` +
    `${cacheSize(status.bytes)}.`;
  // Proven whole from there in at least one channel: not a claim that everything since is held.
  const since = status.oldest_covered ? new Date(status.oldest_covered) : null;
  const whole =
    since && !Number.isNaN(since.getTime()) ? ` Complete in places back to ${dayLabel(since)}.` : "";
  return `${held}${whole}${serverCacheHistory(status)} ${limits}`;
}

/**
 * What an erasing act would erase, as its question names it.
 */
function serverCacheErases(status: MessageCacheStatus): string {
  if (!status.available) return "any file it left";
  if (status.messages === 0) return "its file";
  return `the ${nounCount(status.messages, "saved message")} (${cacheSize(status.bytes)})`;
}

export { SERVER_CACHE_NO_AGE_LIMIT, SERVER_CACHE_TROUBLE, serverCacheErases, serverCacheHistory, serverCacheLine };

// What Settings' channel browser asks for and says (`#19 channel-browser`, `#371 channel-search-hash`;
// `#624 voice-page-modularity`, moved from the legacy script into present/settings). The browser itself
// is web/src/view/settings's.

import { sentenceStart } from "../../foundation/index.ts";

/** How long typing pauses before a search is sent, so each keystroke is not its own request. */
const DIRECTORY_SEARCH_DELAY_MS = 250;
/** Entries asked for per page. */
const DIRECTORY_PAGE_SIZE = 25;
/** The longest label the add form accepts, which a browsed name is cut to. */
const CHANNEL_LABEL_MAX = 60;
/** How often the line under the search says how long a read has taken, and after how long it explains why. */
const DIRECTORY_TICK_MS = 1000;
const DIRECTORY_SLOW_MS = 3000;

/** One channel the chat service lists, as the browser holds it. */
interface DirectoryEntry {
  name: string;
  source: string;
  tracked?: boolean;
  channel_id?: string | null;
}

/** A failed read of the directory or a failed add, as the page's server reports one. */
interface DirectoryFailure {
  readonly status?: number;
  readonly code?: string;
  readonly detail?: string;
  readonly message?: string;
}

/** Whether this directory entry is already a channel in the picker; `known` says whether an id is one. */
function directoryEntryTracked(entry: DirectoryEntry, known: (id: string) => boolean): boolean {
  return (
    entry.tracked === true ||
    (entry.channel_id !== null &&
      entry.channel_id !== undefined &&
      known(entry.channel_id))
  );
}

/** The path that reads a page of the directory: of the search `query`, after `cursor`, of `provider`'s channels. */
function directoryPath(query: string, cursor: string | null, provider: string | null): string {
  let path = `/api/v1/channel-directory?limit=${DIRECTORY_PAGE_SIZE}`;
  if (query) path += `&q=${encodeURIComponent(query)}`;
  if (cursor) path += `&cursor=${encodeURIComponent(cursor)}`;
  // `#355 provider-grouped-channels`. With several services, only the one Settings is showing.
  if (provider !== null) path += `&provider=${encodeURIComponent(provider)}`;
  return path;
}

/** The line while a page is read: more of the list, a search, or the first page. */
const directoryAskingWords = (more: boolean, query: string): string => more
  ? "Loading more channels…"
  : query
    ? `Searching for "${query}"…`
    : "Loading channels…";

/**
 * The line once a read has taken `waited` ms: how long, and once it is slow, why the next will not be.
 * `#371 channel-search-hash`.
 */
function directoryWaitWords(asking: string, waited: number, serviceName: string): string {
  const seconds = Math.max(1, Math.round(waited / 1000));
  const why = waited >= DIRECTORY_SLOW_MS
    ? ` ${sentenceStart(serviceName)} can take a while to list its channels; the same search again within two minutes answers at once.`
    : "";
  return `${asking} ${seconds} s so far.${why}`;
}

/** Whether a failed read is one that trying again cannot fix. */
function directoryFailureIsFinal(error: DirectoryFailure): boolean {
  return error.status === 401 || error.status === 403 || error.status === 501;
}

/** What to say when the directory could not be read, by the kind of failure it was. */
function directoryFailureSentence(error: DirectoryFailure, serviceName: string): string {
  if (error.code === "channel_directory_denied") {
    return (
      `${sentenceStart(serviceName)} does not allow this app to list channels. ` +
      "A channel can still be added by pasting its link under Add a channel."
    );
  }
  if (error.status === 401 || error.status === 403) {
    return "This sign-in cannot browse channels: browsing needs the token that can add channels.";
  }
  if (error.status === 501) {
    return (
      "This chat connector cannot list channels. " +
      "A channel can still be added by pasting its link under Add a channel."
    );
  }
  return `Could not load channels. ${error.detail || error.message}`;
}

/** The standing sentence under the search box once a page has arrived. */
function directorySummary(
  shown: { readonly query: string; readonly entries: readonly unknown[]; readonly nextCursor: string | null; readonly truncated: boolean },
  serviceName: string,
): string {
  const count = shown.entries.length;
  if (count === 0) {
    return shown.query
      ? `No channels match "${shown.query}".`
      : `No channels are available in ${serviceName} yet.`;
  }
  const many = `${count} ${count === 1 ? "channel" : "channels"} shown`;
  if (shown.nextCursor) return `${many}. More are available.`;
  if (shown.truncated) {
    return `${many}. The list stopped early, so search by name to find others.`;
  }
  return `${many}.`;
}

/** The label a browsed channel is added under: its name, cut to what the add form accepts. */
const directoryLabel = (name: string): string => Array.from(name).slice(0, CHANNEL_LABEL_MAX).join("").trim();

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
};
export type { DirectoryEntry, DirectoryFailure };

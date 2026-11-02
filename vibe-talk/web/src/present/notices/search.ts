// What a search says of what it holds and how its walk is going (`#624 voice-page-modularity`, moved
// from the legacy script into present/notices): counts, instants inside a sentence, and the stretch of
// history a column is proven to hold (`#578 search-status-clarity`).

import { sentenceStart } from "../../foundation/index.ts";
import { dayClock, dayKeyOf, searchDayWords, shortTimeFormat } from "../time/index.ts";
import type { DayClock } from "../time/index.ts";

/**
 * What the column being worked on holds of its view, in the owner's terms (the legacy script's
 * `historyReach` measures it from the rows, filling it in as it goes).
 */
interface HistoryReach {
  /** the messages loaded in the column being worked on, as the search counts them */
  total: number;
  /** whether a stretch with no gaps is known; else only `total` may be said */
  proven: boolean;
  /** rows saved on this device that no read has checked since */
  saved: boolean;
  /** of `total`, those in the stretch with no gaps */
  dense: number;
  /** of `total`, those older than the stretch, with messages unloaded between */
  scattered: number;
  /** when the stretch's oldest message was sent; with `saved`, the oldest's */
  from: number | null;
  /** the stretch reaches the view's first message (All: its first top-level one) */
  start: boolean;
  /** every message of the view is loaded */
  whole: boolean;
  /** All in a channel with threads: the stretch is its top-level messages */
  topLevel: boolean;
  /** with `topLevel`, the threads started in the stretch that have replies */
  threads: number;
  /** of those, the ones every reply of which is loaded */
  full: number;
}

/** "1 message", "82 messages". */
const messagesWords = (count: number): string => `${count} message${count === 1 ? "" : "s"}`;

/** An instant inside a sentence, to the minute: "today, 3:14 PM", "Oct 3, 9:02 AM", as each row says its time. */
function searchMomentWords(ms: number, clock: DayClock = dayClock()): string {
  const at = new Date(ms);
  return `${searchDayWords(dayKeyOf(at), clock)}, ${shortTimeFormat().format(at)}`;
}

/** ", back to Oct 3, 9:02 AM", or nothing for an instant not known. */
const reachBackWords = (ms: number | null): string => (ms === null ? "" : `, back to ${searchMomentWords(ms)}`);

/**
 * The stretch `reach` proves, as a clause after its count: "a complete history back to Oct 26, 3:14 PM
 * (no gaps), plus 4 scattered older ones"; on All, "every top-level message back to …, and every reply
 * in 20 of their 31 threads". For a reach that is `proven`.
 */
function reachClause(reach: HistoryReach): string {
  const at = reach.from === null ? "" : searchMomentWords(reach.from);
  const back = reach.start
    ? `back to the first one${at ? ` (${at})` : ""}`
    : `back to ${at || "the oldest loaded"} (no gaps)`;
  const scattered = reach.scattered > 0
    ? `, plus ${reach.scattered} scattered older one${reach.scattered === 1 ? "" : "s"}` : "";
  if (!reach.topLevel) return `${reach.start ? "the complete history" : "a complete history"} ${back}${scattered}`;
  const replies = reach.threads === 0 ? ""
    : reach.full === reach.threads
      ? `, and every reply in their ${reach.threads === 1 ? "thread" : `${reach.threads} threads`}`
      : `, and every reply in ${reach.full} of their ${reach.threads} thread${reach.threads === 1 ? "" : "s"}`;
  return `every top-level message ${back}${replies}${scattered}`;
}

/**
 * What the control says while a walk fetches: how many older messages have come so far — "Fetching… 52
 * older messages · Stop" — and never a count of pages, which the reader has no notion of (`#578
 * search-status-clarity`). Short, as the control is one line on a phone; where they came from and how
 * far back they reach is the note's, when the walk ends (`searchWalkNote`).
 */
function searchWalkProgress(walk: { readonly arrived: number }): string {
  return walk.arrived > 0
    ? `Fetching… ${walk.arrived} older message${walk.arrived === 1 ? "" : "s"} · Stop` : "Fetching older messages… · Stop";
}

/** A walk further back, as far as what it says once it has ended reads it. */
interface EndedWalk {
  /** Older messages it fetched. */
  readonly arrived: number;
  /** Matches among them, in shown rows. */
  readonly found: number;
  /** Reads it made. */
  readonly read: number;
}

/** What a walk's ending sentence needs of the page besides the walk. */
interface WalkFacts {
  /** The chat service as a sentence names it (the page's `serviceWords`). */
  readonly service: string;
  /** The walk's time budget, in seconds (`SEARCH_OLDER_MS`). */
  readonly budgetSeconds: number;
  /** The instant of the oldest message loaded (`oldestLoadedMs`), or null. */
  readonly oldest: number | null;
  /** What the list is, as "the start of …" names it (`channelViewWords`); asked only where said. */
  viewWords(): string;
  /** Why the read that failed failed (`failureWords`); asked only for a failure. */
  failure(): string;
}

/**
 * What a walk that has ended says under its control (`#578 search-status-clarity`). Always something: a
 * press that said nothing over a list the search had emptied — the sentence there saying how far back,
 * and not that anything had happened — was the owner's "I press it and it does nothing for a while and
 * then shows THE SAME button". `reason` is why it ended: "failed", "nowhere", "stopped", "start",
 * "budget", or anything else for a walk that ran its pages.
 */
function searchWalkWords(walk: EndedWalk, reason: string, facts: WalkFacts): string {
  const service = facts.service;
  const fetched = `${walk.arrived} older message${walk.arrived === 1 ? "" : "s"} fetched from ${service}` +
    `${reason === "budget" ? ` in ${facts.budgetSeconds} s` : ""}`;
  const at = facts.oldest;
  const back = reachBackWords(at);
  const matches = `${walk.found} more match${walk.found === 1 ? "" : "es"}`;
  if (reason === "failed") {
    const before = walk.arrived > 0 ? ` Before that, ${fetched}${back}.` : " Nothing older was searched.";
    return `Could not fetch older messages from ${service}: ${facts.failure()}.${before} Press the button ` +
      "again to retry.";
  }
  // A list saying older history exists, with no place to go on from: nothing could be read.
  if (reason === "nowhere") return `Nothing older can be fetched: ${service} gave no way further back.`;
  if (reason === "stopped" && walk.arrived === 0 && walk.found === 0 && walk.read === 0) {
    return "Stopped before any older messages arrived.";
  }
  const lead = reason === "stopped" ? "Stopped. " : "";
  if (reason === "start") {
    const first = `${lead}Reached the start of ${facts.viewWords()}${at === null ? "" : ` (${searchMomentWords(at)})`}`;
    if (walk.arrived === 0) return `${first}.`;
    return walk.found > 0 ? `${first}. Found ${matches} among ${fetched}.` : `${first}; none of the ${fetched} match.`;
  }
  // The chat service sent nothing older, and still says there is more: said, so the press is not taken
  // for one that did nothing.
  if (walk.arrived === 0) {
    return `${lead}${sentenceStart(service)} sent nothing older this time, though it says older messages exist. ` +
      "Press the button again to keep going.";
  }
  if (walk.found > 0) return `${lead}Found ${matches} among ${fetched}${back}.`;
  return `${lead}None of the ${fetched}${back}${back ? "," : ""} match.`;
}

export { messagesWords, reachBackWords, reachClause, searchMomentWords, searchWalkProgress, searchWalkWords };
export type { EndedWalk, WalkFacts };
export type { HistoryReach };

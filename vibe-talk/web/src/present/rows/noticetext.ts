// What the page says about how fresh its rows are, and what the notice under the step back says
// over All (`#578 search-status-clarity`, `#617 cold-load-first-paint`, `#231 two-column-polish`;
// `#624 voice-page-modularity`, moved from the legacy script into present/rows — present/notices
// may not import core/channels, whose exact counts `allNoticeWords` counts by; mu-readcore C3).

import type { Message } from "../../contract/index.ts";
import { exactReplyCount, threadOf } from "../../core/channels/index.ts";
import type { ReplyTally } from "../../core/channels/index.ts";
import { sentenceStart } from "../../foundation/index.ts";
import { stamp } from "../time/index.ts";

/**
 * What a freshness pill says of rows that are not a read that just succeeded, in `state` — saved
 * on this device, kept after a refresh that failed, from a service slow to answer — and last
 * current at `atMs`; "" for rows that are current, which each pill says in its own way. `noun`
 * is what the rows are. `serviceName` names the rows' chat service, asked only for a slow answer.
 * The left column's pill and, since `#231 two-column-polish`, the right column's line, in one
 * set of words.
 */
export function staleWords(
  state: string,
  atMs: number,
  refreshing: boolean,
  noun: string = "messages",
  serviceName: () => string = () => "",
): string {
  const at = stamp(atMs);
  if (state === "saved") return refreshing ? `Saved ${at} · refreshing…` : `Showing ${noun} saved ${at}`;
  if (state === "offline") return `Offline · showing ${noun} saved ${at}`;
  // `#617 cold-load-first-paint`: not while a read that may bring them up to date is out.
  if (state === "failed") {
    return refreshing ? `Showing ${noun} from ${at} · refreshing…` : `Refresh failed · showing ${noun} from ${at}`;
  }
  if (state === "slow") {
    return `${sentenceStart(serviceName())} is slow to answer · showing ${noun} from ${at}`;
  }
  return "";
}

/** The words a server's notice that All left some threads' replies out begins with, then and now. */
// The apostrophe is escaped: a bare one in a regular expression reads as an open quote to the page
// suite's scan of this file's code outside its strings.
export const BOUNDED_ALL_NOTICE = /^(Some threads\x27 replies are not in All|Flat view includes replies)/;

/** What counting All's lacking threads reads: the page's threading, view, rows and tally. */
export interface AllNoticeFacts {
  readonly threading: boolean;
  readonly view: string;
  readonly messages: readonly Message[];
  readonly tally: ReplyTally;
}

/**
 * What the notice under the step back says over All of a page the server says it bounded: in
 * threads, not pages (`#578 search-status-clarity`). The owner: "PAGES are not a concept in vibe
 * talk. THREADS are." So it counts the threads on screen whose replies All has not all brought,
 * by their exact counts (`exactReplyCount`), and says nothing once the reader has every one —
 * opened, say. The server's own words where a count is not known; and the notice as it came
 * everywhere else.
 */
export function allNoticeWords(notice: string, facts: AllNoticeFacts): string {
  // Only the notice that All left replies out: a forum's own words on All, say, are another matter.
  if (!notice || !facts.threading || facts.view !== "flat" || !BOUNDED_ALL_NOTICE.test(notice)) return notice;
  const tally = facts.tally;
  let lacking = 0;
  let unknown = false;
  for (const message of facts.messages) {
    const id = threadOf(message);
    if (!id || !message.thread || message.thread.is_root !== true) continue;
    const count = exactReplyCount(id, message, tally);
    if (count === null) {
      unknown = true;
      continue;
    }
    const entry = tally.threads.get(id);
    if (count > (entry ? entry.held : 0)) lacking += 1;
  }
  if (lacking > 0) {
    return `${lacking} thread${lacking === 1 ? " here has replies" : "s here have replies"} All has not loaded; ` +
      "open a thread to read all of it.";
  }
  return unknown ? notice : "";
}

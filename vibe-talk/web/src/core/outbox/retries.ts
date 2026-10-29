// Sending again on its own (`#195 send-resilience`, `#277 send-order`; `#624 voice-page-modularity`,
// moved from the legacy script; plan step 12): when a send whose answer was lost may go out again by
// itself, and how long it waits first. The page arms the timer and draws the row; this is only the
// decision.
//
// A send whose answer was lost may or may not have landed, so sending it again is only safe where the
// second request is recognised as the first: where every attempt carried its key to a provider that
// posts a keyed message at most once. There, and only there, a transient failure goes again after
// 3 s, 6 s, 12 s and then every 20 s, six times in all; everywhere else the only automatic send is one
// that never left the device. Each wait counts from when the failed attempt STARTED (an attempt that
// took longer than the wait goes again after the floor), and none is longer than the longest.

/**
 * `#195 send-resilience`. A send whose answer was lost goes out again on its own after the first
 * wait, then after each wait this many times the last, never more than the longest — 3 s, 6 s,
 * 12 s, 20 s, 20 s, 20 s — and then a person decides. `#277 send-order`: it was 5 s, 15 s and 45 s,
 * counted from the failure, and a message behind one waits for it.
 */
const OUTGOING_RETRY_FIRST_MS = 3000;
const OUTGOING_RETRY_FACTOR = 2;
const OUTGOING_RETRY_MAX_MS = 20000;
const OUTGOING_RETRY_FLOOR_MS = 1000;
const OUTGOING_RETRY_ATTEMPTS = 6;

/** What `retriesItself` reads of an outbox entry. */
export interface RetryFacts {
  readonly keyedOnly?: boolean;
  readonly channel: string;
  readonly postedCount?: number;
  readonly replyTo?: unknown;
  readonly continueIn?: unknown;
  readonly retriesUsed?: number;
}

/**
 * Whether sending `entry` again cannot post what may already have landed: every attempt so far
 * carried its key to a provider that honours it (`postsOnce`, by the server's account of `entry`'s
 * channel).
 *
 * Not for a remainder that starts with the reply, though. After a 207 the unsent text goes again
 * with the reply target, which makes its first part a different request from the part that failed
 * — and that part is exactly the one whose arrival nobody knows. (The server's 207 says so as well,
 * by not calling that remainder `resumable`; this does not depend on it.) A remainder continuing in
 * the first part's thread goes without the reply target, as the part that failed did, so it is not
 * one of those. `#263 post-split-per-provider`.
 */
function retriesItself(entry: RetryFacts, postsOnce: (channel: string) => boolean): boolean {
  return entry.keyedOnly === true && postsOnce(entry.channel) &&
    !((entry.postedCount as number) > 0 && entry.replyTo && !entry.continueIn);
}

/**
 * The wait before automatic attempt `used` + 1, from when the attempt before it started: 3 s, 6 s,
 * 12 s, then OUTGOING_RETRY_MAX_MS. `#277 send-order`.
 */
function outgoingRetryWait(used: number): number {
  return Math.min(OUTGOING_RETRY_MAX_MS, OUTGOING_RETRY_FIRST_MS * OUTGOING_RETRY_FACTOR ** used);
}

/**
 * After a transient failure of `entry`'s attempt that started at `startedAt`: when its next automatic
 * attempt goes, at `now` or later, or null to leave the row to the reader.
 *
 * `unposted`: the attempt posted nothing — finding out where a reply goes — so going again is safe
 * whether or not the provider honours a key, and spends one of the same OUTGOING_RETRY_ATTEMPTS.
 */
function nextRetryAt(
  entry: RetryFacts,
  startedAt: number,
  now: number,
  postsOnce: (channel: string) => boolean,
  unposted = false,
): number | null {
  const used = entry.retriesUsed || 0;
  if ((!unposted && !retriesItself(entry, postsOnce)) || used >= OUTGOING_RETRY_ATTEMPTS) return null;
  return Math.max(startedAt + outgoingRetryWait(used), now + OUTGOING_RETRY_FLOOR_MS);
}

export { OUTGOING_RETRY_ATTEMPTS, nextRetryAt, outgoingRetryWait, retriesItself };

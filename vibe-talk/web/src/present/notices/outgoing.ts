// What an unsent row says (`#195 send-resilience`, `#430 threads-main-own-sent`;
// `#624 voice-page-modularity`, moved from the legacy script into present/notices;
// mu-readcore): why it waits, what goes first, and how long a send took. Pure over
// snapshots: the blocker, the retry hold and whether the answered message was removed
// are read by the caller and passed in, with the time.

import { fmtDuration } from "./counts.ts";

/** A pending send as its row's words read it. Unknown saved fields ride along unread. */
export interface OutgoingStatusEntry {
  id: string;
  channel: string;
  state: string;
  phase?: string;
  text: string;
  remaining?: string;
  postedCount?: number;
  mayHavePosted?: boolean;
  sentAt?: number;
  parts: readonly { readonly id: string }[];
  observedParts: readonly string[];
  placement?: string;
  replyTo?: string | null;
  detail?: string;
  progress?: { posted: number; parts: number } | null;
  confirming?: boolean;
  serverMs?: number | null;
  upstreamMs?: number | null;
  sendStartedAt?: number;
  createdAt?: number;
};

/** An armed retry as the words read it. */
export interface OutgoingRetryHold {
  at: number;
  offline: boolean;
  unsent: boolean;
}

export function outgoingStatusWords(entry: OutgoingStatusEntry, blocker: OutgoingStatusEntry | null, retry: OutgoingRetryHold | undefined, blockerRetry: OutgoingRetryHold | undefined, blockerRemoved: boolean, now: number, blockerEditable: boolean, canRetry: boolean, blockerSnippet: string): string {
  if (entry.state === "sending") {
    if (entry.phase === "queued") {
      const ahead = blocker;
      return ahead ? `Waiting to send… ${waitingBehindWords(ahead, blockerRetry, blockerRemoved, now, blockerEditable, blockerSnippet)}` : "Waiting to send…";
    }
    if (entry.phase === "placing") return "Finding out which thread the message you answered is in…";
    // `#448 send-dedupe`. A slow send says what it is doing, so it never looks stuck: which part is
    // going, for a post cut in several, and after a lost answer, that it is finding out.
    const progress = entry.progress;
    if (progress && progress.parts > 1) {
      return `Posting part ${Math.min(progress.posted + 1, progress.parts)} of ${progress.parts}…`;
    }
    return entry.confirming ? "Sending… Slack is slow to confirm…" : "Sending…";
  }
  if (entry.state === "sent") return "Sent.";
  const prefix = (entry.postedCount || 0) > 0
    ? `${entry.postedCount} part${entry.postedCount === 1 ? "" : "s"} confirmed. ` : "";
  // `#195 send-resilience`. Waiting to go again on its own: say so, and when, so a row that is
  // about to fix itself does not read as a failure the reader has to act on.
  const waiting = retry;
  if (waiting && waiting.offline) {
    return waiting.unsent ? `${prefix}Offline. Will send when online.` : `${prefix}Sending… Will retry when online.`;
  }
  if (waiting) {
    const seconds = Math.max(1, Math.round((waiting.at - now) / 1000));
    // The cause is often a provider's clause without a full stop; another sentence follows it here.
    const cause = entry.detail && !/[.!?…]$/.test(entry.detail.trim()) ? `${entry.detail.trim()}.` : entry.detail;
    return `${prefix}Sending… ${cause} Retrying in ${seconds} s.`.replace(/\s+/g, " ");
  }
  if (entry.state === "unconfirmed") {
    return canRetry
      ? `${prefix}Delivery unconfirmed. Retrying cannot post it twice. ${entry.detail}`
      : `${prefix}Delivery unconfirmed. Check history before retrying to avoid a duplicate. ${entry.detail}`;
  }
  return `${prefix}Not sent. ${entry.detail}`;
}

/**
 * `#430 threads-main-own-sent`. The messages this page has sent, by id, for as long as it is open. The
 * owner, 2026-10-22, on a phone: "even when I've toggled Main on in thread list View on mobile. I still
 * don't see the message that I just sent to the main channel." The owner's own messages count as read
 * (`ownReadSwitch`), so under Hide read the Threads view's main rows, rebuilt through the read filter, left
 * it out; Main showed it only because its live append skips that filter, until its next redraw. So a
 * message sent from here is one the read filters keep (`readFilterKeeps`, `mainRowMessages`), in every
 * list alike, for the rest of the page's life: what the reader has just said never vanishes as they say it.
 *
 * `#466 reply-promotes-thread`, `#500 sent-receipts-pile-up`. When each was acknowledged, too, kept on
 * the device for OWN_POST_GRACE_MS, so a reload in the grace keeps the page's own post against a read
 * from older state as the page that sent it did (`ownPostStands`). The owner is web/src/core/outbox
 * `createOwnPosts` (`#624 voice-page-modularity`).
 *
 * @type {import("./src/core/outbox/index.ts").OwnPosts}
 */

export function waitingBehindWords(ahead: OutgoingStatusEntry, retry: OutgoingRetryHold | undefined, removed: boolean, now: number, aheadEditable: boolean, aheadSnippet: string): string {
  const which = `“${aheadSnippet}” goes first`;
  if (ahead.state === "sending") {
    return ahead.placement === "unknown" ? `${which}, once it is known which thread it goes in.`
      : ahead.phase === "queued" ? `${which}, and is waiting its turn too.` : `${which}, and is still sending.`;
  }
  const waiting = retry;
  if (waiting && waiting.offline) return `${which}, when this device is online.`;
  if (waiting) {
    const seconds = Math.max(1, Math.round((waiting.at - now) / 1000));
    return `${which}, and goes again in ${seconds} s.`;
  }
  // Only the actions its row offers, so the reader is never sent to a button that is not there.
  const actions = ["Retry", ...(aheadEditable ? ["Edit"] : [])];
  const choice = `${actions.join(", ")} or Dismiss it to let this one go.`;
  if (ahead.state === "unconfirmed") return `${which}, and its delivery is unconfirmed. ${choice}`;
  // `#315 removed-reply-target`. Refused, not undecided: Retry sends it nowhere either.
  if (ahead.placement === "unknown" && ahead.replyTo && removed) {
    return `${which}, and was not sent: the message it answers was removed. ` +
      `${aheadEditable ? "Edit or Dismiss" : "Dismiss"} it to let this one go.`;
  }
  return ahead.placement === "unknown"
    ? `${which}, and was not sent: which thread it goes in is not known yet. ${choice}`
    : `${which}, and was not sent. ${choice}`;
}

/**
 * Put `entry`, just queued or retried, in the outbox's order to go, without making the composer wait
 * for its destination. Answers a promise settled when this attempt is over — sent, failed, or no
 * longer in the outbox — which may be long after it was queued: it waits behind what was written
 * before it.
 */

export function sendTimingTextWords(entry: OutgoingStatusEntry, service: string, now: number): string {
  const server = entry.serverMs;
  const upstream = entry.upstreamMs;
  const total = server !== null && server !== undefined && upstream !== null && upstream !== undefined ? server + upstream
    : Math.max(0, now - (entry.sendStartedAt || entry.createdAt || now));
  const up = upstream === null ? "…" : `+${fmtDuration(upstream)}`;
  return `server ${fmtDuration(server)} · ${service} ${up} · total ${fmtDuration(total)}`;
}

/** Whether `error` means the server's answer was lost rather than given. */

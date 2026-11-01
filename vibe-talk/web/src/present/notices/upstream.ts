// Marking a channel read in the chat service's own app, in the reader's words (`#624
// voice-page-modularity`, moved from the legacy script into present/notices): the status line, and
// what a press of "Mark read now" came to.

import type { UpstreamClearNowResponse, UpstreamClearState } from "../../contract/index.ts";
import { shortTimeFormat } from "../time/index.ts";

/** A millisecond instant as the reader's short local time. */
function upstreamClearClock(ms: unknown): string {
  return typeof ms === "number" && Number.isFinite(ms) ? shortTimeFormat().format(new Date(ms)) : "";
}

/** An instant the provider wrote, as the reader's short local time, or as written if unreadable. */
function upstreamClearCursor(text: unknown): string {
  const at = Date.parse(String(text || ""));
  return Number.isFinite(at) ? upstreamClearClock(at) : String(text || "");
}

/**
 * The status line: what the server last did for this channel, or what it will do next.
 */
function upstreamClearSentence(view: UpstreamClearState, service: string): string {
  if (view.unavailable) {
    return `Not available for this channel: ${view.unavailable}.`;
  }
  const last = view.last || null;
  const pushed = view.pushed || null;
  const undo = view.previous_cursor
    ? ` Before vibe-talk first marked it, ${service}'s read marker was at ` +
      `${upstreamClearCursor(view.previous_cursor)}.`
    : "";
  const through = pushed
    ? `vibe-talk marked it read in ${service} through the message from ` +
      `${upstreamClearClock(pushed.message_at_ms)}, at ${upstreamClearClock(pushed.at_ms)}.`
    : "";
  if (view.mode === "off") {
    return pushed ? `Off. Earlier, ${through}${undo}` : `Off. Nothing here is marked read in ${service}.`;
  }
  if (last && last.result === "failed") {
    const retry = view.retry_at_ms ? ` Trying again at ${upstreamClearClock(view.retry_at_ms)}.` : "";
    return `Failing since ${upstreamClearClock(last.at_ms)}: ${last.error || "the chat service did not answer"}.${retry}`;
  }
  const waiting = view.pending ? " A newer message is waiting to be marked." : "";
  if (view.mode === "dry_run") {
    if (last && last.result === "dry_run") {
      return `Dry run: would mark it read in ${service} through the message from ` +
        `${upstreamClearClock(last.message_at_ms)}. Nothing was written.${waiting}`;
    }
    return `Dry run. The server logs what it would mark read in ${service} and writes nothing.${waiting}`;
  }
  if (last && last.result === "skipped" && !pushed) {
    return `On. The newest message had already left the bridge's memory; the next one is marked.${waiting}`;
  }
  if (pushed) return `On. ${through}${waiting}${undo}`;
  return view.pending
    ? `On. The newest message, and everything before it, is marked read in ${service} shortly.`
    : `On. When the next message arrives here, it and everything before it are marked read in ${service}.`;
}

/**
 * The status line after "Mark read now" (or "Try it now"): what the press came to.
 * `#253 upstream-clear-polish`.
 *
 * What the push did is said from `answer.attempt`, the attempt that answered THIS press, and never
 * from `state.last`, which a later push may already have replaced: "Done" only for a push the
 * service took, a failure as a failure, and a setting changed meanwhile as that.
 */
function upstreamClearNowSentence(answer: UpstreamClearNowResponse, service: string): string {
  const view = answer.state;
  const attempt = answer.attempt || null;
  const last = view.last || null;
  const dryRun = view.mode === "dry_run";
  switch (answer.outcome) {
    case "pushed":
      if (!attempt) break;
      if (attempt.result === "advanced") {
        return `Done: marked read in ${service} through the message from ${upstreamClearClock(attempt.message_at_ms)}.`;
      }
      if (attempt.result === "already_read") {
        return `Done: ${service} already had it read through the message from ` +
          `${upstreamClearClock(attempt.message_at_ms)}, so nothing needed moving.`;
      }
      if (attempt.result === "dry_run") {
        return `Dry run: would mark it read in ${service} through the message from ` +
          `${upstreamClearClock(attempt.message_at_ms)}. Nothing was written.`;
      }
      break;
    case "failed": {
      const why = attempt && attempt.error ? attempt.error : `${service} did not answer`;
      const when = view.retry_at_ms ? ` It is tried again by itself at ${upstreamClearClock(view.retry_at_ms)}.` : "";
      return `Not done: the push failed: ${why}.${when}`;
    }
    case "skipped":
      return "Nothing was marked: the bridge no longer holds the newest message. The next message that arrives is marked.";
    case "changed":
      return `Not finished: this channel's setting changed while the push was on its way. ${upstreamClearSentence(view, service)}`;
    case "covered":
      return dryRun
        ? "Nothing newer to try: the dry run already covered the newest message here. Nothing was written."
        : `Nothing newer to mark: the newest message here is already marked read in ${service}.`;
    case "no_message":
      return dryRun ? "There is no message here to try it on yet." : "There is no message here to mark read yet.";
    case "waiting":
      return `Still waiting for ${service} to answer. The server carries on without this page, ` +
        "and this line shows what it came to when it does.";
    case "retry_waiting": {
      const why = last && last.result === "failed" && last.error ? ` (${last.error})` : "";
      const when = view.retry_at_ms ? `, at ${upstreamClearClock(view.retry_at_ms)}` : "";
      return `Not sent yet: the last try failed${why}, so the newest message goes with the next one${when}.`;
    }
    default:
      break;
  }
  return upstreamClearSentence(view, service);
}

export { upstreamClearClock, upstreamClearCursor, upstreamClearNowSentence, upstreamClearSentence };

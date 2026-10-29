// An outbox entry as the device saved it (`#193 compose-draft-loss`, `#277 send-order`, `#448
// send-dedupe`, `#500 sent-receipts-pile-up`, `#621 send-double-check`; `#624 voice-page-modularity`,
// moved from the legacy script; plan step 12): in the outbox, or among the recently dismissed, read
// back as an entry the outbox can hold, or as nothing for anything that is not one.

/** Where an entry's words went on the first attempt that sent them (`dispatchOutgoingMessage`). */
export interface OutgoingWire {
  readonly text: string;
  readonly replyTo: string | null;
  readonly threadId: string | null;
}

/** One part of a post the server confirmed. */
export interface OutgoingPart {
  readonly id: string;
  readonly content: string;
  readonly [field: string]: unknown;
}

/** An outbox entry read back from the device: never mid-send, as the page that sent it has closed. */
export interface OutgoingEntry {
  id: string;
  channel: string;
  threadId: string | null;
  placement: "unknown" | null;
  replyTo: string | null;
  marksRead: string | null;
  originView: "main" | "threads" | "thread" | "flat";
  text: string;
  remaining: string;
  state: "sent" | "failed" | "unconfirmed";
  phase: string;
  createdAt: number;
  postedCount: number;
  parts: OutgoingPart[];
  observedParts: string[];
  keyedOnly: boolean;
  mayHavePosted: boolean;
  continueIn: string | null;
  wire: OutgoingWire | null;
  sentAt: number;
  sendStartedAt: number;
  serverMs: number | null;
  upstreamMs: number | null;
  serverTick: boolean;
  upstreamTick: boolean;
  tickFailed: boolean;
  showTimes: boolean;
  detail: string;
}

/** A record read back from the device, whose every field is checked where it is read. */
type Saved = { readonly [field: string]: unknown };

/** The saved states of an entry: `sending` is one the page closed on. */
const SAVED_STATES = ["sending", "sent", "failed", "unconfirmed"] as const;
const ORIGIN_VIEWS = ["main", "threads", "thread", "flat"] as const;

/** Whether `value` is one of `allowed`. */
function oneOf<T extends string>(allowed: readonly T[], value: unknown): value is T {
  return (allowed as readonly unknown[]).includes(value);
}

/** A saved entry's `wire`, or null for anything that is not one. */
export function savedWire(wire: unknown): OutgoingWire | null {
  const place = (value: unknown): string | null => (typeof value === "string" && value ? value : null);
  const held = wire as Saved | null | undefined;
  return held && typeof held.text === "string"
    ? { text: held.text, replyTo: place(held.replyTo), threadId: place(held.threadId) } : null;
}

/**
 * An outbox entry as it was saved — in the outbox, or among the recently dismissed — or null for
 * anything that is not one. `now` is the wall clock, asked only where a saved time is missing.
 */
export function outgoingFromSaved(saved: unknown, now: () => number): OutgoingEntry | null {
  const held = saved as Saved | null | undefined;
  if (!held || typeof held.id !== "string" || typeof held.channel !== "string" ||
      !held.channel || typeof held.text !== "string" || !oneOf(SAVED_STATES, held.state)) return null;
  const savedState = held.state;
  const interrupted = savedState === "sending";
  // The rule signing out goes by: only a POST that had started can have been delivered. One
  // still waiting its turn, or still finding out where its reply goes, was posted nowhere.
  const attempted = interrupted && !oneOf(["queued", "placing"], held.phase);
  return {
    id: held.id, channel: held.channel,
    threadId: typeof held.threadId === "string" ? held.threadId : null,
    // A reply whose destination was still to be found out keeps finding it out on Retry.
    placement: held.placement === "unknown" ? "unknown" : null,
    replyTo: typeof held.replyTo === "string" ? held.replyTo : null,
    // `#300 reply-marks-read`. What it marks read once sent. An entry saved before this was kept
    // marks what it answers, as it always did.
    marksRead: typeof held.marksRead === "string" ? held.marksRead
      : held.marksRead === null ? null : typeof held.replyTo === "string" ? held.replyTo : null,
    originView: oneOf(ORIGIN_VIEWS, held.originView) ? held.originView : "main",
    text: held.text,
    remaining: typeof held.remaining === "string" ? held.remaining : held.text.trim(),
    state: attempted ? "unconfirmed" : savedState === "sending" ? "failed" : savedState,
    phase: "", createdAt: Number(held.createdAt) || now(),
    postedCount: Math.max(0, Number(held.postedCount) || 0),
    parts: Array.isArray(held.parts) ? (held.parts as unknown[]).filter((part): part is OutgoingPart =>
      Boolean(part) && typeof (part as Saved).id === "string" && typeof (part as Saved).content === "string") : [],
    observedParts: Array.isArray(held.observedParts)
      ? (held.observedParts as unknown[]).filter((id): id is string => typeof id === "string") : [],
    // Absent from an entry saved before keys were sent: its attempts went without one.
    keyedOnly: held.keyedOnly === true,
    // `#193 compose-draft-loss`. See `editableSend`.
    mayHavePosted: held.mayHavePosted === true || savedState === "unconfirmed" || attempted,
    // `#263 post-split-per-provider`: where the rest of a post cut in parts goes.
    continueIn: typeof held.continueIn === "string" && held.continueIn ? held.continueIn : null,
    // `#448 send-dedupe`: where its words went on the first attempt that sent them.
    wire: savedWire(held.wire),
    // `#500 sent-receipts-pile-up`: when the server confirmed it, for a send that is sent.
    sentAt: Number(held.sentAt) || 0,
    // `#621 send-double-check`.
    sendStartedAt: Number(held.sendStartedAt) || Number(held.createdAt) || now(),
    serverMs: held.serverMs != null && Number.isFinite(Number(held.serverMs)) ? Number(held.serverMs) : null,
    upstreamMs: held.upstreamMs != null && Number.isFinite(Number(held.upstreamMs)) ? Number(held.upstreamMs) : null,
    serverTick: held.serverTick === true,
    upstreamTick: held.upstreamTick === true,
    tickFailed: held.tickFailed === true,
    showTimes: false,
    detail: attempted ? "This page closed before delivery was confirmed."
      : interrupted ? "This page closed before this message was sent." : String(held.detail || ""),
  };
}

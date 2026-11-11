// Settings' message limits and connection/recap wording (`#624 voice-page-modularity`).
// Pure presentation: feature state and every redraw remain with their existing owners.

import { cardMaxLinesWords, collapsedLinesWords } from "../rows/index.ts";

export const collapsedLinesSavedWords = (n: number, kept: boolean): string => kept
  ? `Saved — collapsed messages show ${collapsedLinesWords(n)}.`
  : "This browser refused to store the collapsed lines, so it will be forgotten when you " +
    `reload (private browsing does this). Collapsed messages show ${collapsedLinesWords(n)} until then.`;

export const cardMaxLinesSavedWords = (n: number, kept: boolean): string => kept
  ? `Saved — thread cards show at most ${cardMaxLinesWords(n)} of a first message.`
  : "This browser refused to store the thread card lines, so they will be forgotten when you " +
    `reload (private browsing does this). Thread cards show at most ${cardMaxLinesWords(n)} until then.`;

export function quoteReadPercentSavedWords(n: number, kept: boolean): string {
  const rule = n === 0
    ? "a reply marks the message it answers read, whatever it quotes."
    : `a reply that quotes less than ${n}% of a message leaves it unread.`;
  return kept
    ? `Saved — ${rule}`
    : `This browser refused to store it, so it will be forgotten when you reload (private browsing does this). Until then ${rule}`;
}

/** The status sentences spoken only after the corresponding feature has applied a choice. */
export const MESSAGE_PREFERENCE_WORDS = {
  hideEmpty: (on: boolean): string => on
    ? "messages with nothing in them are hidden."
    : "messages with nothing in them are shown.",
  combine: (on: boolean): string => on
    ? "messages sent seconds apart are shown as one."
    : "every source message is shown as its own row.",
  richHeaders: (on: boolean): string => on
    ? "an agent's tag, title and time are shown in the message header."
    : "every message's first line is shown as it was written.",
  inAppLinks: (on: boolean): string => on
    ? "links to messages open here."
    : "links to messages open the chat service.",
  openInView: (view: string): string => view === "main"
    ? "Open in channel lands in Main."
    : "Open in channel lands in All, every message in order.",
  afterDone: (choice: string): string => choice === "next"
    ? "After Done, e goes to the next message."
    : choice === "unread"
      ? "After Done, e goes to the next unread message."
      : "After Done, e stays on the message.",
  summaries: (on: boolean): string => on
    ? "collapsed messages show a summary when one arrives."
    : "collapsed messages show their opening lines; nothing is summarised.",
} as const;

/** What the last call was given, owned by the voice feature. */
export interface ResumeOutcome {
  readonly attempted: boolean;
  readonly armed: boolean;
  readonly dropped: number;
  readonly failed: boolean;
  readonly words: number | null;
  readonly acked: boolean;
  readonly refused: boolean;
  readonly unsupported: boolean;
  readonly unsent: boolean;
  readonly replaced: boolean;
}

/** Readers stay lazy: an off server must not read a stored preference to word its line. */
export interface ResumeReadout {
  readonly allowed: boolean;
  readonly last: ResumeOutcome;
  readonly conversationId: string | null;
  readonly replayUnavailable: string | null;
  wanted(): boolean;
}

/** One short line per state, including an empty transcript and one entirely over budget. */
export function resumeStateWords(read: ResumeReadout): string {
  const lines = (count: number): string => (count === 1 ? "1 earlier line" : `${count} earlier lines`);
  // `#331 resume-on-websocket`. How much the last call was given, in the server's own count.
  const words = typeof read.last.words === "number" && read.last.words > 0
    ? `${read.last.words.toLocaleString("en-US")} word${read.last.words === 1 ? "" : "s"}`
    : null;
  if (!read.allowed && read.replayUnavailable) {
    // `#331 resume-on-websocket`. Turned on by the operator, and not possible here: the reason is
    // beside the switch.
    return "Resuming is unavailable on this server: every call starts fresh.";
  } else if (!read.allowed) {
    return "Resuming is OFF on this server: every call starts fresh.";
  } else if (!read.wanted()) {
    return "Resuming is off: a new call starts fresh.";
  } else if (read.last.failed) {
    return "The last call could NOT be resumed: the record would not load.";
  } else if (read.last.refused) {
    return "The last call was NOT resumed: the voice service refused the recap.";
  } else if (read.last.unsupported) {
    return "The last call was NOT resumed: its voice session took no recap.";
  } else if (read.last.unsent) {
    return "The last call ended before it could be told what was already said.";
  } else if (read.last.replaced) {
    return "The last call's voice session was replaced; the new one was sent no recap.";
  } else if (read.last.armed && read.last.dropped > 0) {
    return `The last call was resumed IN PART: ${words ? `${words}; ` : ""}` +
      `${lines(read.last.dropped)} did not fit.`;
  } else if (read.last.armed && words) {
    return read.last.acked
      ? `The last call was resumed: the agent took ${words} from earlier.`
      : `The last call was resumed: it was sent ${words} from earlier.`;
  } else if (read.last.armed) {
    return "The last call was resumed.";
  } else if (read.last.attempted && read.last.dropped > 0) {
    // Not "there was nothing to replay". There was; none of it fitted. Reporting that as an empty
    // conversation would tell the reader something false about a call they remember having.
    return (
      `The last call started fresh: ${read.last.dropped === 1 ? "the" : "all"} ` +
      `${lines(read.last.dropped)} ${read.last.dropped === 1 ? "was" : "were"} over the budget.`);
  } else if (read.last.attempted) {
    return "The last call started fresh: there was nothing to replay.";
  } else if (!read.conversationId) {
    return "Resuming is on. Nothing to resume from yet.";
  } else {
    return "Resuming is on: the next call will be told what was already said.";
  }
}

/** Why this server cannot resume, as the line beside the disabled switch says it. */
export function resumeUnavailableWords(unavailable: string | null): string {
  switch (unavailable) {
    case null:
      return "Unavailable: resuming is turned off in this server's configuration.";
    case "bridge_declined":
      return "Unavailable: this server's voice service does not take a recap.";
    case "bridge_unconfirmed":
      return "Unavailable: this server's voice service has not said it takes a recap.";
    default:
      return "Unavailable: this server's voice service cannot take a recap.";
  }
}

/** The live channel's reported transport, never inferred from its silence. */
export interface LiveReadout {
  readonly delivery: string;
  readonly pollSeconds: number;
  attached(): boolean;
}

/** Off, listening, or disconnected; adapter push does not establish the adapter's health. */
export function liveStateWords(read: LiveReadout): string {
  if (read.delivery === "off") {
    return "Live updates are OFF on this server.";
  }
  if (read.delivery === "push") {
    return read.attached()
      ? "Live by adapter push: listening, but the adapter's health is not checked."
      : "Live by adapter push, but not connected right now; it keeps trying.";
  }
  return read.attached()
    ? `Live: this server re-reads the channel every ${read.pollSeconds} seconds.`
    : `Live every ${read.pollSeconds} seconds, but not connected to the stream; it keeps trying.`;
}

/** Connection status gives each part its own line; its transient banner keeps the original line. */
export const connectionDetailWords = (text: string): string => text.split(" · ").join("\n");

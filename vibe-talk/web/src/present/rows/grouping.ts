// Which messages are drawn as one row (`#624 voice-page-modularity`, moved from the legacy script into
// present/rows): a coding agent's post its client split in two, or any two the same author sent within
// a few seconds in the same thread, shown together (the legacy script's "two messages that are really
// one" note). Only drawn: the data model stays one-to-one with the chat service.

import type { Message } from "../../contract/index.ts";
import { threadOf } from "../../core/channels/index.ts";
import { messageDate } from "../time/index.ts";

/** How close two messages have to be to be shown as one. */
const GLOM_WINDOW_MS = 5000;

/**
 * Does the SERVER say this message is read automatically? `#196 auto-read-noise`.
 *
 * The flag and nothing else. The server decides it from the owner's noise rules against the
 * message's text as it reads NOW, on every read, and this page deliberately has no copy of the
 * matcher: two predicates would disagree the first time either one changed, and the one that
 * decides what the agent hears is the server's.
 *
 * Like `markOwnRead` and unlike the declared archive, it is implied: nothing is recorded per
 * message, there is no undo, and an edit or a removed rule brings the message straight back.
 */
function isNoise(message: Pick<Message, "noise"> | null | undefined): boolean {
  return Boolean(message) && (message as Pick<Message, "noise">).noise === true;
}

/**
 * Does `message` belong with the one before it? Never while the reader has turned combining off
 * (the Combine messages switch, `DEVICE_SETTINGS.combineMessages`, passed in).
 *
 * A missing or unparseable timestamp is a NO. Guessing that two messages were sent together when
 * the page cannot tell when either was sent would be combining on no evidence at all.
 */
function joinsGroup(previous: Message | null | undefined, message: Message, combineMessages: boolean): boolean {
  if (!combineMessages || !previous) {
    return false;
  }
  if (threadOf(previous) !== threadOf(message) || (message.thread && message.thread.is_root)) {
    return false;
  }
  if (String(previous.author_id || "") !== String(message.author_id || "")) {
    return false;
  }
  // `#196 auto-read-noise`. A placeholder and the answer that follows it seconds later are two
  // rows, not one: combined, the row would be neither read nor quiet, and the placeholder's text
  // would sit at the head of the real message.
  if (isNoise(previous) !== isNoise(message)) {
    return false;
  }
  const before = messageDate(previous);
  const after = messageDate(message);
  if (before === null || after === null) {
    return false;
  }
  const gap = after.getTime() - before.getTime();
  return gap >= 0 && gap <= GLOM_WINDOW_MS;
}

export { GLOM_WINDOW_MS, isNoise, joinsGroup };

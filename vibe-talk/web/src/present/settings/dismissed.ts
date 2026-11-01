// What Settings' Recently dismissed says about each send it keeps (`#193 compose-draft-loss`, `#348
// settings-dynamic-lists`, `#351 settings-polish`; `#624 voice-page-modularity`, moved from the legacy
// script into present/settings). How many are kept and for how long is web/src/core/outbox's; the list
// and its pop-out are web/src/view/settings's.

import { DISMISSED_SENDS_DAYS, DISMISSED_SENDS_LIMIT, sendSnippet, unsentText } from "../../core/outbox/index.ts";
import type { DismissedSend, SendState } from "../../core/outbox/index.ts";
import { briefAge } from "../time/index.ts";

/**
 * `#351 settings-polish`. How much of a message's first line names its row in Recently dismissed to a
 * screen reader (`dismissedRowLabel`): a line, not a paragraph.
 */
const DISMISSED_LABEL_CHARS = 120;

/** A send the outbox gave up, as Recently dismissed reads it: where it was going, and what it says. */
type DismissedEntry = SendState & {
  readonly id: string | number;
  readonly channel: string;
  readonly replyTo?: unknown;
  readonly threadId?: unknown;
};

/**
 * Where a dismissed send was going, and how long ago it was dismissed, as one line. `channelLabel`
 * names a channel by id.
 */
function dismissedSendMeta(held: DismissedSend<DismissedEntry>, channelLabel: (id: string) => string): string {
  const entry = held.entry;
  const age = briefAge(held.at);
  const when = age === "now" ? "just now" : `${age} ago`;
  return [channelLabel(entry.channel), entry.replyTo ? "Reply" : entry.threadId ? "Thread" : "", `dismissed ${when}`]
    .filter(Boolean).join(" · ");
}

/**
 * What a row of Recently dismissed is called to a screen reader: its place and age, and the first line
 * of its text, cut to DISMISSED_LABEL_CHARS.
 *
 * `#351 settings-polish`. A button is named by everything inside it unless it is given a name, and the
 * row holds the whole message, for find-in-page: so each row passed read a whole message out, where the
 * eye sees one line. The pop-out the row opens is where the whole message is read.
 */
function dismissedRowLabel(held: DismissedSend<DismissedEntry>, channelLabel: (id: string) => string): string {
  const first = String(unsentText(held.entry)).split("\n").find((line) => line.trim()) || "";
  return [dismissedSendMeta(held, channelLabel), sendSnippet(first, DISMISSED_LABEL_CHARS)].filter(Boolean).join(": ");
}

/** The line under Recently dismissed: what it keeps and for how long, with `count` kept now. */
const dismissedListWords = (count: number): string => count
  ? `Messages you dismissed from the outbox unsent, kept on this device for ${DISMISSED_SENDS_DAYS} days ` +
    `(the newest ${DISMISSED_SENDS_LIMIT}). Open one to read it whole, copy it or put it back.`
  : `Nothing dismissed in the last ${DISMISSED_SENDS_DAYS} days. A message you dismiss from the ` +
    "outbox unsent is kept here that long, in case you want it back.";

/** What the pop-out's Put back is called: the message it puts back, in a few words. */
const dismissedRestoreLabel = (entry: DismissedEntry): string => `Put back “${sendSnippet(unsentText(entry))}”`;

export { DISMISSED_LABEL_CHARS, dismissedListWords, dismissedRestoreLabel, dismissedRowLabel, dismissedSendMeta };
export type { DismissedEntry };

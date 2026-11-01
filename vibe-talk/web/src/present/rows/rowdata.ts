// What a row is drawn from beyond its words (`#624 voice-page-modularity`, moved from the legacy script
// into present/rows): a combined row's text, when its messages were last edited (`#320
// slack-edited-mark`), and its reactions tallied per emoji (`#219 emoji-reactions`), with the accounts
// of both that the redraw keys hold.

import type { Message } from "../../contract/index.ts";
import { reactionKey, reactionText } from "./labels.ts";

/** One emoji's tally on a row: its face, whether custom, how many, and whether the owner is among them. */
interface ReactionTally {
  emoji: string;
  custom: boolean;
  count: number;
  own: boolean;
}

/** What goes between two combined messages, so the seam is a line break and not a run-on. */
const GLOM_SEPARATOR = "\n";

/** What a combined row DISPLAYS: the constituents, in order, separated by a line break. */
const combinedContent = (messages: ReadonlyArray<Pick<Message, "content">>): string =>
  messages
    .map((m) => String(m.content === null || m.content === undefined ? "" : m.content))
    .join(GLOM_SEPARATOR);

/**
 * When a row's messages were last edited: the latest edit among them, `at` null when none of the edits
 * says a time a Date can read; or null when none of them was edited.
 */
function rowEdited(messages: ReadonlyArray<Pick<Message, "edited_timestamp"> | null | undefined>): { at: Date | null } | null {
  let edited: { at: Date | null } | null = null;
  for (const message of messages) {
    const raw = message && message.edited_timestamp ? String(message.edited_timestamp) : "";
    if (!raw) continue;
    const at = new Date(raw);
    const readable = Number.isNaN(at.getTime()) ? null : at;
    if (edited === null) edited = { at: readable };
    else if (readable && (edited.at === null || readable.getTime() > edited.at.getTime())) edited.at = readable;
  }
  return edited;
}

/** What the redraw keys hold of a message's edit, so a row that becomes edited is drawn again. */
const editedDrawn = (message: Pick<Message, "edited_timestamp"> | null | undefined): string | null =>
  (message && message.edited_timestamp ? String(message.edited_timestamp) : null);

/**
 * A row's reactions: one tally per emoji, in the order first seen, counted over every message the
 * row stands for, and the owner's when the owner reacted with it to any of them. Empty when none
 * has any, whether its source reported none or cannot see them.
 */
function rowReactions(messages: ReadonlyArray<Pick<Message, "reactions">>): ReactionTally[] {
  const tallies = new Map<string, ReactionTally>();
  for (const message of messages) {
    for (const reaction of Array.isArray(message.reactions) ? message.reactions : []) {
      const count = reaction ? Number(reaction.count) : 0;
      if (!reaction || typeof reaction.emoji !== "string" || reaction.emoji === "" || !(count > 0)) continue;
      const key = reactionKey(reaction);
      const own = reaction.owner_reacted === true;
      const held = tallies.get(key);
      if (held) {
        held.count += count;
        held.own = held.own || own;
      } else tallies.set(key, { emoji: reaction.emoji, custom: reaction.custom === true, count, own });
    }
  }
  return [...tallies.values()];
}

/** A stable account of a message's reactions, for the lists that redraw only when a row changed. */
const reactionsDrawn = (message: Pick<Message, "reactions">): Array<[string, number, boolean]> =>
  rowReactions([message]).map((tally) => [reactionText(tally), tally.count, tally.own]);

export { GLOM_SEPARATOR, combinedContent, editedDrawn, reactionsDrawn, rowEdited, rowReactions };
export type { ReactionTally };

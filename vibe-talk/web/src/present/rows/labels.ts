// What a row and its lines say (`#624 voice-page-modularity`, moved from the legacy script into
// present/rows): what a fold hides, the lines that stand for gaps and runs of read messages, the
// Undo chip's words, a reaction's face and key, and Settings' line counts.

import type { Reaction } from "../../contract/index.ts";

/** What a reaction is shown and known by: a tally from the server, or one the page drew. */
type ReactionFace = Pick<Reaction, "custom" | "custom_id" | "emoji">;

/** The fewest hidden words a label says: under that, a floor cannot say any are hidden. */
const ELIDED_MIN_WORDS = 10;

/** ...and the fewest characters, for text counted in characters: about as much as ten words. */
const ELIDED_MIN_CHARS = 60;

/** Characters to a hidden word past which the text is counted in characters rather than words. */
const SPACE_POOR_CHARS = 12;

/**
 * The label for what a fold hides, `{words, chars}`: "(~240 more words)", "(~400 more characters)"
 * for text poor in spaces, or "" when it is too little to say anything is hidden.
 */
function elisionLabel(hidden: { readonly words: number; readonly chars: number }): string {
  const spacePoor = hidden.chars > SPACE_POOR_CHARS * Math.max(1, hidden.words);
  const count = spacePoor ? hidden.chars : hidden.words;
  if (count < (spacePoor ? ELIDED_MIN_CHARS : ELIDED_MIN_WORDS)) return "";
  const step = count < 20 ? 1 : count < 100 ? 5 : count < 1000 ? 10 : 100;
  return `(~${Math.floor(count / step) * step} more ${spacePoor ? "characters" : "words"})`;
}

/**
 * A line's words: how many messages it stands for, and the days they reach when they reach any
 * (`#250 date-separators`) — "… 12 unmatched messages · Oct 3 – 5 …". With days, each side of the `·`
 * is held together, so a line too narrow for both — a phone 360px wide, the day said in full — wraps
 * between the count and the days and nowhere else: "… 2 unmatched messages ·" over "Today, Oct 12,
 * Mon …", never a lone "…" left on a line of its own. Held by no-break spaces, and a word joiner after
 * a range's dash, which the line breaking rules otherwise let a line end on even before a no-break
 * space: "Oct 9 –" over "Yesterday …".
 */
function lineWords(count: string, days: string): string {
  if (!days) return `${count} …`;
  const held = (words: string): string => words.replace(/ /g, "\u00a0").replace(/–/g, "–\u2060");
  return `${held(count)}\u00a0· ${held(days)}\u00a0…`;
}

/** What a screen reader is given of a line's words: no ellipses, and spaces where they are held together. */
const lineLabel = (words: string): string => words.replace(/…/g, "").replace(/\u2060/g, "").replace(/\u00a0/g, " ").trim();

/** What a gap's line says, and the days it reaches when it reaches any (`#250 date-separators`). */
const gapWords = (count: number, days = ""): string => lineWords(`… ${count} unmatched message${count === 1 ? "" : "s"}`, days);

/** The count in words, as Settings and the status line say it. */
const collapsedLinesWords = (n: number): string =>
  n === 0 ? "only their header" : `${n} line${n === 1 ? "" : "s"}`;

/** K in words, as Settings says it. */
const cardMaxLinesWords = (n: number): string => `${n} line${n === 1 ? "" : "s"}`;

/** Two tallies of one emoji share this: a custom emoji is known by its id, a standard one by itself. */
const reactionKey = (reaction: ReactionFace): string =>
  reaction.custom ? `c:${reaction.custom_id || reaction.emoji}` : `u:${reaction.emoji}`;

/** What a chip shows of its emoji: the emoji itself, or a custom emoji's name between colons. */
const reactionText = (tally: ReactionFace): string => (tally.custom ? `:${tally.emoji}:` : tally.emoji);

/**
 * What a collapsed run's line says: how many, and the days it brings (`#250 date-separators`), held
 * together as a gap's line is (`lineWords`) so a narrow line breaks only between the two.
 */
const readRunText = (count: number, days = ""): string => lineWords(`… ${count} read message${count === 1 ? "" : "s"}`, days);

/**
 * `#245 done-feedback`. What the Undo chip calls the message it would put back: the message's first
 * words, cut at a word to about this many characters — the whole of what its tooltip says. The chip
 * itself floats over the list beside the other chips and shows only what fits there, ending what it
 * cuts with an ellipsis of its own, and on a phone's narrow row only "Undo" (web/voice.css,
 * `#undo-dismiss .undo-name`).
 */
const UNDO_NAME_CHARS = 80;

/**
 * The opening words of `text`, to `UNDO_NAME_CHARS` and cut at a word with an ellipsis, for the
 * Undo chip. A first word longer than that is cut inside itself rather than dropped.
 */
function undoName(text: unknown): string {
  let out = "";
  for (const word of String(text === null || text === undefined ? "" : text).split(/\s+/).filter(Boolean)) {
    const next = out ? `${out} ${word}` : word;
    if (next.length > UNDO_NAME_CHARS) return `${out || word.slice(0, UNDO_NAME_CHARS)}…`;
    out = next;
  }
  return out;
}

/**
 * The Undo chip's words. Not a bare "Undo": after a Done the reader's eye is already on the next
 * message, and an Undo that does not say WHICH message it would put back is a guess.
 */
function undoLabel(dismissal: { readonly name?: string | null; readonly messages: readonly unknown[] } | null): string {
  if (dismissal === null) return "Undo";
  if (dismissal.name) return `Undo “${dismissal.name}”`;
  return `Undo Done (${dismissal.messages.length})`;
}

/** What a gap among the cards says: its messages, and its threads when they are all it holds. */
function cardGapWords(messages: number, threads: number, exact: boolean, onlyCards: boolean): string {
  const counted = `${exact ? "" : "at least "}${messages} unmatched message${messages === 1 ? "" : "s"}` +
    (threads > 0 && onlyCards ? ` in ${threads} thread${threads === 1 ? "" : "s"}` : "");
  return lineWords(`… ${counted}`, "");
}

/** What the line of a run over the cards says: how many read threads, and read messages of Main's. */
function cardRunText(threads: number, messages: number): string {
  const parts: string[] = [];
  if (threads > 0) parts.push(`${threads} read thread${threads === 1 ? "" : "s"}`);
  if (messages > 0) parts.push(`${messages} read message${messages === 1 ? "" : "s"}`);
  return `… ${parts.join(" and ")}`;
}

export {
  ELIDED_MIN_CHARS,
  ELIDED_MIN_WORDS,
  SPACE_POOR_CHARS,
  UNDO_NAME_CHARS,
  cardGapWords,
  cardMaxLinesWords,
  cardRunText,
  collapsedLinesWords,
  elisionLabel,
  gapWords,
  lineLabel,
  lineWords,
  reactionKey,
  reactionText,
  readRunText,
  undoLabel,
  undoName,
};
export type { ReactionFace };

// How stored calls and confirmed sends are worded, with no DOM (#624 voice-page-modularity).
import type { PendingPost, PendingPostResponse, SentRecord } from "../../contract/index.ts";



/**
 * The key a proposal, a send and a frozen card are known by on this page: the serial, and which
 * run of the server numbered it. `#270 voice-transcript-markers`. A server numbers its proposals
 * from one again every time it starts, and this page stays open across that, so a serial alone
 * would take the next run's "serial 3" for the card already drawn for the last run's — and draw
 * nothing for it, or file it in the conversation the old one was shown in. An older server names
 * no run; its key is the serial alone.
 *
 * @param {string | undefined | null} instance
 * @param {number} serial
 */
const postKey = (instance: string | undefined | null, serial: number) => `${instance || ""}:${serial}`;



/** What a frozen card says it answered, and where, in words: never an id. */
function sentWhere(detail: Partial<SentRecord>) {
  const parts = [];
  if (detail.reply_to) {
    if (detail.reply_to_author && detail.reply_to_excerpt) {
      parts.push(`replying to ${detail.reply_to_author}: “${detail.reply_to_excerpt}”`);
    } else if (detail.reply_to_author) {
      parts.push(`replying to ${detail.reply_to_author}`);
    } else {
      parts.push("replying to an earlier message");
    }
  }
  if (detail.thread_name) parts.push(`in thread “${detail.thread_name}”`);
  else if (detail.thread_id) parts.push("in a thread");
  return parts.join(" · ");
}



/** The lines a frozen card adds when less than all of it is known to have gone, or it is cut. */
function sentNotes(detail: Partial<SentRecord>) {
  const notes = [];
  const posted = Number(detail.parts_posted) || 0;
  const total = Number(detail.parts_total) || 0;
  const unsent = Number(detail.unsent_chars) || 0;
  const went = posted === 1 ? "Part 1" : `Parts 1–${posted}`;
  if (detail.outcome === "unconfirmed") {
    notes.push("The chat service may have it — check the channel.");
  } else if (detail.outcome === "partial" && detail.uncertain_part) {
    notes.push(`${went} went; part ${posted + 1} may have arrived — check the channel; the rest was not sent.`);
  } else if (detail.outcome === "partial") {
    notes.push(total > 0
      ? `${went} of ${total} went; the rest (${unsent} characters) was not sent.`
      : `${went} went; the rest (${unsent} characters) was not sent.`);
  }
  if (detail.text_truncated) {
    const shown = Number(detail.text_chars) || 0;
    notes.push(`Shortened here — ${Math.min(shown, 8000)} of ${shown} characters; the full message is in the channel.`);
  }
  return notes;
}



/**
 * What the server says became of `proposal`, from one read's `settled`, or null when it does not
 * say — an older server, or another run of this one.
 *
 * @param {PendingPostResponse} answer
 * @param {PendingPost} proposal
 */
function proposalFate(answer: PendingPostResponse, proposal: PendingPost) {
  if ((answer.instance || "") !== (proposal.instance || "")) return null;
  const settled = (answer.settled || []).find((one) => one.serial === proposal.serial);
  return settled ? settled.fate : null;
}



/**
 * How a stored speaker is shown.
 *
 * THREE stored speakers, not two. A `note` is something the PAGE recorded — a hang-up, an error it
 * wanted kept — and the server accepts and stores it as its own speaker. Folding it into
 * "assistant" would put words in the assistant's mouth that it never said, which is the one
 * attribution this screen is careful about everywhere else. It comes back labelled as what it is.
 * (A line rather than a seam on purpose: a seam's explanation is held to a word budget, and a
 * stored note is text of whatever length it was written with.)
 */
const restoredSpeaker = (speaker: string | undefined) =>
  speaker === "you" ? "you" : speaker === "note" ? "note" : "assistant";



/** Newest first on the wire — a page is taken from the end the reader is at. Read the other way. */
const oldestFirst = <Turn>(payload: {turns?: readonly Turn[]} | null) => [...((payload && payload.turns) || [])].reverse();
export { postKey, sentWhere, sentNotes, proposalFate, restoredSpeaker, oldestFirst };

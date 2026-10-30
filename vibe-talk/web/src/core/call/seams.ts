// What a voice call's transcript lines say when the call moves to a new session, and what came of a
// draft the reader was asked about: sent by voice, refused, or not sent (`#269 approval-card-inline`,
// `#270 voice-transcript-markers`, `#352 voice-reconnect-on-bridge-error`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 13). Fixed sentences, so each one's
// length is a fact a test can hold to the band every seam keeps. Where they are drawn stays with the
// page.

/** The entry of `table` under `key`, read as the legacy script read it (`table[key]`). */
function wordsFor(table: object, key: unknown): string | undefined {
  return (table as { readonly [key: string]: string | undefined })[String(key)];
}

/**
 * What the transcript's "reconnected" line says, by what the new session was handed, the record of
 * the call above (`recap_`) or nothing (`fresh_`), and by what went again: the lost turn's channel
 * messages (`relay`), the reader's typed message (`typed`), nothing (`none`), or nothing though a
 * typed message was lost (`unsent`). Each says what happened and what it means for the agent's
 * memory, within the band every seam keeps.
 */
const RECONNECT_SEAM_DETAILS = {
  recap_relay:
    "The voice service lost a turn, so the call moved to a new session, which was sent this " +
    "call's record, and the messages went again.",
  recap_typed:
    "The voice service lost a turn, so the call moved to a new session, which was sent this " +
    "call's record, and your message went again.",
  recap_none:
    "The voice service lost a turn, so the call moved to a new session, which was sent this " +
    "call's record. If you were speaking, say it again.",
  recap_unsent:
    "The voice service lost a turn, so the call moved to a new session, which was sent this " +
    "call's record. Send your last message again.",
  fresh_relay:
    "The voice service lost a turn, so the call moved to a new session that has seen nothing above " +
    "this line, and the messages went again.",
  fresh_typed:
    "The voice service lost a turn, so the call moved to a new session that has seen nothing above " +
    "this line, and your message went again.",
  fresh_none:
    "The voice service lost a turn, so the call moved to a new session that has seen nothing above " +
    "this line. If you were speaking, say it again.",
  fresh_unsent:
    "The voice service lost a turn, so the call moved to a new session that has seen nothing above " +
    "this line. Send your last message again.",
};

/**
 * What a "Not sent" line says, per reason. A spoken no's is `SPOKEN_YES_SEAMS.declined`.
 *
 * `ended` stays true whatever comes after it: Hang up only puts the draft away on this page, and the
 * server keeps it until it lapses, so a reloaded page can still offer it, and a tap there still sends
 * it — under this line, as a card of its own.
 */
const NOT_SENT_SEAMS = {
  withdrawn:
    "You tapped Don't send, so the draft was withdrawn and nothing was posted. Ask the assistant " +
    "for new words if you want them.",
  lapsed:
    "The draft lapsed before anyone sent it, so nothing was posted. Ask the assistant to draft it " +
    "again if you still want it sent.",
  ended:
    "You hung up with a draft still waiting, so it was not posted then, and the page stopped " +
    "offering it.",
};

/**
 * The transcript seam's explanation, per outcome: fixed sentences, so each one's length is a fact a
 * test can hold to the same band as every other seam. The specifics are on the status line.
 */
const SPOKEN_YES_SEAMS = {
  sent:
    "Posted after you said yes. This page decided it was a yes from what it heard, not the " +
    "assistant, and told the assistant so.",
  partial:
    "Only part of the message was posted after your yes; the rest was not. The screen shows what " +
    "is left, and the assistant was told.",
  unknown:
    "The page could not confirm whether your yes posted the message, so the assistant was told to " +
    "say so. Check the channel before sending again.",
  unconfirmed:
    "The chat service's answer was lost, so nobody can say whether your yes posted it. The card " +
    "above says so, and the assistant was told.",
  refused:
    "Your yes reached the server, which refused the send, so nothing was posted. The status line " +
    "says why, and the assistant was told.",
  gone:
    "Nothing was posted: no draft was waiting by the time you answered. Ask the assistant to draft " +
    "the message again if you still want it sent.",
  not_armed:
    "Nothing was posted: the page did not take that as a yes to the card. Tap Send, or ask the " +
    "assistant to read it back again.",
  declined:
    "You said no to the read-back, so the draft was withdrawn and nothing was posted. Ask the " +
    "assistant for new words if you want them.",
};

/** The seam's label, per outcome. */
const SPOKEN_YES_SEAM_LABELS = {
  sent: "Sent by voice",
  partial: "Partly sent",
  unknown: "Not confirmed",
  unconfirmed: "Not confirmed",
  refused: "Not sent",
  gone: "Not sent",
  not_armed: "Not sent",
  declined: "Not sent",
};

/**
 * The "reconnected" line for a new session handed this call's record (`recap`) or nothing (`fresh`),
 * by what went to it again (`resend`); undefined for a pair no sentence names.
 */
export function reconnectSeamDetail(handed: string, resend: unknown): string | undefined {
  return wordsFor(RECONNECT_SEAM_DETAILS, `${handed}_${resend}`);
}

/** What a draft's "Not sent" line says for `why`: a spoken no's, a named reason's, or a lapse's. */
export function notSentDetail(why: unknown): string {
  return why === "declined" ? SPOKEN_YES_SEAMS.declined : wordsFor(NOT_SENT_SEAMS, why) || NOT_SENT_SEAMS.lapsed;
}

/** The outcome of a spoken yes that the transcript names: `kind` where a sentence says it, else `not_armed`. */
export function spokenOutcomeKind(kind: string): string {
  return kind in SPOKEN_YES_SEAMS ? kind : "not_armed";
}

/** The transcript seam for a spoken yes's outcome `kind` (`spokenOutcomeKind`): its label and sentence. */
export function spokenYesSeam(kind: string): { readonly label: string | undefined; readonly detail: string | undefined } {
  return { label: wordsFor(SPOKEN_YES_SEAM_LABELS, kind), detail: wordsFor(SPOKEN_YES_SEAMS, kind) };
}

// What a spoken answer says to a card's "Send it?" and whether the assistant read a draft back (`#624
// voice-page-modularity`, moved from the legacy script into present/call; `#268 voice-call-drops`,
// `#269 approval-card-inline`, `#579 voice-send-confirm-timing`): exact word matching, pure.

import type { PendingPost } from "../../contract/index.ts";
import { spokenWords } from "./transcript.ts";

/** Answers that mean yes, as word lists; matched whole, longest first. */
const SPOKEN_YES_PHRASES: readonly (readonly string[])[] = [
  ["send", "it"], ["do", "it"], ["go", "ahead"], ["please", "do"],
  ["yes"], ["yeah"], ["yep"], ["send"], ["sure"],
];

/** Words that may pad a yes, at most once: "yes please", "ok send it". */
const SPOKEN_YES_FILLER = new Set<string | undefined>([
  "please", "ok", "okay", "now", "thanks", "yes", "yeah", "yep", "sure", "oh", "um", "uh",
]);

/** How a no starts: "no", "don't send it", "cancel", "stop", "wait", "never mind". */
const SPOKEN_NO_STARTS: readonly (readonly string[])[] = [
  ["no"], ["nope"], ["nah"], ["cancel"], ["stop"], ["wait"], ["don't"], ["dont"],
  ["do", "not"], ["never", "mind"],
];

/**
 * Words that sound like a yes inside an answer that is not one: answered, never acted on. Wide on
 * purpose — "sounds good", "go for it", "uh-huh", "that's right", "of course", "ship it" — because
 * an agent told to say only "One moment." on a yes will take any of them for one, and wait.
 */
const SPOKEN_YES_HINTS = new Set<string>([
  "yes", "yeah", "yep", "yup", "ya", "yah", "aye", "send", "sure", "ok", "okay", "absolutely",
  "correct", "post", "good", "great", "perfect", "fine", "right", "alright", "go", "ship",
  "course", "definitely", "exactly", "affirmative", "agreed", "certainly", "indeed", "totally",
  "cool", "please", "uh", "huh", "mhm", "mm", "hmm", "mmhmm",
]);

/** Words that make sending the subject: of a question the assistant asked, or of the answer itself. */
const SENDING_WORDS = new Set<string>(["send", "sent", "sending", "post", "posted", "posting"]);

/** What stands between a word that sends and where it sends to (`namesWhereToSend`). */
const SEND_DESTINATION_WORDS = new Set<string>(["to", "in", "on", "into"]);

/** How many closing words stand for the question in a transcript with no punctuation at all. */
const SENDING_QUESTION_WORDS = 6;

/**
 * The longest answer, in words, that is answered while sending is the subject even though nothing
 * in it sounds like a yes: an agent may take "make it so" or "fire away" for one. A longer answer
 * that does not — "change it to Friday", "what time is it" — is a request of its own, which the
 * agent will act on, and a notice about the draft would only talk over it.
 */
const SHORT_ANSWER_WORDS = 3;

/** What may open a read-back, once, without changing what it says. */
const READ_BACK_LEAD_INS: readonly (readonly string[])[] = [["okay"], ["ok"], ["alright"], ["all", "right"], ["sure"], ["got", "it"]];

/** Words of an answer or a read-back: `spokenWords`, with curly apostrophes made straight first. */
function answerWords(text: unknown): string[] {
  return spokenWords(String(text).replace(/[\u2018\u2019]/gu, "'"));
}

/**
 * What a final user turn says to a card's "Send it?".
 *
 * Pure, and exact on purpose: a no is anything that STARTS like one ("no, change it to Friday" is a
 * no to this draft); a yes is nothing but phrases from the closed list plus at most one word of
 * filler, at most three pieces in all ("yes", "yes please", "ok send it", "yeah go ahead"); a
 * question is never a yes, because "Send it?" is what the agent just said. A yes inside a sentence
 * that adds anything ("yes and tell him I'm late", "send it to Bob") is a `hint`, which sends
 * nothing and is answered.
 */
function classifySpokenAnswer(text: unknown): "affirmative" | "negative" | "hint" | "other" {
  const raw = String(text);
  const words = answerWords(raw);
  if (words.length === 0) return "other";
  if (SPOKEN_NO_STARTS.some((start) => start.every((word, at) => words[at] === word))) {
    return "negative";
  }
  if (!/[?¿]/u.test(raw) && isSpokenYes(words)) return "affirmative";
  return words.some((word) => SPOKEN_YES_HINTS.has(word)) ? "hint" : "other";
}

/** Does `words` split, left to right, into yes phrases and at most one filler word? */
function isSpokenYes(words: readonly string[]): boolean {
  let at = 0;
  let phrases = 0;
  let fillers = 0;
  let pieces = 0;
  while (at < words.length) {
    const phrase = SPOKEN_YES_PHRASES.find((candidate) =>
      candidate.every((word, offset) => words[at + offset] === word)
    );
    if (phrase) {
      phrases += 1;
      at += phrase.length;
    } else if (SPOKEN_YES_FILLER.has(words[at])) {
      fillers += 1;
      at += 1;
    } else {
      return false;
    }
    pieces += 1;
  }
  return phrases >= 1 && fillers <= 1 && pieces <= 3;
}

/**
 * Is `said` (words) exactly the read-back of `shown`: "To {channel}: {text}. Send it?"
 *
 * Pure. Word for word after `answerWords`, so case and punctuation do not matter and nothing else
 * may differ: at most one lead-in from READ_BACK_LEAD_INS, an optional "the" before the channel's
 * name and "channel" after it, and then the card's text and "send it" with nothing inserted,
 * nothing between them and nothing after. An inserted "not" fails, and so does any sentence of the
 * model's own.
 */
function isExactReadBack(said: readonly string[], shown: { readonly channel_name: string; readonly text: string }): boolean {
  const text = answerWords(shown.text);
  if (text.length === 0) return false;
  const channel = answerWords(shown.channel_name);
  const lead = READ_BACK_LEAD_INS.find((words) => words.every((word, at) => said[at] === word));
  const heard = lead ? [said, said.slice(lead.length)] : [said];
  for (const before of [[], ["the"]] as string[][]) {
    for (const after of [[], ["channel"]] as string[][]) {
      const expected = ["to", ...before, ...channel, ...after, ...text, "send", "it"];
      const same = (words: readonly string[]): boolean =>
        words.length === expected.length && words.every((word, at) => word === expected[at]);
      if (heard.some(same)) return true;
    }
  }
  return false;
}

/**
 * Why no read-back can be held to a draft whose text is `text`, as a key of SPOKEN_YES_REASONS, or
 * null when one can.
 *
 * Pure. The read-back is compared word for word (`isExactReadBack`), and `answerWords` keeps only
 * letters, digits and apostrophes, so anything else in the draft is invisible to that comparison —
 * and it is exactly what a voice drops: markup that strikes, hides or quotes words, a + or a -
 * before a number, an emoji, the dots and slashes that make one address another. So the draft may
 * hold only letters, decimal digits, whitespace, apostrophes, a hyphen between two letters, a dash
 * with a space on each side, and sentence punctuation — . , ; : ! ? … — followed by a space or the
 * end. A dot or a colon inside a word ("github.com", "10:30") is not sentence punctuation. An
 * accent that composes with its letter is part of the letter; a combining mark that does not — a
 * stroke drawn through a word is one — is not. Anything else makes the draft tap-only; a mention
 * says so by name.
 */
function unsayableDraft(text: unknown): "mention" | "symbols" | null {
  const raw = String(text).normalize("NFC");
  if (/<[^<>\s]+>/u.test(raw)) return "mention";
  // The apostrophe in the last class is written as an escape. The page suite pairs the quotes in
  // this file to set its strings aside, and a bare one in a pattern throws that pairing out of step.
  const rest = raw
    .replace(/[\u2018\u2019]/gu, "'")
    .replace(/(\p{L})-(?=\p{L})/gu, "$1")
    .replace(/(\s)[\u2013\u2014](?=\s)/gu, "$1")
    .replace(/[.,;:!?\u2026]+(?=\s|$)/gu, "")
    .replace(/[\p{L}\p{Nd}\u0027\s]/gu, "");
  return rest ? "symbols" : null;
}

/**
 * What the agent wrote of a draft: its text without the tag the server put in front of it, which
 * the proposal names (`prefix`, `#303 voice-post-prefix`).
 *
 * Pure. The tag is the owner's own words, saved in Settings and added by the server to every post
 * the agent makes, so nothing the agent was told or read can change it. It is still held to the
 * read-back word for word — `isExactReadBack` compares the whole text, tag and all, because the tag
 * is posted — but it is not judged by `unsayableDraft`: that guards against a model hiding meaning
 * where a voice cannot say it, and the shipped tag's parentheses would otherwise make every draft
 * tap-only. A proposal with no `prefix` — tagging off, or an older server — is the agent's text
 * whole.
 */
function agentDraft(shown: { readonly text: string; readonly prefix?: unknown }): string {
  const prefix = typeof shown.prefix === "string" ? shown.prefix : "";
  return prefix && shown.text.startsWith(prefix as string) ? shown.text.slice((prefix as string).length) : shown.text;
}

/**
 * Why what the assistant said is not a read-back of `shown`, as a key of SPOKEN_YES_REASONS, or
 * null when it is one.
 *
 * Held to everything said since the last turn of anyone's, or to the part of it said once this
 * draft was on the card — the second so a replacement read back in full is not spoilt by the old
 * one's words before it, the first so a read-back whose transcript beat the card to this page still
 * counts.
 *
 * "The part said once this draft was on the card" is counted in words, not only in turns: the
 * neutral protocol keeps one row per numbered turn, so a sentence the agent said before it proposed
 * ("Sure, let me draft that.") and the read-back after it can be one turn, and that turn began
 * before the card did. Only a turn that began with NO draft on the card is counted that way: in one
 * that began with another draft up, or saw one replaced, a newer draft read back after an older one
 * is refused, as it was before. `#268 voice-call-drops`.
 */
function readBackRefusal(shown: PendingPost, turns: ReadonlyArray<{ readonly text: string; readonly card: string | null; readonly cardFrom?: { readonly handle: string; readonly words: number } | null }>): "mismatch" | "no_question" | null {
  const all = answerWords(turns.map((turn) => turn.text).join(" "));
  const sinceCard = (turn: (typeof turns)[number]): string[] => {
    if (turn.card === shown.handle) return answerWords(turn.text);
    const from = turn.cardFrom;
    return from && from.handle === shown.handle ? answerWords(turn.text).slice(from.words) : [];
  };
  const since = turns.flatMap(sinceCard);
  if (isExactReadBack(all, shown) || isExactReadBack(since, shown)) return null;
  return all.slice(-2).join(" ") === "send it" ? "mismatch" : "no_question";
}

/**
 * Is sending the subject of this answer? Yes when the answer itself says send or post ("ok send
 * it"), or when anything the assistant said since the user last spoke asked about sending (each
 * turn by `turnAsksAboutSending`). Every such question counts, not only the last: a relayed message
 * read out after "Send it?" may ask a question of its own, and the owner's yes may still be the
 * answer to "Send it?". "Want to hear the rest?" is not about sending, nor is "It was not sent. Tap
 * Send or ask me to read it back."
 */
function askedAboutSending(turns: ReadonlyArray<{ readonly text: unknown }>, answer: unknown): boolean {
  if (answerWords(answer).some((word) => SENDING_WORDS.has(word))) return true;
  return turns.some((turn) => turnAsksAboutSending(String(turn.text)));
}

/**
 * Does one assistant turn ask about sending? When any sentence of it that ends in a question mark
 * says send or post ("Send it?", "Shall I post that?"); when it ends on a read-back's closing words
 * whatever the transcript punctuated them with ("...Send it." is still the question); and, for a
 * transcript with no punctuation at all, when its closing words say send or post.
 */
function turnAsksAboutSending(said: string): boolean {
  const words = answerWords(said);
  if (words.slice(-2).join(" ") === "send it") return true;
  if (!/[.!?]/u.test(said)) {
    // No punctuation at all: its closing words stand for the question. One that has sentences but
    // asks nothing asked nothing — "It was not sent. Tap Send." is not a question.
    return words.slice(-SENDING_QUESTION_WORDS).some((word) => SENDING_WORDS.has(word));
  }
  return (said.match(/[^.!?]*\?/gu) || []).some((question) =>
    answerWords(question).some((word) => SENDING_WORDS.has(word))
  );
}

/**
 * Could an agent take `text`, classified `kind`, for a yes? A plain yes, anything with a word that
 * sounds like one, or anything short that does not start like a no. Such an answer, while sending
 * is the subject, is always answered: the agent will have said only "One moment." and be waiting.
 */
function mayBeYes(kind: string, text: unknown): boolean {
  if (kind === "affirmative" || kind === "hint") return true;
  return kind === "other" && answerWords(text).length <= SHORT_ANSWER_WORDS;
}

/**
 * Does `text` ask to send something somewhere: a word that sends, and after it "to", "in", "on" or
 * "into" with a word after that? "Send hello on lead team", "post that in the lead channel", "send it
 * to Bob" do; "ok send it", "yep that's right, send it" and "go on, send it" do not. Pure.
 * `#579 voice-send-confirm-timing`: with no card up, such a turn is the owner's new request, never
 * an answer to a draft that has gone.
 */
function namesWhereToSend(text: unknown): boolean {
  const words = answerWords(text);
  const verb = words.findIndex((word) => SENDING_WORDS.has(word));
  if (verb < 0) return false;
  const to = words.findIndex((word, at) => at > verb && SEND_DESTINATION_WORDS.has(word));
  return to > verb && to < words.length - 1;
}

/**
 * The words the agent is told to say on a yes, and nothing else: every prompt that asks before it
 * sends says `say only "One moment."`. `#268 voice-call-drops`.
 */
const SPOKEN_YES_ACKNOWLEDGEMENT: readonly string[] = ["one", "moment"];

/**
 * Are `text`'s words the acknowledgement of a yes, matched as the prompt instructs it — "One
 * moment." and nothing else, case and punctuation aside? While its turn is still open, "One" alone
 * also counts: the assistant's words are streamed as they come, and that is where "One moment."
 * starts. "One moment, or should I cancel it?" is not it, nor is "Or should I cancel it instead?",
 * nor a turn that completed on "One".
 * `open`: whether the turn may still say more.
 */
function isSpokenYesAcknowledgement(text: unknown, open: boolean): boolean {
  const words = answerWords(text);
  const ack = SPOKEN_YES_ACKNOWLEDGEMENT;
  if (words.length === 0 || words.length > ack.length) return false;
  if (words.length < ack.length && !open) return false;
  return words.every((word, at) => word === ack[at]);
}

/**
 * Could `words` still become a yes from the closed list (`isSpokenYes`)? A yes already, or the start
 * of one whose last word may be unfinished: "ye", "yes", "ok se", "go a". Never one that begins with
 * a word a no begins with, which `classifySpokenAnswer` makes a no whatever follows.
 */
function couldBeSpokenYes(words: readonly string[]): boolean {
  if (words.length === 0) return false;
  if (SPOKEN_NO_STARTS.some((start) => start.length === 1 && start[0] === words[0])) return false;
  return isSpokenYes(words) || yesPrefix(words, 0, 0, 0);
}

/**
 * Can `words` from `at` on be the start of the rest of a yes, given the pieces and filler words
 * before it? Only the last word may be unfinished.
 */
function yesPrefix(words: readonly string[], at: number, pieces: number, fillers: number): boolean {
  if (at === words.length) return pieces < 3;
  if (pieces >= 3) return false;
  const unfinished = (offset: number, whole: string): boolean =>
    at + offset === words.length - 1 && whole.startsWith(words[at + offset] as string);
  for (const phrase of SPOKEN_YES_PHRASES) {
    let used = 0;
    while (used < phrase.length && at + used < words.length && words[at + used] === phrase[used]) used += 1;
    if (at + used === words.length) return true;
    if (used < phrase.length) {
      if (unfinished(used, phrase[used] as string)) return true;
      continue;
    }
    if (yesPrefix(words, at + used, pieces + 1, fillers)) return true;
  }
  if (fillers >= 1) return false;
  if (SPOKEN_YES_FILLER.has(words[at]) && yesPrefix(words, at + 1, pieces + 1, fillers + 1)) return true;
  return [...SPOKEN_YES_FILLER].some((filler) => unfinished(0, filler as string));
}

export {
  READ_BACK_LEAD_INS,
  SENDING_QUESTION_WORDS,
  SENDING_WORDS,
  SEND_DESTINATION_WORDS,
  SHORT_ANSWER_WORDS,
  SPOKEN_NO_STARTS,
  SPOKEN_YES_ACKNOWLEDGEMENT,
  SPOKEN_YES_FILLER,
  SPOKEN_YES_HINTS,
  SPOKEN_YES_PHRASES,
  agentDraft,
  answerWords,
  askedAboutSending,
  classifySpokenAnswer,
  couldBeSpokenYes,
  isExactReadBack,
  isSpokenYes,
  isSpokenYesAcknowledgement,
  mayBeYes,
  namesWhereToSend,
  readBackRefusal,
  turnAsksAboutSending,
  unsayableDraft,
  yesPrefix,
};

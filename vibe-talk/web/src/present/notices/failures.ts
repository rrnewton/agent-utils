// What went wrong, in the reader's words (`#624 voice-page-modularity`, moved from the legacy script
// into present/notices): a refused token, a refused or failed send, a failed read, a device voice
// that would not speak, a call the voice service ended.

import { SLOW_WORDS, providerWords, readableFailure } from "../../core/api/index.ts";
import { sentenceStart } from "../../foundation/index.ts";

/**
 * The sentence for a refusal, chosen by WHICH refusal it was.
 *
 * 403 is not a bad token — it is the right token with the wrong scope, and the two are fixed
 * differently: one is a typo, the other is a different string in the same config file. The
 * server's own taxonomy already separates them (`forbidden`, "this token may read but not post"),
 * and this is the one place the difference is expensive, because `/api/v1/client-config` accepts
 * a read-scope token just as it accepts a write-scope one. So a read token signs in cleanly and
 * fails only at the first thing that writes, which is minting a conversation. Calling that
 * "refused" sends the owner hunting for a typo in a token that is not wrong. (It does now say
 * which scope it saw — `tokenScope` — but only so the page stops asking for what a read token
 * cannot have; reading channels with one is supported.)
 */
function refusalSentence(error: { readonly status?: unknown } | null | undefined): string {
  return error && error.status === 403
    ? "that looks like a READ token — this page needs the write-scope one"
    : "that token was refused — paste the write-scope token";
}

/**
 * A 207's `refusal` — the chat service's 4xx answer to the part that failed — as a sentence for the
 * reader: "Google Chat refused this message: it contains @Ada, which would notify someone." The
 * server puts the reason in the reader's words (`explanation`); "" where there is no refusal, from
 * a failure that is not one or a server too old to say. `#444 send-refusal-reason`.
 */
function refusalWords(refusal: { readonly explanation?: unknown } | null | undefined, service: string): string {
  const why = refusal && typeof refusal.explanation === "string" ? refusal.explanation.trim() : "";
  if (!why) return "";
  return `${sentenceStart(service)} refused this message: ${why.replace(/[.!?…]+$/, "")}.`;
}

/**
 * {@link readableFailure} for the cause a 207 carries, which is always the provider's: its answer
 * (`… returned HTTP 400: …`), or the server's failure to get one (`… request failed: …`).
 */
function readableCause(detail: unknown, service: string): string {
  const text = String(detail || "");
  const status = Number((/returned HTTP (\d{3})/.exec(text) || [])[1]) || 0;
  if (status && status < 500 && status !== 408) return providerWords(text);
  if (!status && /request failed/i.test(text) && !SLOW_WORDS.test(text)) {
    return `${sentenceStart(service)} could not be reached right now.`;
  }
  return readableFailure(Object.assign(new Error(text), { status: status || 502, detail: text }), service);
}

function browserSpeechFailure(error: unknown): string {
  if (error === "start-timeout" || error === "speech-timeout") {
    return "The device voice stopped responding. Tap the message again to restart it.";
  }
  if (error === "not-allowed") {
    return "Device speech was blocked. Tap the message again to allow playback.";
  }
  if (error === "voice-unavailable" || error === "language-unavailable") {
    return "The device voice is unavailable. Download a voice in Android Text-to-speech settings, then tap again.";
  }
  return "The device voice could not read that message. Check your media volume and installed voices, then tap again.";
}

/** What a failed read of older messages said went wrong, for a sentence: the server's own words where it gave some. */
function failureWords(error: unknown): string {
  const said = error && typeof error === "object"
    ? String((error as { detail?: unknown; message?: unknown }).detail || (error as { message?: unknown }).message || "")
    : String(error || "");
  return said.trim().replace(/[.\s]+$/, "") || "no reason was given";
}

/**
 * The words for a call the voice service's side ended while the reader did not hang up, the page
 * was in sight, and no error preceded the close.
 *
 * @param span how long the call had been open, in words
 * @param lost whether the close came without a closing handshake
 * @param reason the reason the service gave with its close, if any
 */
function serviceEndedSentence(span: string, lost: boolean, reason: unknown): string {
  const into = span ? ` ${span} in` : "";
  if (lost) {
    return `The call dropped${into}: the connection to the voice service was cut without a ` +
      "goodbye — by the network, the phone or the service, not by Hang up. Start a new call to carry on.";
  }
  const given = String(reason || "").slice(0, 120).replace(/[\s.]+$/u, "");
  const said = given ? ` It said: ${given}.` : "";
  return `The voice service ended the call${into} — you did not hang up.${said} Start a new call to carry on.`;
}

export { browserSpeechFailure, failureWords, readableCause, refusalSentence, refusalWords, serviceEndedSentence };

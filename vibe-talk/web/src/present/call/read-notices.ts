// Read-aloud failure wording (`#624 voice-page-modularity`).


/** What the reader is told of a cloud read that failed for `reason`, with the voice's `detail`. */
export function cloudFailureText(reason: string, detail: unknown) {
  if (reason === "silent") return CLOUD_SILENT;
  if (reason === "no_audio") return CLOUD_NO_AUDIO;
  const raw = String(detail || "").replace(/\s+/gu, " ").trim();
  const known = CLOUD_CAUSES.find(([pattern]) => pattern.test(raw));
  const cause = known ? known[1]!
    : raw ? raw.slice(0, 160)
    : reason === "lost" ? "the connection to it kept dropping"
    : "it could not play this message";
  return `Cloud voice: ${cause}. The message was not archived.`;
}

/** How long a part of a cloud read may go with no speech arrived before the read is stopped. */
export const READ_NO_SPEECH_MS = 4000;

/**
 * What the reader is told when a cloud read fails: a part arrived with no speech in it (`#540`), no
 * speech arrived in time, or the audio request was answered with an error.
 */
export const CLOUD_SILENT = "Cloud voice isn't producing audio: it sent silence for this message (its speech " +
  "access may have expired), so the message was not archived.";

export const CLOUD_NO_AUDIO = "Cloud voice isn't producing audio: no speech came within " +
  `${READ_NO_SPEECH_MS / 1000} seconds of its first audio, so the message was not archived.`;

/**
 * A cloud voice's reason, in short words: the first pattern its detail matches. Else the detail.
 * @type {Array<[RegExp, string]>}
 */
export const CLOUD_CAUSES: [RegExp, string][] = [
  [/speech_access_denied|\bACL\b|access denied/iu,
    "speech access denied (ACL)"],
  [/speech_silent|silence|no_audio|without returning audio|no audio/iu, "it is not producing audio"],
  [/timed? ?out|not acknowledged|within \d+/iu, "it did not answer in time"],
  [/could not open|connection|closed the session|reset|unreachable|lost the session|session ended/iu,
    "the connection to it failed"],
  [/not configured|not_configured/iu, "it is not configured on this server"],
];

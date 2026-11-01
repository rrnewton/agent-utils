// What the sign-in screen says about a pairing link (`#369 pairing-link`; `#624 voice-page-modularity`,
// moved from the legacy script into present/settings). The offer and the redemption are
// web/src/view/settings's.

/**
 * What the page says when a pairing link does not work. One sentence for every refusal, because the
 * server gives one answer for all of them: a link that ran out, one already opened, and one mistyped
 * are the same to it, and the remedy is the same too. The second sentence because a link someone else
 * opened first looks exactly like this from here, and the person who sent it is the one who can tell
 * and act on it.
 */
const PAIRING_EXPIRED =
  "This pairing link has expired or was already used. Ask for a new one. If you had not opened it " +
  "before, tell whoever sent it: someone else may have.";

/** Why a redemption failed, by the server's answer: too many tries from this network, or any refusal. */
const pairingRefusedWords = (status: number): string => status === 429
  ? "Too many pairing attempts from this network. Wait a minute, then ask for a new link."
  : PAIRING_EXPIRED;

/** The redemption never reached the server: the code was not spent. */
const PAIRING_UNREACHED = "Could not reach vibe-talk to pair this device. Check the connection and tap again.";

/** A pasted link for another server, or not a link at all. */
const PAIRING_WRONG_LINK = "Paste a pairing link for this server, or its pairing code.";

/** A pasted code that is not a whole one. */
const PAIRING_PARTIAL = "Paste the complete pairing link or code. Ask for a fresh link if it was already used.";

/** Whether `code` has the shape of a pairing code: 43 characters of base64url. */
const pairingCodeShaped = (code: string): boolean => /^[A-Za-z0-9_-]{43}$/.test(code);

/** The address that carries a pairing code to another device's browser. */
const pairingTransferAddress = (origin: string, code: string | null): string => `${origin}/#pair=${encodeURIComponent(code || "")}`;

export {
  PAIRING_EXPIRED,
  PAIRING_PARTIAL,
  PAIRING_UNREACHED,
  PAIRING_WRONG_LINK,
  pairingCodeShaped,
  pairingRefusedWords,
  pairingTransferAddress,
};

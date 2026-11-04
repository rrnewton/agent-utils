// Whose rows are on screen (`#18 offline-message-cache`; `#624 voice-page-modularity`, moved from the legacy
// script into core/sync): the fingerprint of the token the channel rows on screen were read or drawn for,
// so rows read for one credential are neither saved under another nor merged into its first read once
// another tab or the main app has replaced it. Fingerprinting a token, and clearing the rows, are the page's
// (`tokenFingerprint`, `screenBelongsToToken`).

/** Whose rows are on screen. */
export interface ScreenIdentity {
  /** The rows on screen are now the credential's whose fingerprint is `fingerprint`. */
  claim(fingerprint: string): void;
  /** Whether the rows on screen are the credential's whose fingerprint is `fingerprint`. */
  belongsTo(fingerprint: string): boolean;
}

/** The rows on screen are the credential's whose fingerprint is `fingerprint`. */
export function createScreenIdentity(fingerprint: string): ScreenIdentity {
  let screenIdentity = fingerprint;
  return {
    claim(next) {
      screenIdentity = next;
    },
    belongsTo: (other) => screenIdentity === other,
  };
}

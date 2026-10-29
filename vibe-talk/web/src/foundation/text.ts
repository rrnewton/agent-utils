// Small text helpers for sentences the page writes (`#624 voice-page-modularity`, moved from the
// legacy script).

/** `text` with its first letter capitalised, for a name that opens a sentence. */
export function sentenceStart(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

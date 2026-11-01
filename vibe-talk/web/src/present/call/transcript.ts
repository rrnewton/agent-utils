// How a spoken turn's words are shown (`#624 voice-page-modularity`, moved from the legacy script into
// present/call): the best text for a turn from the frames the voice service streams, and the mark on an
// answer the page knows was cut short.

/** A turn as the transcript holds it: the words settled so far, and the frame still changing. */
interface SpokenEntry {
  readonly settled: string;
  readonly pending?: string | null;
}

function tidySpeech(text: unknown): string {
  return String(text).replace(/\s+/g, " ").trim();
}

/** Lower-case words with punctuation removed: what "the same words" means for a hypothesis. */
function spokenWords(text: unknown): string[] {
  return String(text)
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s\u0027]/gu, " ") // U+0027 keeps contractions whole.
    .split(/\s+/)
    .filter(Boolean);
}

/** Does `needle` occur as a contiguous run of whole words in `hay`? */
function containsWords(hay: readonly string[], needle: readonly string[]): boolean {
  for (let i = 0; i + needle.length <= hay.length; i += 1) {
    if (needle.every((word, j) => hay[i + j] === word)) {
      return true;
    }
  }
  return false;
}

/**
 * Word edit distance from all of `a` to the closest PREFIX of `b`. Zero when `b` merely extends
 * `a`; small when `b` is `a` with a word corrected, and then possibly extended.
 */
function revisionDistance(a: readonly string[], b: readonly string[]): number {
  let row = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i += 1) {
    const next = [i];
    for (let j = 1; j <= b.length; j += 1) {
      // Every index read here was written on the row before or the step before.
      next[j] = Math.min((row[j] as number) + 1, (next[j - 1] as number) + 1, (row[j - 1] as number) + (a[i - 1] === b[j - 1] ? 0 : 1));
    }
    row = next;
  }
  return Math.min(...row);
}

/**
 * The best text for a turn, given what it held and a new frame for it.
 *
 * Four outcomes. The new frame CONTAINS what was held — a cumulative hypothesis — so it wins. It
 * is contained BY what was held — a late repeat — so nothing changes. It is a CORRECTION of what
 * was held, a word or so different over a sentence long enough for that to mean something, so it
 * replaces it. Otherwise it is the next fragment of the same turn and is appended. Three words is
 * the floor for a correction because "yes" and "no" differ by one word too.
 */
function mergeHypothesis(held: unknown, frame: unknown): string {
  const before = tidySpeech(held);
  const after = tidySpeech(frame);
  if (!before || !after) {
    return before || after;
  }
  const a = spokenWords(before);
  const b = spokenWords(after);
  if (containsWords(b, a)) {
    return after;
  }
  if (containsWords(a, b)) {
    return before;
  }
  if (a.length >= 3 && b.length >= 3 && revisionDistance(a, b) <= Math.max(1, Math.floor(a.length / 4))) {
    return after;
  }
  return `${before} ${after}`;
}

function spokenText(entry: SpokenEntry): string {
  return entry.pending ? mergeHypothesis(entry.settled, entry.pending) : entry.settled;
}

/**
 * What marks an answer the page knows was cut short — by the reader's typed message, by an
 * interrupted `turn_complete`, or by the call ending under it. `#330 voice-long-turn-drop`. The
 * words that arrived are kept, on screen and in the record; this says they are not the whole answer.
 */
const CUT_OFF_MARK = "(cut off)";

/** A turn's words as its row shows them and its record keeps them: marked when it was cut off. */
function shownSpoken(entry: SpokenEntry & { readonly cut?: boolean }): string {
  const text = spokenText(entry);
  return entry.cut ? `${text}${text ? " " : ""}${CUT_OFF_MARK}` : text;
}

export { CUT_OFF_MARK, containsWords, mergeHypothesis, revisionDistance, shownSpoken, spokenText, spokenWords, tidySpeech };
export type { SpokenEntry };

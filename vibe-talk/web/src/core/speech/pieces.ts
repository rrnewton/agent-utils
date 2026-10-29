// What a read plays and the pieces a block is read in, moved here from web/voice.js unchanged but
// for its types (`#624 voice-page-modularity`): pure functions over strings, numbers and plain
// records, with no browser, clock or page state.

/** One thing a read plays: the message's place in the row, and its block, or null for the whole message. */
export interface SpeechSegment {
  readonly part: number;
  readonly block: number | null;
}

/** A row's prepared entry, as far as what a read plays depends on it. */
export type SpeechReadyEntry = {
  readonly blocks?: readonly ({ readonly kind: string; readonly spoken?: boolean } | null | undefined)[] | null;
} | null | undefined;

/** A piece of a block's text: offsets into it, white space at either end left out. */
export interface ReadPiece {
  readonly start: number;
  readonly end: number;
}

/**
 * What one read plays, in order: each block that says something of each message in the row, or the
 * message whole where it was not prepared in blocks. `{part, block}`: the message's place in the
 * row, and its block, or null for the whole message.
 *
 * @param ready the row's prepared entries.
 */
function speechSegments(ready: readonly SpeechReadyEntry[]): SpeechSegment[] {
  const segments: SpeechSegment[] = [];
  ready.forEach((entry, part) => {
    const blocks = entry && Array.isArray(entry.blocks) ? entry.blocks : null;
    if (!blocks) {
      segments.push({ part, block: null });
      return;
    }
    blocks.forEach((block, index) => {
      if (block && block.spoken !== false) segments.push({ part, block: index });
    });
  });
  return segments;
}

/**
 * Where the sentences of `text[from, to)` end, as offsets into `text`, the last at `to`. A sentence
 * ends at `.`, `!`, `?` or `…` (and any closing quote or bracket) that is followed by white space or
 * the end. src/render.rs `sentence_ranges` is the same rule.
 */
function sentenceEnds(text: string, from = 0, to = text.length): number[] {
  const ends: number[] = [];
  for (let at = from; at < to; at += 1) {
    if (!/[.!?…]/.test(text[at]!)) continue;
    let end = at + 1;
    while (end < to && /[.!?…"'”’)\]]/.test(text[end]!)) end += 1;
    if (end === to || /\s/.test(text[end]!)) {
      ends.push(end);
      at = end - 1;
    }
  }
  if (ends[ends.length - 1] !== to) ends.push(to);
  return ends;
}

/** The most words one piece says, unless one sentence is longer. src/render.rs `READ_PIECE_WORDS`. */
const READ_PIECE_WORDS = 60;
/** The words a piece aims at in a message that is one paragraph. src/render.rs `READ_PIECE_ALONE_WORDS`. */
const READ_PIECE_ALONE_WORDS = 15;
/** The longest piece of a paragraph at most this many times its shortest. src/render.rs `READ_PIECE_SKEW`. */
const READ_PIECE_SKEW = 2;
/** The glyphs a typed bullet begins with. src/render.rs `READ_BULLETS`. */
const READ_BULLETS = "•◦▪▫‣⁃●○■□▸▹►";

/** How many words `text` holds: runs of anything but white space. */
function wordCount(text: unknown): number {
  return (String(text).match(/\S+/g) || []).length;
}

/**
 * The pieces `text` is read in, in order, each as `{start, end}`, offsets into `text` with the white
 * space at either end left out. `alone`: `text` is the whole of a message that is one block, which is
 * cut finer. See the section note.
 *
 */
function readPieces(text: string, alone = false): ReadPiece[] {
  const groups: [number, number][] = [];
  let run: [number, number] | null = null;
  let at = 0;
  for (const line of String(text).split("\n")) {
    const from = at;
    const to = at + line.length;
    at = to + 1;
    if (READ_BULLETS.includes(line.trimStart().charAt(0)) && line.trimStart() !== "") {
      if (run) groups.push(run);
      run = null;
      groups.push([from, to]);
    } else {
      run = run ? [run[0], to] : [from, to];
    }
  }
  if (run) groups.push(run);
  // Cut finer only a paragraph alone: typed bullets keep the pieces they were typed as.
  const finer = alone && groups.length === 1;
  const ranges: [number, number][] = [];
  for (const [from, to] of groups) {
    if (!finer && wordCount(text.slice(from, to)) <= READ_PIECE_WORDS) {
      ranges.push([from, to]);
      continue;
    }
    // The sentences between the ones too long for a piece, each balanced on its own run.
    let sentences: [number, number, number][] = [];
    const flush = (): void => {
      let at = 0;
      for (const size of balanceSentences(sentences.map((one) => one[2]), finer)) {
        ranges.push([sentences[at]![0], sentences[at + size - 1]![1]]);
        at += size;
      }
      sentences = [];
    };
    let start = from;
    for (const end of sentenceEnds(text, from, to)) {
      const count = wordCount(text.slice(start, end));
      if (count > READ_PIECE_WORDS) {
        flush();
        ranges.push(...evenRuns(text, start, end, count));
      } else if (count === 0) {
        if (sentences.length > 0) sentences[sentences.length - 1]![1] = end;
      } else {
        sentences.push([start, end, count]);
      }
      start = end;
    }
    flush();
  }
  const pieces: ReadPiece[] = [];
  for (let [start, end] of ranges) {
    while (start < end && /\s/.test(text[start]!)) start += 1;
    while (end > start && /\s/.test(text[end - 1]!)) end -= 1;
    if (end > start) pieces.push({ start, end });
  }
  return pieces;
}

/**
 * `read-chunk-balance`. How many of the sentences of `counts` go in each piece, in order: the most even
 * cut for each number of pieces, the first that passes the balance metric, tried from the number
 * wanted down to the fewest that fit and then up. src/render.rs `balance_sentences`, the same rule step
 * for step, ties included.
 *
 * @param counts each sentence's words, none over `READ_PIECE_WORDS`.
 */
function balanceSentences(counts: readonly number[], alone: boolean): number[] {
  const n = counts.length;
  const total = counts.reduce((sum, count) => sum + count, 0);
  if (n === 0) return [];
  if (!alone && total <= READ_PIECE_WORDS) return [n];
  const fewest = Math.max(1, Math.ceil(total / READ_PIECE_WORDS));
  const wanted = Math.min(n, Math.max(fewest, alone ? Math.floor(total / READ_PIECE_ALONE_WORDS) : fewest));
  const prefix = [0];
  for (const count of counts) prefix.push(prefix[prefix.length - 1]! + count);
  const best: (number | null)[][] = Array.from({ length: n + 1 }, () => new Array<number | null>(n + 1).fill(null));
  const from: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(n + 1).fill(0));
  best[0]![0] = 0;
  for (let j = 1; j <= n; j += 1) {
    for (let i = j; i <= n; i += 1) {
      for (let m = j - 1; m < i; m += 1) {
        const words = prefix[i]! - prefix[m]!;
        if (words > READ_PIECE_WORDS || best[j - 1]![m] === null) continue;
        const sum = best[j - 1]![m]! + words * words;
        if (best[j]![i] === null || sum < best[j]![i]!) {
          best[j]![i] = sum;
          from[j]![i] = m;
        }
      }
    }
  }
  const cut = (k: number): number[] | null => {
    if (best[k]![n] === null) return null;
    const sizes: number[] = [];
    let i = n;
    for (let j = k; j >= 1; j -= 1) {
      sizes.push(i - from[j]![i]!);
      i = from[j]![i]!;
    }
    return sizes.reverse();
  };
  const order: number[] = [];
  for (let k = wanted; k >= fewest; k -= 1) order.push(k);
  for (let k = wanted + 1; k <= n; k += 1) order.push(k);
  let least: { sizes: number[]; longest: number; shortest: number } | null = null;
  for (const k of order) {
    const sizes = cut(k);
    if (!sizes) continue;
    let at = 0;
    const words = sizes.map((size) => {
      at += size;
      return prefix[at]! - prefix[at - size]!;
    });
    if (pieceSkewPasses(words)) return sizes;
    const [longest, shortest] = longestAndShortest(words);
    if (least === null || longest * least.shortest < least.longest * shortest) least = { sizes, longest, shortest };
  }
  return least ? least.sizes : counts.map(() => 1);
}

/** The longest and the shortest of `words`, at least 1 each. src/render.rs `longest_and_shortest`. */
function longestAndShortest(words: readonly number[]): [number, number] {
  return [Math.max(1, ...words), Math.max(1, Math.min(...words))];
}

/** The balance metric: the longest piece at most `READ_PIECE_SKEW` times the shortest. src/render.rs `piece_skew_passes`. */
function pieceSkewPasses(words: readonly number[]): boolean {
  const [longest, shortest] = longestAndShortest(words);
  return longest <= READ_PIECE_SKEW * shortest;
}

/**
 * `text[from, to)`, of `count` words, cut into the fewest runs of at most `READ_PIECE_WORDS` words,
 * as equal as words allow, the longer runs first. src/render.rs `even_runs`.
 */
function evenRuns(text: string, from: number, to: number, count: number): [number, number][] {
  const runs = Math.ceil(count / READ_PIECE_WORDS);
  const size = Math.floor(count / runs);
  const longer = count % runs;
  const bounds: number[] = [];
  let inWord = false;
  let seen = 0;
  let wanted = size + (longer > 0 ? 1 : 0);
  for (let at = from; at < to; at += 1) {
    const space = /\s/.test(text[at]!);
    if (!space && !inWord) {
      if (seen === wanted) {
        bounds.push(at);
        wanted += size + (bounds.length < longer ? 1 : 0);
      }
      seen += 1;
    }
    inWord = !space;
  }
  const found: [number, number][] = [];
  let start = from;
  for (const bound of bounds) {
    found.push([start, bound]);
    start = bound;
  }
  found.push([start, to]);
  return found;
}

// What the page and the tests use of it.
export {
  balanceSentences,
  pieceSkewPasses,
  readPieces,
  sentenceEnds,
  speechSegments,
};

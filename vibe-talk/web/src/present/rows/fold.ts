// How much a fold is hiding (`#624 voice-page-modularity`, moved from the legacy script into
// present/rows): the model that lays a message's text out at every width a line can be, and the label
// it gives at each. The legacy script reads a drawn body into runs (`foldRuns`) and draws the labels;
// the model needs neither the DOM nor a layout engine.
//
// `#244 elision-word-count`. The owner: "when messages are collapsed I have no idea if they're big
// huge messages or just barely over the limit". So a folded message says, under its last line, how
// much the fold is keeping back: "(~240 more words)". The rules it keeps (NEVER MORE THAN IS HIDDEN,
// EACH COUNT IS A FLOOR, ROUNDED DOWN, WORDS ARE WHAT THE ROW SHOWS, NOT EVERYTHING IN A MESSAGE IS
// PROSE, TEXT POOR IN SPACES IS COUNTED IN CHARACTERS) are set out where web/voice.js draws the label,
// beside `foldRuns`, which reads a body as this model needs it.

import { SPACE_EM, WIDE_SCRIPT, glyphEm, monoEm } from "../../foundation/index.ts";
import { elisionLabel } from "./labels.ts";

/**
 * One run of a body as the model reads it, `[kind, text]`: `t` text that flows, `m` the same in code's
 * face, `p` one code block's text, `w` the words of a block drawn whole, `s` a spoken turn's text.
 */
type FoldRun = readonly [string, string];

/** A word, or the run of one between two characters of a wide script: what a line may break between. */
interface FoldPiece {
  em: number;
  glyphs: number[];
  wide: boolean;
  word: boolean;
}

/** A line the clamp counts: its pieces, the code block it is in (`pre`, from 1, or 0), whether it stays when empty. */
interface FoldTextLine {
  whole?: false;
  pieces: FoldPiece[];
  pre: number;
  keep: boolean;
}

/** A block drawn whole, whose lines the clamp does not count: its words and characters. */
interface FoldWholeLine {
  whole: true;
  words: number;
  chars: number;
}

type FoldLine = FoldTextLine | FoldWholeLine;

/** A text laid out: its lines, and its words and characters in all. */
interface FoldLaid {
  lines: FoldLine[];
  words: number;
  chars: number;
}

/**
 * The line widths the count is worked out at, in em of the message text: the upper edge of each
 * width class `.elided`'s container query picks between (web/voice.css says the same list), from a
 * narrow phone at the largest text size to a wide desk. Past the last there is no count, because
 * there is no floor. Measured in Chromium: a phone at the default size is 24 to 30em (360 to 430px),
 * a desk's 72ch column 44em; the edges sit just above those, so the floor is close where it is read
 * most.
 */
const FOLD_WIDTHS_EM = [14, 16, 18, 20, 22, 23.5, 25, 27, 28.5, 30, 33, 36, 40, 45, 50, 56, 63, 72, 84, 100];

/**
 * What the model lays out at every width, from `runs` (see `foldRuns`; a string is a spoken turn's
 * text, every line of it a line, blank ones too, as `white-space: pre-wrap` draws them): its LINES,
 * each a list of PIECES a line may break between, and the blocks drawn whole. A piece is a word, or
 * the run of it between two characters of a wide script, each of which is a piece of its own. A
 * line of a code block says which block (`pre`, counted from 1) for the padding that shows the top
 * of its next line. Counted once per text, and laid out at every width from these.
 */
function foldPieces(source: string | readonly FoldRun[]): FoldLaid {
  const runs: readonly FoldRun[] = typeof source === "string" ? [["s", source]] : source;
  const lines: FoldLine[] = [];
  let words = 0;
  let chars = 0;
  let blocks = 0;
  let line: FoldTextLine | null = null;
  let piece: FoldPiece | null = null;
  let inWord = false;
  const endLine = () => {
    if (line && (line.keep || line.pieces.length > 0)) lines.push(line);
    line = null;
    piece = null;
    inWord = false;
  };
  const feed = (part: string, mono: boolean): void => {
    for (const ch of part) {
      if (/\s/.test(ch)) {
        inWord = false;
        piece = null;
        continue;
      }
      const wide = WIDE_SCRIPT.test(ch);
      if (!piece || wide || piece.wide) {
        piece = { em: 0, glyphs: [], wide, word: !inWord };
        (line as FoldTextLine).pieces.push(piece);
      }
      if (!inWord) words += 1;
      inWord = true;
      const em = mono ? monoEm(ch) : glyphEm(ch);
      piece.em += em;
      piece.glyphs.push(em);
      chars += 1;
    }
  };
  for (const [kind, text] of runs) {
    if (kind === "w") {
      endLine();
      const found = String(text).split(/\s+/).filter(Boolean);
      const count = found.reduce((sum, word) => sum + [...word].length, 0);
      lines.push({ whole: true, words: found.length, chars: count });
      words += found.length;
      chars += count;
    } else if (kind === "p") {
      endLine();
      blocks += 1;
      const rows = String(text).split("\n");
      if (rows.length > 1 && rows[rows.length - 1] === "") rows.pop();
      for (const row of rows) {
        line = { pieces: [], pre: blocks, keep: true };
        feed(row, true);
        endLine();
      }
    } else {
      String(text).split("\n").forEach((part, at) => {
        if (at > 0) endLine();
        if (!line) line = { pieces: [], pre: 0, keep: kind === "s" };
        feed(part, kind === "m");
      });
    }
  }
  endLine();
  return { lines, words, chars };
}

/**
 * How many words and characters of `laid` (see `foldPieces`) a fold shows when a line is `widthEm`
 * wide. Words fill a line greedily, a piece too long for a line breaks inside itself as
 * `overflow-wrap: anywhere` breaks it, and a word that STARTS inside the shown lines is shown,
 * because the reader can see that it is there. A block drawn whole that starts inside the shown
 * lines is shown, all of it, and takes none of them; and where the last shown line is a code block's,
 * the next line of that block is shown too, its top in the block's padding.
 */
function foldShows(laid: FoldLaid, widthEm: number, collapsedLines: number): { words: number; chars: number } {
  let line = 0;
  let words = 0;
  let chars = 0;
  // The code block the clamp's last line is in, or 0.
  let edge = 0;
  const start = (pre: number): boolean => {
    line += 1;
    if (line === collapsedLines) edge = pre;
    return line <= collapsedLines || (line === collapsedLines + 1 && pre !== 0 && pre === edge);
  };
  for (const entry of laid.lines) {
    if (entry.whole === true) {
      if (line >= collapsedLines) break;
      words += entry.words;
      chars += entry.chars;
      continue;
    }
    if (!start(entry.pre)) break;
    let at = 0;
    for (const piece of entry.pieces) {
      const gap = piece.word && at > 0 ? SPACE_EM : 0;
      if (at > 0 && at + gap + piece.em > widthEm) {
        if (!start(entry.pre)) return { words, chars };
        at = 0;
      } else {
        at += gap;
      }
      if (piece.word) words += 1;
      for (const em of piece.glyphs) {
        if (at > 0 && at + em > widthEm) {
          if (!start(entry.pre)) return { words, chars };
          at = 0;
        }
        at += em;
        chars += 1;
      }
    }
  }
  return { words, chars };
}

/**
 * The labels of texts already counted. A redraw builds every row again from the same text — on a
 * threaded channel every Done does — and the count is the same each time. Bounded, and forgotten
 * whole when full, which costs one recount of what is on screen.
 */
const elisionCounted = new Map<string, readonly string[]>();

/**
 * Forget every count held: the lines the fold shows changed, and the labels on the page are counted
 * again (the legacy script's `refreshElisionLines`). A held count is keyed by its lines, so this only
 * spares the memory of the old ones.
 */
function forgetElisionCounts(): void {
  elisionCounted.clear();
}

/** How many texts `elisionCounted` holds before it starts again. */
const ELISION_COUNTED_TEXTS = 500;

/** No label at any width: for a body holding something the model cannot lay out (`foldRuns`). */
const NO_ELISION_LABELS = Object.freeze(FOLD_WIDTHS_EM.map(() => ""));

/**
 * The label for `source` at each of `FOLD_WIDTHS_EM`, in order: a spoken turn's text, a drawn body's
 * runs (`foldRuns`), or null for a body the model cannot lay out, which says nothing, counted against
 * `collapsedLines`, the lines the fold shows. Shared: read it, never change it.
 */
function elisionLabels(source: string | readonly FoldRun[] | null | undefined, collapsedLines: number): readonly string[] {
  if (source === null) return NO_ELISION_LABELS;
  const runs: readonly FoldRun[] = typeof source === "string" ? [["s", source]] : source === undefined ? [["s", ""]] : source;
  // `#425 collapsed-lines`. The count is against the lines in force, so they are part of what a
  // held label is a label FOR: a count made at three lines, served at eight, would claim words
  // the reader can see, which is the one thing this label may never do.
  const key = `${collapsedLines}\n${JSON.stringify(runs)}`;
  const held = elisionCounted.get(key);
  if (held) return held;
  const laid = foldPieces(runs);
  const labels = FOLD_WIDTHS_EM.map((widthEm) => {
    const shown = foldShows(laid, widthEm, collapsedLines);
    return elisionLabel({ words: laid.words - shown.words, chars: laid.chars - shown.chars });
  });
  if (elisionCounted.size >= ELISION_COUNTED_TEXTS) elisionCounted.clear();
  elisionCounted.set(key, labels);
  return labels;
}

export { ELISION_COUNTED_TEXTS, FOLD_WIDTHS_EM, NO_ELISION_LABELS, elisionLabels, foldPieces, foldShows, forgetElisionCounts };
export type { FoldLaid, FoldLine, FoldPiece, FoldRun };

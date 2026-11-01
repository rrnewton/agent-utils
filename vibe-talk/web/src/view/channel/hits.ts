// Where a search's words are in a drawn row (`#624 voice-page-modularity`, moved from the legacy script
// into view/channel; `#249 search-highlight`): the ranges of a text the terms cover, the runs of text a
// row draws, the parts of a row a search reads, and the links among them.

import { classWords } from "../dom/index.ts";

/** One piece of a run of text: a text node, or an element standing for its own text (`own`). */
type TextRun = { node: Element; own: true } | { node: Text; own: false };

/** The elements whose edges are line ends when a body is read as text. */
const TEXT_BLOCKS = new Set([
  "blockquote", "div", "h1", "h2", "h3", "h4", "hr", "li", "ol", "p", "pre", "table", "tr", "ul",
]);

/**
 * Where each term is in `lower`, a lower-cased text, as `[start, end]` pairs in order: overlapping
 * and touching hits of different terms joined into one.
 */
function hitRanges(lower: string, terms: readonly string[]): Array<[number, number]> {
  const found: Array<[number, number]> = [];
  for (const term of terms) {
    for (let at = lower.indexOf(term); at >= 0; at = lower.indexOf(term, at + term.length)) {
      found.push([at, at + term.length]);
    }
  }
  found.sort((a, b) => a[0] - b[0]);
  const joined: Array<[number, number]> = [];
  for (const range of found) {
    const last = joined[joined.length - 1];
    if (last && range[0] <= last[1]) last[1] = Math.max(last[1], range[1]);
    else joined.push([range[0], range[1]]);
  }
  return joined;
}

/**
 * The hits of `terms` in `text`, as `[start, end]` pairs in `text`'s own offsets. Matched in the
 * lower case the search matches in; where lower-casing changes a character's length, the offsets are
 * mapped back one character at a time.
 */
function textHits(text: string, terms: readonly string[]): Array<[number, number]> {
  const whole = text.toLowerCase();
  if (whole.length === text.length) return hitRanges(whole, terms);
  let lower = "";
  const back: number[] = [];
  for (let at = 0; at < text.length;) {
    const character = String.fromCodePoint(text.codePointAt(at) as number);
    const folded = character.toLowerCase();
    for (let unit = 0; unit < folded.length; unit += 1) back.push(at);
    lower += folded;
    at += character.length;
  }
  back.push(text.length);
  return hitRanges(lower, terms)
    .map(([start, end]): [number, number] => [back[start] as number, back[end] as number])
    .filter(([start, end]) => end > start);
}

/**
 * The runs of text in `part`, each a list of the text nodes one block of it is drawn in, in order: a
 * block's edge, a line break or a control ends a run. An element holding its text without a text node
 * of its own stands for one (`own`); no browser builds one, but the page's test fixture does.
 */
function textRunsOf(part: Node): TextRun[][] {
  const runs: TextRun[][] = [];
  let run: TextRun[] = [];
  const end = (): void => {
    if (run.length) runs.push(run);
    run = [];
  };
  const walk = (at: Node): void => {
    const kids = at.childNodes;
    if (kids.length === 0 && at.nodeType === 1 && at.textContent) {
      run.push({ node: at as Element, own: true });
      return;
    }
    for (const kid of [...kids]) {
      if (kid.nodeType === 3) {
        if (kid.textContent) run.push({ node: kid as Text, own: false });
        continue;
      }
      if (kid.nodeType !== 1) continue;
      const tag = String((kid as Element).tagName).toLowerCase();
      if (tag === "br" || tag === "button" || tag === "input") {
        end();
        continue;
      }
      const block = TEXT_BLOCKS.has(tag);
      if (block) end();
      walk(kid);
      if (block) end();
    }
  };
  walk(part);
  end();
  return runs;
}

/**
 * The parts of `row` a hit is drawn in, and its author's name, which is lit whole. See the head of
 * this section for which and why.
 */
function findableParts(row: Element): { parts: Element[]; author: Element | null } {
  const parts: Element[] = [];
  let author: Element | null = null;
  const fromMeta = (meta: Element): void => {
    for (const piece of meta.children) {
      const words = classWords(piece);
      if (words.includes("msg-author") || words.includes("who")) author = piece;
      // A rich header's pieces each on their own: `#242 rich-headers`.
      else if (words.includes("rh-head")) parts.push(...piece.children);
    }
  };
  for (const kid of row.children) {
    const words = classWords(kid);
    if (words.includes("meta")) fromMeta(kid);
    else if (words.includes("body") || words.includes("msg-text") || words.includes("row-links")) parts.push(kid);
    else if (words.includes("thread-open")) parts.push(...[...kid.children].filter((one) => String(one.tagName).toLowerCase() === "strong"));
    else if (words.includes("side-card")) parts.push(...[...kid.children].filter((one) => classWords(one).includes("side-card-text")));
  }
  return { parts, author };
}

/** The anchors in `parts`, the drawn links among the row's words. */
function anchorsAmong(parts: readonly Element[]): Element[] {
  const found: Element[] = [];
  const walk = (at: Element): void => {
    for (const kid of at.children) {
      if (String(kid.tagName).toLowerCase() === "a") found.push(kid);
      else walk(kid);
    }
  };
  for (const part of parts) walk(part);
  return found;
}

export { TEXT_BLOCKS, anchorsAmong, findableParts, hitRanges, textHits, textRunsOf };
export type { TextRun };

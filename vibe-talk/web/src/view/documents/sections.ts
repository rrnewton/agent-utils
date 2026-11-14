// The Documents navigator's section model (`#1508
// documents-scrubber-multilevel` for the tree and numbers, `#1541
// documents-nav-outline-tree` for the scrubber rows over it): the
// normalized section tree over the ONE block model (`blocks.ts`,
// `#1505`), its numbers, and the two scrubber rows the tree unfolds
// into. Pure over the blocks, so numbering, the you-are-here
// unfolding and its windowing are testable without a reader; the
// reader and the navigator both consume this model, so a tap on a
// cell starts reading at that cell's block and the reading light
// follows in the same index space.
//
// The numbering rules are the approved design's
// (`ai_docs/plans/2026-10-26-documents-view`, "Section numbers"):
// - the title (the document's only heading at its top level, opening it) is
//   not numbered, and its level is dropped from the numbering;
// - depth is the rank among a heading's ancestors, so skipped source levels
//   collapse (`##` then `####` gives `3` and `3.1`, never `3.0.1`);
// - text before the first numbered section is section 0, "Introduction";
// - the author's numbers win, per level: when every heading at a depth
//   already starts with a number or letter (`3.`, `3.2`, `III.`, `A.`), those
//   are the labels and no computed numbering appears beside them.

import type { DocumentReadBlock } from "./blocks.ts";

/** One numbered section of the document, in document order. */
export interface DocumentSection {
  /** The shown number: `0` (Introduction), `1`, `3.2`, or the author's own. */
  number: string;
  /** The heading's words, without any number; "Introduction" for section 0. */
  title: string;
  /** Depth in the normalized tree (0 = top level); the title is excluded. */
  depth: number;
  /** First block of the section (its heading, or 0 for the Introduction). */
  start: number;
  /** One past the section's last block. */
  end: number;
  /** Index of the heading block, or null for the Introduction. */
  headingBlock: number | null;
  /** Index of the parent section in the same list, or -1 at the top level. */
  parent: number;
  /** Indices of the child sections, in order. */
  children: number[];
}

/** The section tree over one document's blocks. */
export interface SectionModel {
  sections: DocumentSection[];
  /** Block index of the title heading, or -1 where the document has none. */
  titleBlock: number;
  /** The section containing each block (the title block maps to -1). */
  sectionOfBlock: number[];
}

const ROMAN_VALUES: Readonly<Record<string, number>> = {
  I: 1, V: 5, X: 10, L: 50, C: 100, D: 500, M: 1000,
};

function romanValue(token: string): number {
  let total = 0;
  let previous = 0;
  for (let at = token.length - 1; at >= 0; at -= 1) {
    const value = ROMAN_VALUES[token.charAt(at)] ?? 0;
    total += value < previous ? -value : value;
    previous = Math.max(previous, value);
  }
  return total;
}

/**
 * The author's own number opening a heading's text, or null where the
 * heading does not start with one: dotted arabic (`3.`, `3.2`, `3.2.`),
 * roman (`III.`, `IV)`), or a single letter (`A.`, `B)`). A bare year or a
 * numbered list item's count is not a section number: the token must end in
 * `.` or `)` or be dotted, and a roman token must be a valid numeral.
 */
export function authorNumberOf(text: string): string | null {
  const trimmed = text.trim();
  const arabic = /^(\d+(?:\.\d+)*)[.)]\s/.exec(trimmed) ?? /^(\d+\.\d+)\s/.exec(trimmed);
  if (arabic !== null) return (arabic[1] ?? "").replace(/\.$/, "");
  const roman = /^([IVXLCDM]{1,7})[.)]\s/.exec(trimmed);
  if (roman !== null) {
    const token = roman[1] ?? "";
    // Valid numerals only: recompose and compare, so `IIII` or `IC` lose.
    const letters = ["M", "CM", "D", "CD", "C", "XC", "L", "XL", "X", "IX", "V", "IV", "I"];
    const values = [1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1];
    let rest = romanValue(token);
    let composed = "";
    for (let at = 0; at < values.length; at += 1) {
      while (rest >= (values[at] as number)) {
        composed += letters[at] as string;
        rest -= values[at] as number;
      }
    }
    if (composed === token) return token;
    return null;
  }
  const letter = /^([A-Za-z])[.)]\s/.exec(trimmed);
  if (letter !== null) return (letter[1] ?? "").toUpperCase();
  return null;
}

/** Build the section model over `blocks` (the read model's own blocks). */
export function buildSections(blocks: readonly DocumentReadBlock[]): SectionModel {
  const headings: Array<{ block: number; level: number; text: string }> = [];
  blocks.forEach((block, at) => {
    if (block.kind === "heading") headings.push({ block: at, level: block.level ?? 1, text: block.text });
  });
  // The title: the first heading, the only one at the shallowest level.
  let titleBlock = -1;
  if (headings.length > 0) {
    const shallowest = Math.min(...headings.map((heading) => heading.level));
    const atShallowest = headings.filter((heading) => heading.level === shallowest);
    if (atShallowest.length === 1 && (atShallowest[0] as (typeof headings)[number]).block === (headings[0] as (typeof headings)[number]).block) {
      titleBlock = (headings[0] as (typeof headings)[number]).block;
    }
  }
  const numbered = headings.filter((heading) => heading.block !== titleBlock);
  // Depth and parent from the ancestor rank, skipping the title entirely.
  interface Placed {
    block: number;
    text: string;
    depth: number;
    parentBlock: number;
    sibling: number;
    author: string | null;
  }
  const placed: Placed[] = [];
  const stack: Array<{ level: number; block: number; siblings: number }> = [];
  for (const heading of numbered) {
    while (stack.length > 0 && (stack[stack.length - 1] as (typeof stack)[number]).level >= heading.level) stack.pop();
    const parent = stack.length > 0 ? (stack[stack.length - 1] as (typeof stack)[number]) : null;
    const depth = stack.length;
    // Sibling count lives on the parent's stack entry (or a root counter).
    let sibling: number;
    if (parent !== null) {
      parent.siblings += 1;
      sibling = parent.siblings;
    } else {
      sibling = placed.filter((entry) => entry.depth === 0).length + 1;
    }
    placed.push({
      block: heading.block,
      text: heading.text,
      depth,
      parentBlock: parent?.block ?? -1,
      sibling,
      author: authorNumberOf(heading.text),
    });
    stack.push({ level: heading.level, block: heading.block, siblings: 0 });
  }
  // The author's numbers win per depth: every heading there must carry one.
  const authorWon = new Map<number, boolean>();
  for (const entry of placed) {
    authorWon.set(entry.depth, (authorWon.get(entry.depth) ?? true) && entry.author !== null);
  }
  const labelOf = new Map<number, string>();
  for (const entry of placed) {
    let segment: string;
    if (authorWon.get(entry.depth) === true && entry.author !== null) {
      const token = entry.author;
      // A full dotted token (`3.2` at depth 1) already names the whole path.
      segment = token.split(".").length === entry.depth + 1 ? token : (token.split(".").pop() as string);
      if (token.split(".").length === entry.depth + 1) {
        labelOf.set(entry.block, token);
        continue;
      }
    } else {
      segment = String(entry.sibling);
    }
    const parentLabel = entry.parentBlock === -1 ? "" : (labelOf.get(entry.parentBlock) ?? "");
    labelOf.set(entry.block, parentLabel === "" ? segment : `${parentLabel}.${segment}`);
  }
  const sections: DocumentSection[] = [];
  const indexOfBlock = new Map<number, number>();
  const firstNumbered = numbered.length > 0 ? (numbered[0] as (typeof numbered)[number]).block : blocks.length;
  const hasIntro = firstNumbered > 0 && blocks.slice(0, firstNumbered).some((_block, at) => at !== titleBlock);
  if (hasIntro) {
    sections.push({
      number: "0", title: "Introduction", depth: 0, start: 0, end: firstNumbered,
      headingBlock: null, parent: -1, children: [],
    });
  }
  placed.forEach((entry, at) => {
    indexOfBlock.set(entry.block, sections.length);
    // A section's span runs to the next heading at its depth or shallower
    // (its descendants lie inside it), or to the document's end.
    let end = blocks.length;
    for (let later = at + 1; later < placed.length; later += 1) {
      if ((placed[later] as (typeof placed)[number]).depth <= entry.depth) {
        end = (placed[later] as (typeof placed)[number]).block;
        break;
      }
    }
    sections.push({
      number: labelOf.get(entry.block) ?? String(entry.sibling),
      title: entry.text,
      depth: entry.depth,
      start: entry.block,
      end,
      headingBlock: entry.block,
      parent: -1,
      children: [],
    });
  });
  // Parents and children by heading block. Top-level sections stay the
  // Introduction's siblings (parent -1), never its children.
  for (const entry of placed) {
    if (entry.parentBlock === -1) continue;
    const child = sections[indexOfBlock.get(entry.block) as number] as DocumentSection;
    const parent = sections[indexOfBlock.get(entry.parentBlock) as number] as DocumentSection;
    child.parent = indexOfBlock.get(entry.parentBlock) as number;
    parent.children.push(indexOfBlock.get(entry.block) as number);
  }
  const sectionOfBlock: number[] = new Array(blocks.length).fill(-1);
  sections.forEach((section, index) => {
    for (let at = section.start; at < section.end; at += 1) sectionOfBlock[at] = index;
  });
  if (titleBlock >= 0) sectionOfBlock[titleBlock] = -1;
  return { sections, titleBlock, sectionOfBlock };
}

/** The section containing block `at`, or -1 (the title block, empty doc). */
export function sectionIndexAt(model: SectionModel, at: number): number {
  if (at < 0 || at >= model.sectionOfBlock.length) return -1;
  return model.sectionOfBlock[at] as number;
}

/** A block's spoken address: `3.2 ¶ 4` (the Introduction is `0 ¶ n`). */
export function blockAddress(model: SectionModel, at: number): string {
  const index = sectionIndexAt(model, at);
  if (index === -1) return "";
  const section = model.sections[index] as DocumentSection;
  let offset = at - section.start + 1;
  if (section.headingBlock !== null && at >= section.headingBlock) offset -= 1;
  if (model.titleBlock >= section.start && model.titleBlock <= at) offset -= 1;
  return `${section.number} ¶ ${Math.max(1, offset)}`;
}

// The scrubber navigator over the section tree (`#1541
// documents-nav-outline-tree`, owner UI request 066, revising the
// focus-plus-context navigator `#1508` built): two rows that live in the
// read scrubber, built from the Outline's tree rather than from raw
// block indices. Row one is the section tree unfolded only along the
// path to the current section; row two is the current section's own
// chunks, one chunk exactly one block, so a label always names the tap's
// exact target. Overflow is windowed with "…" cells, never merged away:
// aggregation is what made `#1508`'s cells land on paragraphs their
// labels did not suggest. Reading order is the block model's document
// order throughout, so a table's chunk sits between the chunks before
// and after it by construction.

/** One visible section of the scrubber's section row. */
export interface ScrubSectionNode {
  kind: "section";
  /** Index into `SectionModel.sections`. */
  sectionIndex: number;
  number: string;
  title: string;
  /** First block of the section: the tap target. */
  start: number;
  /** One past the section's last block. */
  end: number;
  depth: number;
  /** The section holding the reading position. */
  current: boolean;
  /** An ancestor of the current section (drawn expanded). */
  ancestor: boolean;
  /** Wholly before the reading position. */
  heard: boolean;
  /** Visible children where this section is unfolded, else null. */
  children: ScrubSectionCell[] | null;
}

/** A run of sibling sections the window hid, as one "…" cell. */
export interface ScrubGapNode {
  kind: "gap";
  /** First block of the run's first hidden section: the tap target. */
  start: number;
  /** Index of the run's first hidden section. */
  sectionIndex: number;
}

/** One cell of the section row, in document order. */
export type ScrubSectionCell = ScrubSectionNode | ScrubGapNode;

/** One cell of the chunk row: one block, or a hidden run as "…". */
export type ScrubChunkCell =
  | {
      kind: "chunk";
      /** The block this chunk is: the tap target. */
      block: number;
      /** Its 1-based place among the section's own chunks. */
      local: number;
      weight: number;
      current: boolean;
      heard: boolean;
      /** Spoken address of the block (`3.2 ¶ 4`), for its label. */
      address: string;
    }
  | { kind: "gap"; block: number };

/** The scrubber's two rows at one reading position and width. */
export interface ScrubberNavModel {
  /** Row one: the unfolded section forest (empty with no sections). */
  roots: ScrubSectionCell[];
  /** Row two: the current section's own chunks, windowed. */
  chunks: ScrubChunkCell[];
  /** Section holding the reading position, or -1 with no sections. */
  currentSection: number;
  /** Where the reading position sits: `3.2 ¶ 4`. */
  address: string;
  /** The most cells either row may hold at this width. */
  maxCells: number;
}

/** Cell cap from the strip width, at the design's 24 px target size. */
export function scrubberMaxCells(widthPx: number): number {
  const width = Math.max(96, Math.floor(widthPx) || 360);
  return Math.max(5, Math.floor(width / 24));
}

/**
 * The window of a list of `count` items around `focus` that fits
 * `maxCells` cells: kept indices in order, each maximal hidden run
 * (leading, inner, or trailing) as one gap naming its first hidden
 * index. The focus is always kept; its nearest neighbours are kept
 * next; the first and last items are kept where the cap leaves room.
 * With room for everything, every index is kept and no gap appears.
 */
export function windowIndices(
  count: number,
  focus: number,
  maxCells: number,
): Array<{ index: number } | { gap: number }> {
  if (count <= 0) return [];
  const at = Math.max(0, Math.min(focus, count - 1));
  if (count <= maxCells) {
    return Array.from({ length: count }, (_unused, index) => ({ index }));
  }
  const cellsOf = (keep: ReadonlySet<number>): number => {
    const sorted = [...keep].sort((a, b) => a - b);
    let cells = sorted.length;
    if ((sorted[0] as number) > 0) cells += 1;
    if ((sorted[sorted.length - 1] as number) < count - 1) cells += 1;
    for (let i = 1; i < sorted.length; i += 1) {
      if ((sorted[i] as number) - (sorted[i - 1] as number) > 1) cells += 1;
    }
    return cells;
  };
  const keep = new Set<number>([at]);
  // The list's ends first (they say where the list begins and ends),
  // then the focus's neighbours outward, each only while it fits.
  const tryAdd = (index: number): boolean => {
    if (index < 0 || index >= count || keep.has(index)) return false;
    keep.add(index);
    if (cellsOf(keep) <= maxCells) return true;
    keep.delete(index);
    return false;
  };
  tryAdd(0);
  tryAdd(count - 1);
  for (let step = 1; step < count; step += 1) {
    const left = tryAdd(at - step);
    const right = tryAdd(at + step);
    if (!left && !right && at - step < 0 && at + step >= count) break;
    if (cellsOf(keep) >= maxCells && !left && !right) break;
  }
  const out: Array<{ index: number } | { gap: number }> = [];
  let i = 0;
  while (i < count) {
    if (keep.has(i)) {
      out.push({ index: i });
      i += 1;
      continue;
    }
    out.push({ gap: i });
    while (i < count && !keep.has(i)) i += 1;
  }
  return out;
}

/**
 * A section's own chunks as a block range: from its first block to its
 * first subsection (a leaf's chunks are all its blocks). The ranges of
 * the deepest sections tile the document in order, title apart.
 */
export function sectionChunkRange(
  model: SectionModel,
  sectionIndex: number,
): { start: number; end: number } {
  const section = model.sections[sectionIndex] as DocumentSection;
  const firstChild = section.children[0];
  return {
    start: section.start,
    end: firstChild === undefined
      ? section.end
      : (model.sections[firstChild] as DocumentSection).start,
  };
}

/**
 * Build the scrubber's two rows for reading position `currentBlock`.
 * You-are-here: the tree is unfolded only along the path to the
 * current section — a section is expanded exactly when it is the
 * current section or one of its ancestors — the siblings that
 * unfolding showed are kept adjacent to the path, and every other run
 * is one "…" cell. The section row's cells (labels and gaps, across
 * every unfolded list) and the chunk row's cells each stay within
 * `scrubberMaxCells(widthPx)`, so both rows fit the strip.
 */
export function buildScrubberNav(
  blocks: readonly DocumentReadBlock[],
  model: SectionModel,
  currentBlock: number,
  widthPx: number,
): ScrubberNavModel {
  const maxCells = scrubberMaxCells(widthPx);
  const address = blockAddress(model, currentBlock);
  // Block weights by prefix sum, so a section's weight is one subtraction.
  const prefix: number[] = [0];
  blocks.forEach((block) => {
    prefix.push((prefix[prefix.length - 1] as number) + Math.max(1, block.text.length));
  });
  const weightOf = (start: number, end: number): number =>
    Math.max(1, (prefix[end] as number) - (prefix[start] as number));

  let currentSection = sectionIndexAt(model, currentBlock);
  // A position in no section (the title block) stands at the first
  // section's path, as the reader opens there.
  if (currentSection < 0 && model.sections.length > 0) currentSection = 0;

  const chunkRange = currentSection >= 0
    ? sectionChunkRange(model, currentSection)
    : { start: 0, end: blocks.length };
  const chunkCount = Math.max(0, chunkRange.end - chunkRange.start);
  const chunkFocus = chunkCount === 0
    ? 0
    : Math.max(0, Math.min(currentBlock - chunkRange.start, chunkCount - 1));
  const chunks: ScrubChunkCell[] = windowIndices(chunkCount, chunkFocus, maxCells).map((entry) => {
    if ("gap" in entry) return { kind: "gap" as const, block: chunkRange.start + entry.gap };
    const block = chunkRange.start + entry.index;
    return {
      kind: "chunk" as const,
      block,
      local: entry.index + 1,
      weight: weightOf(block, block + 1),
      current: block === currentBlock,
      heard: block < currentBlock,
      address: blockAddress(model, block),
    };
  });

  if (currentSection < 0) {
    return { roots: [], chunks, currentSection, address, maxCells };
  }

  // The path from the top level to the current section, as a set.
  const onPath = new Set<number>();
  for (let at = currentSection; at >= 0; at = (model.sections[at] as DocumentSection).parent) {
    onPath.add(at);
  }
  const expanded = (index: number): boolean =>
    onPath.has(index) && (model.sections[index] as DocumentSection).children.length > 0;
  // Cell budget per sibling list: the root list plus one list per
  // expanded section on the path share the row's cap between them.
  const listCount = 1 + [...onPath].filter((index) => expanded(index)).length;
  const perList = Math.max(3, Math.floor(maxCells / listCount));

  const buildNode = (index: number): ScrubSectionNode => {
    const section = model.sections[index] as DocumentSection;
    const node: ScrubSectionNode = {
      kind: "section",
      sectionIndex: index,
      number: section.number,
      title: section.title,
      start: section.start,
      end: section.end,
      depth: section.depth,
      current: index === currentSection,
      ancestor: onPath.has(index) && index !== currentSection,
      heard: section.end <= currentBlock,
      children: null,
    };
    if (expanded(index)) {
      // Focus the window on the child the path continues through; at
      // the current section itself the path ends, so its first child.
      const pathChild = section.children.find((child) => onPath.has(child));
      node.children = buildList(section.children, pathChild ?? (section.children[0] as number));
    }
    return node;
  };
  const buildList = (indices: readonly number[], focusSection: number): ScrubSectionCell[] => {
    const focusPos = Math.max(0, indices.indexOf(focusSection));
    return windowIndices(indices.length, focusPos, perList).map((entry) => {
      if ("gap" in entry) {
        const hidden = indices[entry.gap] as number;
        return {
          kind: "gap" as const,
          start: (model.sections[hidden] as DocumentSection).start,
          sectionIndex: hidden,
        };
      }
      return buildNode(indices[entry.index] as number);
    });
  };
  const rootIndices = model.sections
    .map((section, index) => (section.parent === -1 ? index : -1))
    .filter((index) => index >= 0);
  const rootOnPath = rootIndices.find((index) => onPath.has(index)) ?? (rootIndices[0] as number);
  const roots = buildList(rootIndices, rootOnPath);
  return { roots, chunks, currentSection, address, maxCells };
}

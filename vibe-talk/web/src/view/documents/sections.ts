// The Documents navigator's section model (`#1508 documents-scrubber-multilevel`,
// finishing the navigator of `#629 documents-view-client`): the normalized
// section tree over the ONE block model (`blocks.ts`, `#1505`), its numbers,
// and the focus-plus-context rows the scrubber draws. Pure over the blocks,
// so numbering, aggregation, range compression and boundary marks are
// testable without a reader; the reader and the navigator both consume this
// model, so a tap on a unit starts reading at that unit's first block and
// the reading light follows in the same index space.
//
// The rules are the approved design's (`ai_docs/plans/2026-10-26-documents-view`,
// "Section numbers" and "Decision: a focus-plus-context row over the block
// row (owner 2026-10-28)"):
// - the title (the document's only heading at its top level, opening it) is
//   not numbered, and its level is dropped from the numbering;
// - depth is the rank among a heading's ancestors, so skipped source levels
//   collapse (`##` then `####` gives `3` and `3.1`, never `3.0.1`);
// - text before the first numbered section is section 0, "Introduction";
// - the author's numbers win, per level: when every heading at a depth
//   already starts with a number or letter (`3.`, `3.2`, `III.`, `A.`), those
//   are the labels and no computed numbering appears beside them.

import type { DocumentReadBlock } from "./blocks.ts";

/** The smallest unit the scrubber offers, in characters of block text. */
export const MIN_CHUNK_CHARS = 120;

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
  const levelOfBlock = new Map(numbered.map((heading) => [heading.block, heading.level]));
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
    void levelOfBlock;
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
  // Parents and children by heading block, then fix each span: a section's
  // own span runs to the next heading of any depth (set above by document
  // order), which is exactly the linear sequence the bottom row walks.
  for (const entry of placed) {
    if (entry.parentBlock === -1) continue;
    const child = sections[indexOfBlock.get(entry.block) as number] as DocumentSection;
    const parent = sections[indexOfBlock.get(entry.parentBlock) as number] as DocumentSection;
    child.parent = indexOfBlock.get(entry.parentBlock) as number;
    parent.children.push(indexOfBlock.get(entry.block) as number);
  }
  if (hasIntro) {
    const intro = sections[0] as DocumentSection;
    for (const section of sections) {
      if (section.parent === -1 && section !== intro && section.depth === 0) {
        // Top-level sections are the Introduction's siblings, not children.
      }
    }
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

/** One tappable unit of a navigator row: blocks [start, end). */
export interface NavUnit {
  start: number;
  end: number;
  weight: number;
  kind: "blocks" | "section" | "part" | "range";
  /** Section number where the unit is (or enters) a section, else "". */
  number: string;
  /** Shown label: a section number, `3.3 (2/4)`, or a range `6 … 15`. */
  label: string;
  /** Section number whose start opens this unit, where it crosses one. */
  boundary: string;
  /** A folded row's units, drawn nested inside this cell (context row). */
  nested?: NavUnit[];
}

/** Aggregate one leaf section's blocks into units (design step 2). */
export function aggregateLeafUnits(
  blocks: readonly DocumentReadBlock[],
  section: DocumentSection,
  kTarget: number,
  kHard: number,
): NavUnit[] {
  const items: Array<{ start: number; end: number; weight: number; kind: string }> = [];
  for (let at = section.start; at < section.end; at += 1) {
    const block = blocks[at] as DocumentReadBlock;
    items.push({ start: at, end: at + 1, weight: Math.max(1, block.text.length), kind: block.kind });
  }
  if (items.length === 0) return [];
  // Runs aggregate up into units first: a list, quote or table run is one
  // unit, and a heading stays with what follows it.
  const runUnits: typeof items = [];
  for (const item of items) {
    const last = runUnits[runUnits.length - 1];
    const runnable = item.kind === "item" || item.kind === "quote" || item.kind === "table";
    if (last !== undefined && ((runnable && last.kind === item.kind) || last.kind === "heading")) {
      last.end = item.end;
      last.weight += item.weight;
      if (last.kind === "heading") last.kind = item.kind;
      continue;
    }
    runUnits.push({ ...item });
  }
  // The minimum chunk size: no offered unit says less than MIN_CHUNK_CHARS
  // where a neighbour can absorb it.
  const chunked: typeof items = [];
  for (const unit of runUnits) {
    const last = chunked[chunked.length - 1];
    if (last !== undefined && last.weight < MIN_CHUNK_CHARS) {
      last.end = unit.end;
      last.weight += unit.weight;
      continue;
    }
    chunked.push({ ...unit });
  }
  if (chunked.length > 1 && (chunked[chunked.length - 1] as (typeof items)[number]).weight < MIN_CHUNK_CHARS) {
    const last = chunked.pop() as (typeof items)[number];
    const prev = chunked[chunked.length - 1] as (typeof items)[number];
    prev.end = last.end;
    prev.weight += last.weight;
  }
  const toUnit = (span: (typeof items)[number]): NavUnit => ({
    start: span.start, end: span.end, weight: span.weight, kind: "blocks", number: "", label: "", boundary: "",
  });
  if (chunked.length <= kHard) return chunked.map(toUnit);
  // Synthesize parts: g balanced groups, boundaries minimizing the heaviest.
  return balancedPartition(chunked, Math.ceil(chunked.length / Math.max(1, kTarget))).map(toUnit);
}

/** Partition consecutive spans into `g` groups of near-equal total weight. */
export function balancedPartition<T extends { weight: number }>(spans: readonly T[], g: number): T[] {
  if (g <= 1 || spans.length === 0) return spans.slice();
  const total = spans.reduce((sum, span) => sum + span.weight, 0);
  const target = total / g;
  const groups: T[] = [];
  let current: T | null = null;
  let remaining = spans.length;
  for (const span of spans) {
    remaining -= 1;
    if (current === null) {
      current = { ...span };
      groups.push(current);
      continue;
    }
    const groupsLeft = g - groups.length;
    if (current.weight >= target && groupsLeft > 0 && remaining >= groupsLeft - 1) {
      current = { ...span };
      groups.push(current);
      continue;
    }
    (current as { end?: number; weight: number }).weight += span.weight;
    if ("end" in current && "end" in span) (current as unknown as { end: number }).end = (span as unknown as { end: number }).end;
  }
  return groups;
}

/** Compress a row's non-current cells into ranges so it fits `kHard`. */
export function compressRow(units: readonly NavUnit[], currentAt: number, kHard: number): NavUnit[] {
  if (units.length <= kHard || kHard < 3) return units.slice();
  const current = Math.max(0, Math.min(currentAt, units.length - 1));
  const keep = new Set<number>([current]);
  // Keep the nearest neighbours on each side until the row fits, counting
  // each compressed run as one cell.
  let left = current - 1;
  let right = current + 1;
  const fits = (): boolean => {
    let cells = keep.size;
    if (Math.min(...keep) > 0) cells += 1;
    if (Math.max(...keep) < units.length - 1) cells += 1;
    return cells <= kHard;
  };
  while (!fits() && (left >= 0 || right < units.length)) {
    if (left >= 0) { keep.add(left); left -= 1; }
    if (fits()) break;
    if (right < units.length) { keep.add(right); right += 1; }
  }
  const out: NavUnit[] = [];
  let at = 0;
  while (at < units.length) {
    if (keep.has(at)) {
      out.push(units[at] as NavUnit);
      at += 1;
      continue;
    }
    let end = at;
    while (end < units.length && !keep.has(end)) end += 1;
    const first = units[at] as NavUnit;
    const last = units[end - 1] as NavUnit;
    const labelOfUnit = (unit: NavUnit): string => unit.number !== "" ? unit.number : unit.label;
    out.push({
      start: first.start, end: last.end,
      weight: units.slice(at, end).reduce((sum, unit) => sum + unit.weight, 0),
      kind: "range", number: "",
      label: `${labelOfUnit(first)} … ${labelOfUnit(last)}`,
      boundary: "",
    });
    at = end;
  }
  return out;
}

/** One drawn row of the navigator: its units, top row first. */
export interface NavRow {
  units: NavUnit[];
  /** Index of the unit holding the reading position, or -1. */
  current: number;
}

/** The navigator over one document at one reading position and width. */
export interface NavigatorModel {
  kTarget: number;
  kHard: number;
  maxRows: number;
  /** Path labels above the top shown row (`3 Storage › 3.3 Relative`). */
  breadcrumb: string[];
  rows: NavRow[];
  /** Adjacent leaf sections' numbers for the bottom row's edge placeholders. */
  prevSection: string;
  nextSection: string;
  /** Where the reading position sits: `3.2 ¶ 4`. */
  address: string;
}

/** K values from the strip width (design: 24 px target, 14 px hard cap). */
export function navigatorKs(widthPx: number): { kTarget: number; kHard: number; maxRows: number } {
  const width = Math.max(96, Math.floor(widthPx) || 360);
  return {
    kTarget: Math.max(1, Math.floor(width / 24)),
    kHard: Math.max(1, Math.floor(width / 14)),
    maxRows: width >= 600 ? 3 : 2,
  };
}

function sectionUnits(
  blocks: readonly DocumentReadBlock[],
  model: SectionModel,
  section: DocumentSection,
  kTarget: number,
  kHard: number,
): NavUnit[] {
  if (section.children.length === 0) return aggregateLeafUnits(blocks, section, kTarget, kHard);
  const units: NavUnit[] = [];
  const firstChild = model.sections[section.children[0] as number] as DocumentSection;
  const ownEnd = firstChild.start;
  const ownStart = section.start + (section.headingBlock !== null ? 1 : 0);
  if (ownEnd > ownStart) {
    const weight = blocks.slice(ownStart, ownEnd).reduce((sum, block) => sum + Math.max(1, block.text.length), 0);
    units.push({ start: section.start, end: ownEnd, weight, kind: "section", number: section.number, label: section.number, boundary: "" });
  }
  for (const childIndex of section.children) {
    const child = model.sections[childIndex] as DocumentSection;
    const weight = blocks.slice(child.start, child.end).reduce((sum, block) => sum + Math.max(1, block.text.length), 0);
    units.push({ start: child.start, end: child.end, weight, kind: "section", number: child.number, label: child.number, boundary: "" });
  }
  return units;
}

/** Build the navigator rows for reading position `currentBlock`. */
export function buildNavigator(
  blocks: readonly DocumentReadBlock[],
  model: SectionModel,
  currentBlock: number,
  widthPx: number,
): NavigatorModel {
  const { kTarget, kHard, maxRows } = navigatorKs(widthPx);
  const address = blockAddress(model, currentBlock);
  const currentSectionIndex = sectionIndexAt(model, currentBlock);
  // The path from the top level down to the section holding the position
  // (the title block, in no section, stands at the first section's path).
  const path: DocumentSection[] = [];
  const pathSection = currentSectionIndex >= 0 ? currentSectionIndex : 0;
  if (model.sections.length > 0) {
    let cursor: DocumentSection | undefined = model.sections[pathSection];
    while (cursor !== undefined) {
      path.unshift(cursor);
      cursor = cursor.parent >= 0 ? model.sections[cursor.parent] : undefined;
    }
  }
  // Adjacent sections in document order, for the edge placeholders.
  const leafOrder = model.sections;
  const prevSection = currentSectionIndex > 0 ? (leafOrder[currentSectionIndex - 1] as DocumentSection).number : "";
  const nextSection = currentSectionIndex >= 0 && currentSectionIndex < leafOrder.length - 1
    ? (leafOrder[currentSectionIndex + 1] as DocumentSection).number : "";
  if (path.length === 0) {
    // No sections at all: one row of aggregated blocks (today's single row).
    const whole: DocumentSection = {
      number: "", title: "", depth: 0, start: 0, end: blocks.length, headingBlock: null, parent: -1, children: [],
    };
    const units = aggregateLeafUnits(blocks, whole, kTarget, kHard);
    const current = units.findIndex((unit) => currentBlock >= unit.start && currentBlock < unit.end);
    return { kTarget, kHard, maxRows, breadcrumb: [], rows: [{ units: compressRow(units, Math.max(0, current), kHard), current: Math.max(0, current) }], prevSection: "", nextSection: "", address };
  }
  // One candidate row per path node with at least two units, deepest last,
  // opened by the root row (the top-level sections, the design's context
  // row); a leaf with more than K_hard units gains a virtual-parts row
  // above the block row (design step 2: `3.3 (2/4)`).
  interface Candidate { node: DocumentSection; units: NavUnit[] }
  const candidates: Candidate[] = [];
  const root: DocumentSection = {
    number: "", title: "", depth: -1, start: 0, end: blocks.length, headingBlock: null, parent: -1,
    children: model.sections.filter((section) => section.parent === -1).map((section) => model.sections.indexOf(section)),
  };
  for (const node of [root, ...path]) {
    let units = sectionUnits(blocks, model, node, kTarget, kHard);
    if (node.children.length === 0 && units.length > kHard) {
      const parts = balancedPartition(units, Math.ceil(units.length / Math.max(1, kTarget)));
      const partAt = parts.findIndex((part) => currentBlock >= part.start && currentBlock < part.end);
      const partUnits: NavUnit[] = parts.map((part, at) => ({
        ...part, kind: "part", number: node.number,
        label: `${node.number} (${at + 1}/${parts.length})`, boundary: "",
      }));
      candidates.push({ node, units: partUnits });
      units = aggregateLeafUnits(blocks, { ...node, start: (parts[Math.max(0, partAt)] as NavUnit).start, end: (parts[Math.max(0, partAt)] as NavUnit).end }, kTarget, kHard);
    }
    if (units.length >= 2 || node === path[path.length - 1]) candidates.push({ node, units });
  }
  // Fold rows beyond the cap: the row above the bottom nests inside the
  // current cell of the row above it; anything higher becomes breadcrumb.
  let breadcrumb: string[] = [];
  while (candidates.length > maxRows && candidates.length >= 2) {
    const folded = candidates.splice(candidates.length - 2, 1)[0] as Candidate;
    const host = candidates[candidates.length - 2] as Candidate | undefined;
    if (host === undefined) { candidates.unshift(folded); break; }
    const hostCurrent = host.units.findIndex((unit) => currentBlock >= unit.start && currentBlock < unit.end);
    const cell = host.units[Math.max(0, hostCurrent)] as NavUnit & { nested?: NavUnit[] };
    cell.nested = folded.units;
    void cell;
  }
  while (candidates.length > maxRows) {
    const dropped = candidates.shift() as Candidate;
    breadcrumb.push(`${dropped.node.number} ${dropped.node.title}`.trim());
  }
  if (candidates.length > 0 && breadcrumb.length === 0 && path.length > candidates.length) {
    const shown = new Set(candidates.map((candidate) => candidate.node));
    breadcrumb = path.filter((node) => !shown.has(node)).map((node) => `${node.number} ${node.title}`.trim());
  }
  const rows: NavRow[] = candidates.map((candidate) => {
    let current = candidate.units.findIndex((unit) => currentBlock >= unit.start && currentBlock < unit.end);
    const units = compressRow(candidate.units, Math.max(0, current), kHard);
    current = units.findIndex((unit) => currentBlock >= unit.start && currentBlock < unit.end);
    return { units, current };
  });
  // Boundary marks on the bottom row: where its linear sequence crosses
  // into a section, the mark carries that section's number (design: marks
  // are labels, not targets; tapping still moves by block or edge).
  const bottom = rows[rows.length - 1];
  if (bottom !== undefined) {
    const startNumbers = new Map(model.sections.map((section) => [section.start, section.number]));
    for (const unit of bottom.units) {
      const entered = startNumbers.get(unit.start);
      if (entered !== undefined && entered !== (path[path.length - 1] as DocumentSection).number) unit.boundary = entered;
      else if (unit.kind === "section") unit.boundary = "";
    }
  }
  return { kTarget, kHard, maxRows, breadcrumb, rows, prevSection, nextSection, address };
}

// A thread card's body cut into the units it may stop after (`#624 voice-page-modularity`, moved from
// the legacy script into view/channel; `#522 thread-card-max-lines`, `#533 card-split-atomic-blocks`):
// how many card lines each takes by the server's proxy (present/rows), what is kept, where the "…"
// goes, and the one line a block too tall for the card is stood for by.

import { CARD_INDENTED_CHARS, CARD_LINE_CHARS, cardProxyLines, proxyLinesOf } from "../../present/rows/index.ts";
import { sayingNodes, tagNameOf } from "../dom/index.ts";
import { renderedText } from "./text.ts";

/** One unit of a card's body (`cardUnits`). */
interface CardUnit {
  block: ChildNode;
  index: number;
  nodes: ChildNode[];
  lines: number;
  mark: number;
  whole: boolean;
  heading: boolean;
}

/** The block tags whose items, not themselves, are a card's units. */
const CARD_LIST_TAGS = new Set(["ul", "ol"]);

/**
 * The blocks a card never cuts into nor clamps (`#533 card-split-atomic-blocks`): one too tall to be on the
 * card is stood for there by one line (`cardStubText`) and drawn whole in the continuation.
 */
const CARD_WHOLE_TAGS = new Set(["table", "pre", "blockquote"]);

/** The blocks a card's "…" is written into, rather than after. */
const CARD_TEXT_TAGS = new Set(["p", "h1", "h2", "h3", "h4", "li"]);

/** Headings, which a card never ends on: each goes with the part it heads (`#551 headings-real-headings`). */
const CARD_HEADING_TAGS = new Set(["h1", "h2", "h3", "h4"]);

/** What one node says, as `renderedText` reads an element. */
const nodeWords = (node: Node): string => (node.nodeType === 3 ? String(node.textContent) : renderedText(node));

/**
 * The units of drawn body `box` a card may stop after, in order: each block at its top; each item of
 * a list there, rather than the list; and each line of a paragraph, its line break included. Each
 * with its block, its place in that block (`index`), its nodes, the card lines its words take, and
 * `mark`, the line a "…" after it takes of its own (1 after a block with no line of words to end:
 * code, a quote, a table, an item ending in a list of its own), else 0. A table counts a line a row and
 * code a line a line, as they draw them (`#533 card-split-atomic-blocks`); `whole` marks either, or a
 * quote, as a block the card never cuts into (`CARD_WHOLE_TAGS`); `heading`, a heading, which a card
 * never ends on (`CARD_HEADING_TAGS`).
 */
function cardUnits(box: Node): CardUnit[] {
  const units: CardUnit[] = [];
  const unit = (block: ChildNode, index: number, nodes: ChildNode[], width: number): void => {
    const words = nodes.map(nodeWords).join("");
    const last = nodes.length === 1 ? nodes[0] : null;
    const tag = tagNameOf(last);
    const inner = last && tag === "li" ? sayingNodes(last).pop() : null;
    const mark = last && (!CARD_TEXT_TAGS.has(tag) || (inner && CARD_LIST_TAGS.has(tagNameOf(inner)))) ? 1 : 0;
    const lines = tag === "table" ? Math.max(1, tableShape(last as Element).rows)
      : tag === "pre" ? Math.max(1, (String((last as Element).textContent || "") || renderedText(last as Element)).replace(/\n+$/, "").split("\n").length)
        : cardProxyLines(proxyLinesOf(words), width);
    units.push({ block, index, nodes, lines, mark, whole: CARD_WHOLE_TAGS.has(tag), heading: CARD_HEADING_TAGS.has(tag) });
  };
  for (const block of sayingNodes(box)) {
    const tag = tagNameOf(block);
    if (CARD_LIST_TAGS.has(tag)) {
      [...block.childNodes].filter((node) => node.nodeType === 1)
        .forEach((item, index) => unit(block, index, [item], CARD_INDENTED_CHARS));
    } else if (tag === "p") {
      let line: ChildNode[] = [];
      let index = 0;
      for (const node of [...block.childNodes]) {
        line.push(node);
        if (tagNameOf(node) === "br") {
          unit(block, index, line, CARD_LINE_CHARS);
          line = [];
          index += 1;
        }
      }
      if (line.length) unit(block, index, line, CARD_LINE_CHARS);
    } else {
      unit(block, 0, [block], tag === "blockquote" ? CARD_INDENTED_CHARS : CARD_LINE_CHARS);
    }
  }
  return units;
}

/**
 * Keep in body `box` only the units `keep` says, by their place among its units (`cardUnits`): the
 * rest taken out, a list or paragraph left with none taken out with them, a numbered list that lost
 * its first items numbered on from where they stopped, and a paragraph's last line break gone.
 */
function keepCardUnits(box: HTMLElement, keep: (at: number) => boolean): void {
  // Each block with units kept, and the place of its first kept one.
  const kept = new Map<ChildNode, number>();
  cardUnits(box).forEach((unit, at) => {
    if (keep(at)) {
      if (!kept.has(unit.block)) kept.set(unit.block, unit.index);
      return;
    }
    for (const node of unit.nodes) if (node.parentNode) node.parentNode.removeChild(node);
  });
  for (const block of sayingNodes(box)) {
    const tag = tagNameOf(block);
    if (!CARD_LIST_TAGS.has(tag) && tag !== "p") continue;
    if (!kept.has(block)) {
      box.removeChild(block);
      continue;
    }
    const first = kept.get(block) || 0;
    const start = Number.parseInt(String((block as Element).getAttribute("start") || ""), 10);
    if (tag === "ol" && first > 0) (block as Element).setAttribute("start", String((Number.isFinite(start) ? start : 1) + first));
    const nodes = [...block.childNodes];
    const last = nodes[nodes.length - 1];
    if (tag === "p" && last && tagNameOf(last) === "br") block.removeChild(last);
  }
}

/** Where a card's "…" goes at the `end` (or start) of `box`: the text block there, or null for none. */
function cardMarkTarget(box: Node, end: boolean): Element | null {
  const pick = (nodes: ChildNode[]): ChildNode | undefined => (end ? nodes[nodes.length - 1] : nodes[0]);
  let at = pick(sayingNodes(box));
  if (at && CARD_LIST_TAGS.has(tagNameOf(at))) at = pick([...at.childNodes].filter((node) => node.nodeType === 1));
  // A loose list's item holds paragraphs: the mark goes in the paragraph, on its line.
  if (at && tagNameOf(at) === "li") {
    const inner = pick(sayingNodes(at));
    if (inner && tagNameOf(inner) === "p") at = inner;
  }
  // A text block's tag says it is an element.
  return at && CARD_TEXT_TAGS.has(tagNameOf(at)) ? (at as Element) : null;
}

/**
 * The rows and columns of table `table`: its rows, header included, and the most cells in one of them.
 */
function tableShape(table: Element): { rows: number; columns: number } {
  const rows: ChildNode[] = [];
  const walk = (node: Node): void => {
    for (const kid of [...node.childNodes]) {
      if (kid.nodeType !== 1) continue;
      if (tagNameOf(kid) === "tr") rows.push(kid);
      else if (tagNameOf(kid) !== "table") walk(kid);
    }
  };
  walk(table);
  const cells = (row: ChildNode): number => [...row.childNodes].filter((kid) => ["td", "th"].includes(tagNameOf(kid))).length;
  return { rows: rows.length, columns: Math.max(0, ...rows.map(cells)) };
}

/**
 * The one line a card says for block `block`, too tall to be on it (`#533 card-split-atomic-blocks`):
 * "Table (7 columns × 6 rows)", or the first line of a code block or a quote.
 */
function cardStubText(block: Node): string {
  if (tagNameOf(block) === "table") {
    const { rows, columns } = tableShape(block as Element);
    return `Table (${columns} column${columns === 1 ? "" : "s"} × ${rows} row${rows === 1 ? "" : "s"})`;
  }
  const first = renderedText(block).split("\n").map((line) => line.trim()).find(Boolean);
  return first || (tagNameOf(block) === "pre" ? "Code" : "Quote");
}

export {
  CARD_HEADING_TAGS,
  CARD_LIST_TAGS,
  CARD_TEXT_TAGS,
  CARD_WHOLE_TAGS,
  cardMarkTarget,
  cardStubText,
  cardUnits,
  keepCardUnits,
  nodeWords,
  tableShape,
};
export type { CardUnit };

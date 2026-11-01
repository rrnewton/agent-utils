// Which row a selection quotes from (`#624 voice-page-modularity`, moved from the legacy script into
// view/channel; `#267 quote-reply`): the row a node is in, the row whose drawn body holds it, and whether
// the page draws any words between two points of a selection.

import { childByClass, nodeWithin } from "../dom/index.ts";

/** A row the legacy script drew with a Reply button on it. */
type ReplyRow = Element & { replyButton?: HTMLButtonElement };

/** The row `node` is in, while that row is on a list and has a Reply; else null. */
function replyRowOf(node: Node | null | undefined): HTMLLIElement | null {
  for (let at: Node | null | undefined = node; at; at = at.parentNode) {
    if ((at as ReplyRow).replyButton) return at.parentNode ? (at as HTMLLIElement) : null;
  }
  return null;
}

/** The row whose drawn body holds `node`, while that row is on a list and has a Reply; else null. */
function quotableRow(node: Node): HTMLLIElement | null {
  const row = replyRowOf(node);
  const body = row ? childByClass(row, "body") : null;
  return body && nodeWithin(body, node) ? row : null;
}

/** How many nodes `drawnWordsBetween` walks before it takes the stretch for one with words in it. */
const QUOTE_EDGE_NODES = 400;

/**
 * Whether the page draws any words between two points of the document, each a node and an offset as
 * a range's ends are, the first not after the second: text that is more than spaces, outside every
 * `hidden` element, in an element the engine gives a box (where it can say). Walked by hand, as
 * `nodeWithin` is, and given up as worded after `QUOTE_EDGE_NODES`. `#267 quote-reply`.
 */
function drawnWordsBetween(from: Node, fromOffset: number, to: Node, toOffset: number): boolean {
  const kids = (node: Node): NodeListOf<ChildNode> | never[] => node.childNodes || [];
  const leaf = (node: Node): boolean => kids(node).length === 0;
  const textOf = (node: Node): string => (node.nodeType === 3 || node.nodeType === 1 ? String(node.textContent || "") : "");
  const after = (node: Node): ChildNode | null => {
    for (let at: Node | null = node; at; at = at.parentNode) {
      if (at.nextSibling) return at.nextSibling;
    }
    return null;
  };
  const worded = (node: Node, text: string): boolean => {
    if (!/\S/.test(text)) return false;
    const box = (node.nodeType === 3 ? node.parentNode : node) as Element | null;
    return !box || typeof box.getClientRects !== "function" || box.getClientRects().length > 0;
  };
  const stop = leaf(to) ? to : kids(to)[toOffset] || after(to);
  let at: Node | null | undefined = null;
  if (leaf(from)) {
    if (from === stop) return worded(from, textOf(from).slice(fromOffset, toOffset));
    if (worded(from, textOf(from).slice(fromOffset))) return true;
    at = after(from);
  } else {
    at = kids(from)[fromOffset] || after(from);
  }
  for (let walked = 0; at; walked += 1) {
    if (walked > QUOTE_EDGE_NODES) return true;
    if (at === stop) return leaf(to) && worded(at, textOf(at).slice(0, toOffset));
    if ((at as HTMLElement).hidden === true) {
      if (nodeWithin(at, stop)) return false;
      at = after(at);
    } else if (leaf(at)) {
      if (worded(at, textOf(at))) return true;
      at = after(at);
    } else {
      at = at.firstChild;
    }
  }
  return false;
}

/**
 * The row one range of the selection quotes from, or null: the row whose drawn body holds all of it,
 * or holds one end of it where what lies between the body and the other end has no words drawn.
 *
 * That second case is a TRIPLE-CLICK. Chromium selects the paragraph and the break after it, and
 * where the paragraph is the last of its message it says so by ending the range at the very start of
 * the next row: the range's common ancestor is the list, as a selection across two rows would have
 * it, while nothing of the next row is selected or drawn as selected. A drag that runs on into the
 * row's own padding ends in the same way. A selection that really takes in the next row's time or
 * name, or this row's own header line, does have words there, and is not a quote.
 */
function rowQuotedBy(range: Range): HTMLLIElement | null {
  const whole = quotableRow(range.commonAncestorContainer);
  if (whole) return whole;
  const first = range.startContainer ? quotableRow(range.startContainer) : null;
  const last = range.endContainer ? quotableRow(range.endContainer) : null;
  if (first && !last) {
    const body = childByClass(first, "body") as Element;
    const end = (body.childNodes || []).length || String(body.textContent || "").length;
    return drawnWordsBetween(body, end, range.endContainer, range.endOffset) ? null : first;
  }
  if (last && !first) {
    const body = childByClass(last, "body") as Element;
    return drawnWordsBetween(range.startContainer, range.startOffset, body, 0) ? null : last;
  }
  return null;
}

export { QUOTE_EDGE_NODES, drawnWordsBetween, quotableRow, replyRowOf, rowQuotedBy };

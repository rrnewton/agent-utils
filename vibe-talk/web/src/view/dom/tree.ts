// Small walks over the page's own DOM that know nothing of columns, channels or messages (`#624
// voice-page-modularity`, moved from the legacy script; plan step 16, view/dom). Each takes the nodes it
// works on; none looks anything up by id.

/** A row that remembers the parts `childByClass` has found in it (`#532 shift-o-latency`). */
type PartsCache = Element & { partsFound?: Map<string, Element> };

/** A node, as far as walking up from it needs. */
type Walked = { readonly parentNode?: unknown; readonly nodeType?: number; readonly hidden?: boolean } | null | undefined;

/** Whether `node` is `root` or inside it. Walked by hand so it holds for any element. */
export function nodeWithin(root: unknown, node: unknown): boolean {
  for (let at = node as Walked; at; at = at.parentNode as Walked) {
    if (at === root) return true;
  }
  return false;
}

/** Whether `node` and every element around it are shown, as far as their `hidden` says. */
export function shownNode(node: unknown): boolean {
  for (let at = node as Walked; at && at.nodeType === 1; at = at.parentNode as Walked) {
    if (at.hidden === true) return false;
  }
  return true;
}

/**
 * The row's own archive control, found by walking the row rather than by selector.
 *
 * A row is a small tree this file built itself — a meta line and a body — so a walk is exact. It is
 * also the only lookup here that does not go through `el`, and going through `children` keeps it to
 * the same handful of DOM operations the rest of this page uses.
 *
 * `#226 page-energy-profile`. NOT INTO THE MESSAGE'S OWN TEXT. Since the server renders bodies
 * (`#217 markdown-blocks`) a row's text is most of its tree — paragraphs, lists, code — and none of
 * the parts this looks for is ever in it: a `.md` box holds the message and nothing of this file's,
 * and the sanitizer lets no class through that could pass for one of them. Every pass over the rows
 * asks this six to eight times a row, so walking every word of every message, splitting each class
 * attribute on the way, was a quarter of the page's work in live traffic. A class attribute that does
 * not even contain the name is passed over without being split.
 */
export function childByClass(row: unknown, className: string): HTMLElement | null {
  // Matched among the element's classes rather than against the whole attribute. The row being
  // read carries `msg-author reading-mark`, and an exact-string match stopped finding it the
  // moment the second class was added -- so the label could be set to "reading" and never set
  // back, which is precisely the bug that reached the suite.
  const has = (name: string): boolean => name === className || (name.includes(className) && name.split(/\s+/).includes(className));
  // `#532 shift-o-latency`. Found before, and still in the row with the class: a pass dressing every row
  // unpacked under the cards asks this for each part of every row, and walked them all each time.
  const parts = (row as PartsCache).partsFound;
  const known = parts ? parts.get(className) : undefined;
  if (known && has(String(known.className || "")) && nodeWithin(row, known)) return known as HTMLElement;
  const stack: Element[] = [];
  const push = (node: Element): void => {
    const kids = node.children;
    if (kids) for (let at = 0; at < kids.length; at += 1) stack.push(kids[at] as Element);
  };
  push(row as Element);
  while (stack.length > 0) {
    const node = stack.pop() as Element;
    const name = String(node.className || "");
    if (has(name)) {
      if (!parts) (row as PartsCache).partsFound = new Map();
      (row as PartsCache).partsFound!.set(className, node);
      return node as HTMLElement;
    }
    if (name !== "md") {
      push(node);
    }
  }
  return null;
}

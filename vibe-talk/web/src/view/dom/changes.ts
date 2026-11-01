// Small DOM operations the whole page shares (`#624 voice-page-modularity`, moved from the legacy script
// into view/dom): a node's tag, the child nodes that say something, and writes made only when they
// change something (`#532 shift-o-latency`: the browser counts a write of the same value as a change).

/** A node's tag, lower case, or "" for a text node. */
const tagNameOf = (node: Node | null | undefined): string =>
  String((node && (node as Partial<Element>).tagName) || "").toLowerCase();

/** `parent`'s child nodes that say something: its elements, and any text not white space. */
const sayingNodes = (parent: Node): ChildNode[] => [...parent.childNodes].filter((node) =>
  node.nodeType === 1 || (node.nodeType === 3 && String(node.textContent).trim() !== ""));

/** Set `node`'s text, unless it already says that. */
function setTextIfChanged(node: Node, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

/**
 * Set `node`'s attribute `name` to `value`, unless it already holds that. `#532 shift-o-latency`: the
 * browser counts a write of the same value as a change, and the passes that dress every row again
 * (`dressChannelRow`) would otherwise have every row styled again for nothing.
 */
function setAttributeIfChanged(node: Element, name: string, value: string): void {
  if (node.getAttribute(name) !== value) node.setAttribute(name, value);
}

/** Hide or show `node`, unless it already is (`setAttributeIfChanged`). */
function setHiddenIfChanged(node: HTMLElement, hidden: boolean): void {
  if (node.hidden !== hidden) node.hidden = hidden;
}

/** Set `node`'s classes to `name`, unless they already are (`setAttributeIfChanged`). */
function setClassNameIfChanged(node: Element, name: string): void {
  if (node.className !== name) node.className = name;
}

export { sayingNodes, setAttributeIfChanged, setClassNameIfChanged, setHiddenIfChanged, setTextIfChanged, tagNameOf };

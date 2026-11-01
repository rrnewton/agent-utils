// Class words on an element (`#624 voice-page-modularity`, moved from the legacy script into view/dom):
// one added or taken off without disturbing the rest, and the words an element wears.

/**
 * Add or remove ONE class word, leaving whatever else the element is wearing alone.
 *
 * The page's idiom everywhere else is to assign `className` outright, which is fine for an element
 * whose classes are all decided in one place. These rows are not that: a transcript row is `mine`
 * or `theirs`, a channel row carries `discord-message`, and the filter has an opinion about
 * neither.
 */
function setClassWord(element: Element, word: string, on: boolean): void {
  const words = element.className.split(/\s+/).filter((one) => one && one !== word);
  if (on) {
    words.push(word);
  }
  element.className = words.join(" ");
}

/** An element's class words. */
const classWords = (node: { readonly className?: unknown } | null | undefined): string[] => String((node && node.className) || "").split(/\s+/);

export { classWords, setClassWord };

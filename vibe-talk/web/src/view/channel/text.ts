// What a drawn body says, as text (`#624 voice-page-modularity`, moved from the legacy script into
// view/channel): what search matches and a thread is named by.

import { TEXT_BLOCKS } from "./hits.ts";

/**
 * What a drawn body SAYS, as text: what the reader sees, with a line end at each line break and at
 * the edge of each block. What search matches and a thread is named by — not the Markdown source,
 * whose asterisks and link targets are not on the screen.
 */
function renderedText(node: Node): string {
  let out = "";
  const walk = (at: Node): void => {
    for (const kid of at.childNodes) {
      if (kid.nodeType === 3) {
        out += kid.textContent;
        continue;
      }
      if (kid.nodeType !== 1) continue;
      const tag = String((kid as Element).tagName).toLowerCase();
      if (tag === "br") {
        out += "\n";
        continue;
      }
      const block = TEXT_BLOCKS.has(tag);
      if (block) out += "\n";
      walk(kid);
      if (block) out += "\n";
    }
  };
  walk(node);
  return out.replace(/[ \t]*\n\s*/g, "\n").trim();
}

export { renderedText };

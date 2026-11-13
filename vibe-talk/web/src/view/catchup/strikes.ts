// Citation-to-bullet mapping and the live strike for one drawn Discuss
// answer (#1388 discuss-summary-staleness-update, landing 1: LIVE
// STRIKE-THROUGH, page-only). The rendered answer's citation links are the
// map: each bullet (`li`) and each folded section (`details`) cites the
// messages its own links name (`m=provider:channel:message[:thread]`, the
// address `shape_answer` built and `answerTap` already parses). A bullet or
// section whose cited messages are ALL read is struck; one with no citations
// is never struck. Pure over a structural element shape (view/dom's fold
// idiom), so the mapping is testable without a page.

import { deepLinkRefFrom } from "../../foundation/index.ts";

/** The least an element must offer for the strike walk. */
export interface StrikeElement {
  tagName: string;
  className?: string;
  getAttribute?(name: string): string | null;
  children: ArrayLike<StrikeElement>;
}

/** A bullet whose cited messages are all read. */
export const STRUCK_BULLET_CLASS = "discuss-struck";
/** A section whose cited messages are all read (its heading is struck). */
export const STRUCK_SECTION_CLASS = "discuss-struck-section";
/** A bullet or section the latest Update added and not yet read (landing 2). */
export const FRESH_CLASS = "discuss-fresh";

/** Add or remove one class word, leaving the rest alone. */
function setWord(el: StrikeElement, word: string, on: boolean): void {
  const words = String(el.className ?? "").split(/\s+/).filter((one) => one !== "" && one !== word);
  if (on) words.push(word);
  el.className = words.join(" ");
}

/** Whether `el` now wears class `word`. */
export function hasWord(el: StrikeElement, word: string): boolean {
  return String(el.className ?? "").split(/\s+/).includes(word);
}

/**
 * The message ids `el`'s citation links name, in first-link order, deduped.
 * Only in-app citation addresses count: an `m=` ref the page's own parser
 * accepts. Issue links and any other anchor name nothing here.
 */
export function citationIdsOf(el: StrikeElement): string[] {
  const found: string[] = [];
  const walk = (node: StrikeElement): void => {
    if (node.tagName.toLowerCase() === "a" && typeof node.getAttribute === "function") {
      const href = node.getAttribute("href") ?? "";
      const queryAt = href.indexOf("?");
      if (!href.startsWith("http") && queryAt >= 0) {
        const ref = deepLinkRefFrom(new URLSearchParams(href.slice(queryAt + 1)).get("m") ?? "");
        if (ref !== null && !found.includes(ref.message)) found.push(ref.message);
      }
    }
    for (const kid of Array.from(node.children ?? [])) walk(kid);
  };
  walk(el);
  return found;
}

/** Every element of `tag` under `root`, `root` itself included, in tree order. */
function allOf(root: StrikeElement, tag: string): StrikeElement[] {
  const out: StrikeElement[] = [];
  const walk = (node: StrikeElement): void => {
    if (node.tagName.toLowerCase() === tag) out.push(node);
    for (const kid of Array.from(node.children ?? [])) walk(kid);
  };
  walk(root);
  return out;
}

/**
 * Restate the strike (and any fresh green) over one drawn answer. `isRead`
 * answers for the page's read set NOW, so a recompute after any mark moves
 * the strike live; `fresh` names the latest Update's cited ids (empty before
 * landing 2 draws any). Struck wins over fresh: a read section is done, not
 * new. Folds are untouched — only class words move, so an open section
 * stays open and its heading stays tappable. Returns what was struck.
 */
export function applyStrikes(
  root: StrikeElement,
  isRead: (id: string) => boolean,
  fresh: ReadonlySet<string> = new Set(),
): { bullets: number; sections: number } {
  let bullets = 0;
  for (const li of allOf(root, "li")) {
    const cited = citationIdsOf(li);
    const struck = cited.length > 0 && cited.every((id) => isRead(id));
    const isFresh = !struck && cited.some((id) => fresh.has(id));
    setWord(li, STRUCK_BULLET_CLASS, struck);
    setWord(li, FRESH_CLASS, isFresh);
    if (struck) bullets += 1;
  }
  let sections = 0;
  for (const section of allOf(root, "details")) {
    const cited = citationIdsOf(section);
    const struck = cited.length > 0 && cited.every((id) => isRead(id));
    const isFresh = !struck && cited.some((id) => fresh.has(id));
    setWord(section, STRUCK_SECTION_CLASS, struck);
    setWord(section, FRESH_CLASS, isFresh);
    if (struck) sections += 1;
  }
  return { bullets, sections };
}

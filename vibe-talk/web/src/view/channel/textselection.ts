// The reader's text selection in a list, kept across a rebuild and put back on the rebuilt
// row (`#624 voice-page-modularity`, moved from the legacy script; `#1209
// selection-survives-updates`). Words and order are as they were in the script. The row ids
// are the page's (`idsOf`), as is the element lookup for the scroll area; the listeners are
// taken down by the mount's `unmount`, through web/src/view/dom's listening helper.

import { createListening } from "../dom/index.ts";

/** A selection kept across a rebuild: its row's message and its ends into the row's words. */
export interface KeptTextSelection {
  id: string;
  start: number;
  end: number;
}

/** What keeping a text selection asks of the page. */
export interface TextSelectionDependencies {
  /** The element with this id, here for the scroll area the live selection is read in (`el`). */
  el(id: string): HTMLElement;
  /** The ids of a row's message or messages, or none for a row that is not one (`idsOf`). */
  idsOf(row: Element | null | undefined): string[];
}

/** Keeping and restoring the reader's text selection, mounted. */
export interface TextSelection {
  textSelectionInNow(area: HTMLElement): KeptTextSelection | null;
  textSelectionIn(area: HTMLElement): KeptTextSelection | null;
  restoreTextSelectionIn(area: HTMLElement, kept: KeptTextSelection): void;
  /** Take down the document listeners this mount added. */
  unmount(): void;
}

/** Mount the keeping of the reader's selection, its listeners first, in the page's order. */
export function mountTextSelection(deps: TextSelectionDependencies): TextSelection {
  const { listen, unmount } = createListening();
  /**
   * The reader's text selection in `area`, for keeping across a rebuild (`#1209
   * selection-survives-updates`): the message it stands in and the ends' distances into that
   * row's words, or null where it does not stand in one row of the list.
   *
   */
  /** The last live selection in a list row, kept as it changes: a switch or a rebuild takes the
   * live one before any capture at rebuild time can see it (`#1209`). */
  let keptTextSelection: KeptTextSelection | null = null;

  listen<Event>(document, "selectionchange", () => {
    const live = textSelectionInNow(deps.el("scroll-area"));
    if (live) keptTextSelection = live;
  });
  // The reader's own hand lets the kept selection go: a rebuild after it must not resurrect it.
  for (const type of ["pointerdown", "keydown"]) {
    listen<Event>(document, type, () => { keptTextSelection = null; }, { capture: true, passive: true });
  }

  /**
   * The reader's live text selection in `area`, as it stands now (no kept fallback).
   *
   */
  function textSelectionInNow(area: HTMLElement): KeptTextSelection | null {
    const sel = typeof getSelection === "function" ? getSelection() : null;
    if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
    const range = sel.getRangeAt(0);
    const rowOf = (node: Node | null): Element | null => {
      let at: Node | null = node && (node.nodeType === 1 ? node : node.parentElement);
      while (at && at.parentNode) {
        const parent = at.parentNode as Element;
        if (parent.id === "discord-log" || parent.id === "pinned-log" || parent.id === "transcript") return at as Element;
        at = parent;
      }
      return null;
    };
    const startRow = rowOf(range.startContainer);
    const endRow = rowOf(range.endContainer);
    let inArea = false;
    for (let at: Node | null = startRow; at; at = at.parentNode) if (at === area) { inArea = true; break; }
    if (!startRow || startRow !== endRow || !inArea) return null;
    const ids = deps.idsOf(startRow);
    if (!ids.length) return null;
    const before = document.createRange();
    before.setStart(startRow, 0);
    before.setEnd(range.startContainer, range.startOffset);
    const until = document.createRange();
    until.setStart(startRow, 0);
    until.setEnd(range.endContainer, range.endOffset);
    return { id: ids[0] as string, start: before.toString().length, end: until.toString().length };
  }

  function textSelectionIn(area: HTMLElement): KeptTextSelection | null {
    const live = textSelectionInNow(area);
    if (live) return live;
    // Nothing live: the last selection kept. Its row may not stand in this list yet (a switch
    // drew another view's first): putting it back looks for the row after the rebuild, and
    // finds nothing where it never comes back.
    return keptTextSelection;
  }

  /**
   * Put a selection kept by `textSelectionIn` back on the rebuilt row in `area`, by its words.
   *
   */
  function restoreTextSelectionIn(area: HTMLElement, kept: KeptTextSelection): void {
    keptTextSelection = kept;
    const sel = typeof getSelection === "function" ? getSelection() : null;
    if (!sel) return;
    const rows = area.id === "discord-log" || area.id === "pinned-log" || area.id === "transcript"
      ? [...area.children] : [...area.querySelectorAll("li")];
    const row = rows.find((candidate) => deps.idsOf(candidate).includes(kept.id));
    if (!row) return;
    const point = (at: number): [Node, number] | null => {
      let left = at;
      const visit = (node: Node): [Node, number] | null => {
        if (node.nodeType === 3) {
          const text = node.textContent as string;
          if (left <= text.length) return [node, left];
          left -= text.length;
          return null;
        }
        for (const child of node.childNodes) {
          const found = visit(child);
          if (found) return found;
        }
        // An element holding its words itself, with no text node under it: the point is in it.
        if (node.childNodes.length === 0 && node.textContent) return [node, Math.min(left, node.textContent.length)];
        return null;
      };
      return visit(row);
    };
    const from = point(kept.start);
    const to = point(kept.end);
    if (!from || !to) return;
    const range = document.createRange();
    range.setStart(from[0], from[1]);
    range.setEnd(to[0], to[1]);
    sel.removeAllRanges();
    sel.addRange(range);
  }

  return { textSelectionInNow, textSelectionIn, restoreTextSelectionIn, unmount };
}

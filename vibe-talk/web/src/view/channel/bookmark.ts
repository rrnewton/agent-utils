// The reading bookmark's line as the DOM draws it (`#420 bookmark-clear`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): made once on its row and kept, as a collapsed run's
// line is, and moved to the row the bookmark stands on. Where that is, is present/rows' `bookmarkSpot`;
// what the X does, the page's.

import { childByClass } from "../dom/index.ts";

/** A row as the bookmark's line reads and writes it: the element, and the lines it keeps on itself. */
export type BookmarkedRow = HTMLElement & { bookmarkLine?: HTMLElement | null; seamLine?: Node | null };

/** Where the line stands: on `row`, above it or below it. */
export interface BookmarkPlace {
  readonly row: BookmarkedRow;
  readonly place: string;
}

/** What the line asks of the page. */
export interface BookmarkLinesDependencies {
  /** The X's name and title. */
  readonly clearName: string;
  /**
   * The listener for a press of `clear`, the X on `row`'s line, made as the line is made (so it belongs
   * to the column the row was drawn in): take the bookmark away.
   */
  clearListener(row: BookmarkedRow, clear: HTMLElement): (event: Event) => void;
}

/** The bookmark's line: made on a row, and drawn where it stands. */
export interface BookmarkLines {
  /** The line on `row`, made once and kept on it. */
  bookmarkLineOf(row: BookmarkedRow): HTMLElement;
  /** The line taken off every row of `drawn` but `target`'s, and put on `target`'s row, as it says. */
  drawBookmarkLine(drawn: readonly BookmarkedRow[], target: BookmarkPlace | null): void;
}

/** The bookmark's line, its X handed to the page. */
export function createBookmarkLines(deps: BookmarkLinesDependencies): BookmarkLines {
  /**
   * The line on `row`, made once and kept on it, as a collapsed run's is (`readRunLine`): the word, a
   * separator to a screen reader, and the X at the line's end that takes the bookmark away (`#420
   * bookmark-clear`). The X is a sibling of the word, never inside a separator, whose children a
   * screen reader does not reach. It acts in the column the row was drawn in.
   */
  const bookmarkLineOf = (row: BookmarkedRow): HTMLElement => {
    if (row.bookmarkLine) return row.bookmarkLine;
    const line = document.createElement("div");
    line.className = "bookmark-line";
    const word = document.createElement("span");
    word.className = "bookmark-word";
    word.setAttribute("role", "separator");
    word.setAttribute("aria-label", "Bookmark");
    word.textContent = "Bookmark";
    const clear = document.createElement("button");
    clear.className = "bookmark-clear";
    clear.setAttribute("type", "button");
    clear.setAttribute("aria-label", deps.clearName);
    clear.setAttribute("title", deps.clearName);
    const mark = document.createElement("span");
    mark.className = "bookmark-clear-mark";
    mark.setAttribute("aria-hidden", "true");
    clear.append(mark);
    clear.addEventListener("click", deps.clearListener(row, clear));
    line.append(word, clear);
    row.bookmarkLine = line;
    return line;
  };

  /** The row's ⋯ menu item, saying what it would do there. */
  const word = (row: BookmarkedRow, above: boolean): void => {
    const item = childByClass(row, "row-bookmark-button");
    const said = above ? "Remove bookmark" : "Bookmark above";
    if (item && item.textContent !== said) item.textContent = said;
  };

  const drawBookmarkLine = (drawn: readonly BookmarkedRow[], target: BookmarkPlace | null): void => {
    for (const row of drawn) {
      if (target && row === target.row) continue;
      row.removeAttribute("data-bookmark");
      if (row.bookmarkLine && row.bookmarkLine.parentNode === row) row.removeChild(row.bookmarkLine);
      word(row, false);
    }
    if (!target) return;
    target.row.setAttribute("data-bookmark", target.place);
    word(target.row, target.place === "above");
    const line = bookmarkLineOf(target.row);
    if (line.parentNode !== target.row) {
      const row = target.row;
      const first = row.firstChild;
      // Under a day's or a gap's line, which stands above the bookmark's.
      row.insertBefore(line, first !== null && first === row.seamLine ? first.nextSibling : first);
    }
  };

  return { bookmarkLineOf, drawBookmarkLine };
}

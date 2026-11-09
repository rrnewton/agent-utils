// The reading column's line count (`#55 voice-desktop-app`, `#244 elision-word-count`,
// `#47 scrollback-stability`; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel for the plan's step 17): every drawn fold's hidden-word count worked out again
// under the count now in force, and a new count applied between the scroll keepers. Which
// layout is in force is decided entirely in CSS; this code only ever sets a number, in
// characters. The folds, the columns and the keepers are handed in; the words are the page's.

import { forgetElisionCounts } from "../../present/rows/index.ts";
import { FOLD_LISTS } from "./folding.ts";
import type { FoldEntry } from "./folding.ts";
import { foldedTextLine } from "./fold.ts";

/** What the reading column is handed by the page. */
export interface ReadingColumnDependencies<Column, ScrollMark> {
  foldsOf(list: Element | null): FoldEntry[];
  setFolded(entry: FoldEntry, folded: boolean): void;
  isFolded(entry: FoldEntry): boolean;
  collapsedLines: { current(): number; apply(value: unknown): number };
  savedCollapsedLines(count: number): void;
  stepCollapsedLinesPreference(direction: 1 | -1): void;
  screen(): string;
  mainColumn(): Column;
  sideColumn(): Column;
  inColumn<T>(column: Column, act: () => T): T;
  captureScroll(): ScrollMark;
  restoreScroll(mark: ScrollMark): void;
  sideShowing(): boolean;
  keepingCardPlace(act: () => void): void;
  defaultCollapsedLines: number;
  documentById(id: string): Element | null;
}

/** The reading column's count, as the page and Settings call it. */
export interface ReadingColumn {
  refreshElisionLines(): void;
  collapsedLinesChanged(value: unknown): number;
  stepCollapsedLines(direction: 1 | -1): void;
}

/** Make the reading column from the page's folds, columns and scroll keepers. */
export function createReadingColumn<Column, ScrollMark>(deps: ReadingColumnDependencies<Column, ScrollMark>): ReadingColumn {
  /**
   * Work every drawn fold's hidden-word count out again under the count now in force. A label made
   * at three lines and left at eight would claim words the reader can see — the one thing the label
   * may never do (`#244 elision-word-count`) — and a channel redraw would only mend the channel's
   * rows, on the next poll, and the transcript's never. So the rows on the page are recounted where
   * they stand, from what each draws (`foldedTextLine`), and put back through `setFolded` so the
   * new line is shown exactly where the old one was.
   */
  function refreshElisionLines(): void {
    forgetElisionCounts();
    // And the rows unpacked under their cards, and Main's among them (`#404`, `#419`): drawn into
    // the cards' list rather than a column's log, so `FOLD_LISTS` never names them, and a label left
    // there is a label claiming words a raised count has put on the screen.
    for (const id of [...FOLD_LISTS, "side-cards"]) {
      for (const entry of deps.foldsOf(deps.documentById(id))) {
        const fresh = foldedTextLine(entry.body, entry.foldText, deps.collapsedLines.current());
        const old = entry.elided;
        if (old && fresh) old.replaceWith(fresh);
        else if (old && !fresh) old.remove();
        else if (!old && fresh && entry.body.parentNode) {
          entry.body.parentNode.insertBefore(fresh, entry.body.nextSibling);
        }
        entry.elided = fresh;
        deps.setFolded(entry, deps.isFolded(entry));
      }
    }
  }

  /**
   * Apply a new count and say, in Settings, whether it will survive a reload. Every collapsed row
   * on the page changes height at once, so the change is made between the scroll keepers: both
   * columns' lists (`captureScroll` and `restoreScroll`, `#47 scrollback-stability`) and, while the
   * right column or the Threads view shows the thread cards instead of a thread, the cards' own
   * (`keepingCardPlace`) — every raw card and every unpacked row under it changes height too. A
   * reader scrolled up any list keeps the message or card they are on, and a reader at the newest
   * line stays on it. Off the main screen — the slider's own home in Settings — there is no list on
   * screen to hold, and the count is simply put on.
   */
  function collapsedLinesChanged(value: unknown): number {
    const apply = (): number => {
      const n = deps.collapsedLines.apply(value);
      refreshElisionLines();
      deps.savedCollapsedLines(n);
      return n;
    };
    if (deps.screen() !== "main") return apply();
    const held: Array<[Column, ScrollMark]> = [[deps.mainColumn(), deps.inColumn(deps.mainColumn(), deps.captureScroll)]];
    if (deps.sideShowing()) held.push([deps.sideColumn(), deps.inColumn(deps.sideColumn(), deps.captureScroll)]);
    let n = deps.defaultCollapsedLines;
    deps.keepingCardPlace(() => { n = apply(); });
    for (const [column, mark] of held) deps.inColumn(column, () => deps.restoreScroll(mark));
    return n;
  }

  /**
   * One line fewer (-1) or more (+1): what the slider's − and + do, through `collapsedLinesChanged`,
   * the slider's own path, as `textSize.step` steps the text size.
   */
  function stepCollapsedLines(direction: 1 | -1): void {
    deps.stepCollapsedLinesPreference(direction);
  }

  return { refreshElisionLines, collapsedLinesChanged, stepCollapsedLines };
}

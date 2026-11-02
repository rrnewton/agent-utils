// The lines between a list's rows as the DOM draws them (`#249 search-highlight`, `#250
// date-separators`, `#439 search-unmatched-separators`; `#624 voice-page-modularity`, moved from the
// legacy script into view/channel). A line is a child of the row it stands above (`.row-seam`), or
// below for a gap after the last row shown, made once and kept on the row: a gap's line is a button,
// a day's a separator. What each line says is present/rows' `seamPlan`; what a press of a gap's line
// opens is the page's, passed in.

import { lineLabel } from "../../present/rows/index.ts";
import type { Seam } from "../../present/rows/index.ts";

/** A row as the lines read and write it: the element, and the lines it keeps on itself. */
export type SeamedRow = HTMLElement & {
  seamLine?: HTMLElement | null;
  seamBelowLine?: HTMLElement | null;
  seamRefocus?: boolean;
};

/** A gap's line, which remembers which kind of gap it opens. */
export type GapLine = HTMLElement & { gapCards?: boolean; gapUnpack?: boolean };

/** A line as `placeSeam` takes it: the plan's `Seam`, and for a gap, which kind of gap it opens. */
export interface PlacedSeam extends Seam {
  /** A gap among the thread cards (`drawCardGaps`), counting threads. */
  readonly cards?: boolean;
  /** A gap among a thread's unpacked rows. */
  readonly unpack?: boolean;
}

/** What the lines ask of the page. */
export interface SeamLinesDependencies {
  /**
   * The listener for a press of the gap's line `line` on `row`, above it (`below` false) or below it,
   * made as the line is made (so it belongs to the column the line was drawn in): open what it stands
   * for, which `line.gapCards` and `line.gapUnpack` say as it was last drawn.
   */
  gapListener(row: SeamedRow, below: boolean, line: GapLine): (event: Event) => void;
}

/** The lines between rows: make, check and place them. */
export interface SeamLines {
  /** The line on `row` above it (`below` false) or below it, made once and kept on it. */
  seamLineOf(row: SeamedRow, below: boolean, seam: PlacedSeam | null): HTMLElement;
  /** Whether `row` has, above or below it, exactly the line `seam` says, or none for null. */
  seamDrawn(row: SeamedRow, seam: PlacedSeam | null, below: boolean): boolean;
  /** Put `row`'s line above it, or below it, as `seam` says, or take it away for null. */
  placeSeam(row: SeamedRow, seam: PlacedSeam | null, below: boolean): void;
}

/** The classes of a line above or below its row, a gap among the cards marked as such. */
const seamClasses = (below: boolean, seam: PlacedSeam | null): string =>
  (below ? "row-seam row-seam-below" : "row-seam") + (seam && seam.cards ? " card-seam" : "");

/** The lines between rows, a press of a gap's line handed to the page. */
export function createSeamLines(deps: SeamLinesDependencies): SeamLines {
  /**
   * The line on `row`. A gap's line is a button (`#439 search-unmatched-separators`), as a collapsed
   * run's is; a day's stays a separator. A row whose line changed kind (a filter closed over the same
   * boundary) gets the other element.
   */
  const seamLineOf = (row: SeamedRow, below: boolean, seam: PlacedSeam | null): HTMLElement => {
    const control = seam !== null && seam.kind === "gap";
    let held = (below ? row.seamBelowLine : row.seamLine) as GapLine | null | undefined;
    if (held && (String(held.tagName).toLowerCase() === "button") !== control) {
      // A keyboard tabbed onto the line going is left on its row by `placeSeam`, not on nothing.
      if (document.activeElement === held) row.seamRefocus = true;
      if (held.parentNode === row) row.removeChild(held);
      held = null;
      if (below) row.seamBelowLine = null;
      else row.seamLine = null;
    }
    if (held) {
      held.gapCards = Boolean(seam && seam.cards);
      held.gapUnpack = Boolean(seam && seam.unpack);
      const classes = seamClasses(below, seam);
      if (held.className !== classes) held.className = classes;
      return held;
    }
    const line = document.createElement(control ? "button" : "div") as GapLine;
    line.className = seamClasses(below, seam);
    line.gapCards = Boolean(seam && seam.cards);
    line.gapUnpack = Boolean(seam && seam.unpack);
    if (control) {
      line.setAttribute("type", "button");
      line.addEventListener("click", deps.gapListener(row, below, line));
    } else {
      line.setAttribute("role", "separator");
    }
    if (below) row.seamBelowLine = line;
    else row.seamLine = line;
    return line;
  };

  const seamDrawn = (row: SeamedRow, seam: PlacedSeam | null, below: boolean): boolean => {
    const held = (below ? row.seamBelowLine : row.seamLine) as GapLine | null | undefined;
    const drawn = Boolean(held && held.parentNode === row);
    if (!seam) return !drawn;
    return drawn && held !== null && held !== undefined && held.textContent === seam.words &&
      row.getAttribute(below ? "data-seam-below" : "data-seam") === seam.kind &&
      (String(held.tagName).toLowerCase() === "button") === (seam.kind === "gap") &&
      Boolean(held.gapCards) === Boolean(seam.cards) && Boolean(held.gapUnpack) === Boolean(seam.unpack);
  };

  const placeSeam = (row: SeamedRow, seam: PlacedSeam | null, below: boolean): void => {
    if (seamDrawn(row, seam, below)) return;
    const attribute = below ? "data-seam-below" : "data-seam";
    if (!seam) {
      const held = below ? row.seamBelowLine : row.seamLine;
      const focused = held && document.activeElement === held;
      if (held && held.parentNode === row) row.removeChild(held);
      row.removeAttribute(attribute);
      if (focused) {
        row.setAttribute("tabindex", "-1");
        row.focus({ preventScroll: true });
      }
      return;
    }
    const line = seamLineOf(row, below, seam);
    if (line.textContent !== seam.words) {
      line.textContent = seam.words;
      // A gap's line is pressed, so its name says the act, as a collapsed run's title does.
      const label = seam.kind === "gap" ? `Show ${lineLabel(seam.words)}` : lineLabel(seam.words);
      line.setAttribute("aria-label", label);
      if (seam.kind === "gap") line.setAttribute("title", label);
    }
    row.setAttribute(attribute, seam.kind);
    if (line.parentNode !== row) {
      // Above: first, ahead of the bookmark's line, which sits nearer the row. Below: last. Put in, not
      // the row's children written again: that would take every one of them out of the document and
      // back, and the browser drops the focus and the selection of whatever it takes out — a reader
      // tabbed onto a row's Reply when midnight draws its day's rule would be left on nothing.
      if (below) row.append(line);
      else row.insertBefore(line, row.firstChild);
    }
    if (row.seamRefocus) {
      row.seamRefocus = false;
      const target = seam.kind === "gap" ? line : row;
      target.setAttribute("tabindex", "-1");
      target.focus({ preventScroll: true });
    }
  };

  return { seamLineOf, seamDrawn, placeSeam };
}

// The draggable separator between the desk's two columns (`#1186 two-column-splitter`; the owner,
// 2026-11-09: "make it possible to grab and drag the middle separator to change the width of the two
// columns"). It follows the reading-width grip's shape (web/src/view/settings `mountReadingWidth`):
// a `role=separator` the arrow keys move, the choice kept on the device and stored once at the end
// of a drag. The separator's own element only draws the line and takes the focus: the drag is
// claimed from any press that lands on the boundary, ahead of the press's own business, so the
// wheel, the scrollbars and every press elsewhere keep their native targets.
//
// The split is a percentage of the main screen's width, written as `--two-column-split`, which the
// two-column grid in web/voice.css reads; the stylesheet still decides whether two columns show at
// all, and this code only ever sets a number while they do.
//
// DRAGGING SCROLLS NEITHER COLUMN. A width change reflows every row, so each move first reads where
// each column's reader is (`readerPlace`, the place a change of layout reads before it draws a
// column at another width) and puts it back after (`keepReaderPlace`): the anchor row stays at the
// same viewport height while the rows around it re-wrap, and no other scroll writer runs.

import { createListening } from "../dom/index.ts";

/** The narrowest either column may be dragged to, in CSS pixels. */
const MIN_COLUMN_PX = 300;

/** How far either side of the boundary's line a press still claims the drag, in CSS pixels. */
const CLAIM_PX = 8;

/** Where one column's reader is, as the splitter keeps it across a width change. */
export type SplitterPlace = unknown;

/** What the column splitter is handed by the page. */
export interface ColumnSplitterDependencies {
  /** The element with this id, from the left column's side of the page (`leftEl`). */
  lookup(id: string): HTMLElement;
  /** Keep the split on the device; may throw where storage is refused. */
  keep(value: number): void;
  /** A split this page will apply: the device setting's clamp, handed in. */
  clampSplit(value: unknown): number;
  /** The split a double-click (and no choice yet) restores. */
  defaultSplit: number;
  /** Whether two columns are on screen now. */
  columnsOn(): boolean;
  /** The left and right columns, as the layout works on them. */
  mainColumn(): unknown;
  sideColumn(): unknown;
  /** Run `act` while working on `column`, and return what it returns. */
  inColumn<T>(column: unknown, act: () => T): T;
  /** Where the reader of the column being worked on is. */
  readerPlace(): SplitterPlace;
  /** Put that reader back where they were, after the width changed. */
  keepReaderPlace(place: SplitterPlace): boolean;
  /** The links between the columns follow the widths. */
  scheduleColumnLinks(): void;
}

/** The column splitter's named commands. */
export interface ColumnSplitter {
  /** Put a split on the page. Returns the split actually applied, which is the clamped one. */
  apply(value: unknown): number;
  /** Apply a new split and keep it on the device. */
  changed(value: unknown): number;
  /** Back to the default split, kept. */
  reset(): number;
  /** The split in force now. */
  split(): number;
  /** A press on the main screen: true (and a drag begins) when it lands on the boundary. */
  down(event: PointerEvent | undefined): boolean;
  /** The pointer moved over the main screen: a drag follows it, else the boundary's cursor. */
  move(event: PointerEvent | undefined): void;
  /** The pointer let go: the split it stopped at is kept. */
  up(): void;
  /** A key on the separator. */
  key(event: KeyboardEvent | undefined): void;
  /** A double-click on the main screen: true (and the default split) when it lands on the boundary. */
  doubleClick(event: MouseEvent | undefined): boolean;
}

/** The column splitter, made where the page kept the split. */
export function createColumnSplitter(deps: ColumnSplitterDependencies): ColumnSplitter {
  // The split currently in force, kept here rather than read back out of the DOM: reading a custom
  // property back gives you the string that was written, and a drag would then be parsing its own
  // output forty times a second.
  let split = deps.defaultSplit;
  // A drag, from its press: the main screen's box, measured once — re-measuring mid-drag would
  // feed the drag its own output, as the width grip's centre would — and where each column's
  // reader was at the press, which every move of the drag pins back to: a box that refits as the
  // focus leaves it settles a frame into the drag, and places read mid-drag would pin the drift.
  let drag: { left: number; width: number; places: { main: SplitterPlace; side: SplitterPlace } } | null = null;
  /** The separator's keys, each the split it moves to from the one in force: the width grip's
   * table turned sideways (Left narrows the left column), with Home and End at the clamp's ends. */
  const keys: { readonly [key: string]: (() => number) | undefined } = {
    ArrowLeft: () => split - 1,
    ArrowRight: () => split + 1,
    ArrowDown: () => split - 1,
    ArrowUp: () => split + 1,
    PageDown: () => split - 5,
    PageUp: () => split + 5,
    Home: () => deps.clampSplit(0),
    End: () => deps.clampSplit(100),
  };

  function apply(value: unknown): number {
    split = deps.clampSplit(value);
    deps.lookup("screen-main").style.setProperty("--two-column-split", `${split}%`);
    const separator = deps.lookup("column-splitter");
    separator.setAttribute("aria-valuenow", String(split));
    separator.setAttribute("aria-valuetext", `left column ${split} percent`);
    return split;
  }

  function changed(value: unknown): number {
    const applied = apply(value);
    deps.scheduleColumnLinks();
    try {
      deps.keep(applied);
    } catch (_error) {
      // Storage refused: the split holds until a reload, as a refused reading width does.
    }
    return applied;
  }

  /** Where each column's reader is, read before the width changes. */
  function capture(): { main: SplitterPlace; side: SplitterPlace } {
    return {
      main: deps.inColumn(deps.mainColumn(), () => deps.readerPlace()),
      side: deps.inColumn(deps.sideColumn(), () => deps.readerPlace()),
    };
  }

  /** Put each column's reader back where they were, now that the rows reflowed. */
  function restore(places: { main: SplitterPlace; side: SplitterPlace }): void {
    deps.inColumn(deps.mainColumn(), () => deps.keepReaderPlace(places.main));
    deps.inColumn(deps.sideColumn(), () => deps.keepReaderPlace(places.side));
  }

  /** The split `clientX` asks for inside the main screen `box` wide, clamped to a column a side. */
  function splitFor(clientX: number, box: { left: number; width: number }): number {
    if (!(box.width > 0)) return split;
    const floor = Math.max(deps.clampSplit(0), (MIN_COLUMN_PX / box.width) * 100);
    const ceil = Math.min(deps.clampSplit(100), 100 - (MIN_COLUMN_PX / box.width) * 100);
    const asked = ((clientX - box.left) / box.width) * 100;
    return Math.min(ceil, Math.max(floor, asked));
  }

  /** Where the boundary stands, in the main screen `box`: the split's share of its width. */
  function boundaryX(box: { left: number; width: number }): number {
    return box.left + (box.width * split) / 100;
  }

  /** Whether `event` lands on the boundary, within the claim either side of its line. */
  function onBoundary(event: PointerEvent | MouseEvent): boolean {
    if (!deps.columnsOn()) return false;
    const box = deps.lookup("screen-main").getBoundingClientRect();
    return Math.abs(event.clientX - boundaryX(box)) <= CLAIM_PX;
  }

  return {
    apply,
    changed,
    reset: () => changed(deps.defaultSplit),
    split: () => split,
    down(event) {
      if (!event || !onBoundary(event)) return false;
      const box = deps.lookup("screen-main").getBoundingClientRect();
      drag = { left: box.left, width: box.width, places: capture() };
      const screen = deps.lookup("screen-main");
      if (event.pointerId !== undefined && screen.setPointerCapture) {
        // The drag is the screen's until it lets go, wherever the pointer wanders.
        try { screen.setPointerCapture(event.pointerId); } catch (_error) { /* no pointer to hold */ }
      }
      const separator = deps.lookup("column-splitter") as HTMLElement & { focus?: (options?: unknown) => void };
      if (separator.focus) separator.focus({ preventScroll: true });
      return true;
    },
    move(event) {
      if (!event) return;
      if (!drag) {
        // Resting on the boundary is col-resize, and nothing of the drag: the cursor alone.
        const screen = deps.lookup("screen-main");
        const wanted = onBoundary(event) ? "col-resize" : "";
        if (screen.style.cursor !== wanted) screen.style.cursor = wanted;
        return;
      }
      const places = drag.places;
      apply(splitFor(event.clientX, drag));
      restore(places);
      // What settles a frame into the drag — a box refitting as the focus left it, the browser's
      // own scroll anchoring — moves a column after this pin. Pin the same places again once
      // that frame has laid out, so the row ends where the reader had it at the press.
      if (typeof requestAnimationFrame === "function") {
        requestAnimationFrame(() => restore(places));
      }
      deps.scheduleColumnLinks();
    },
    up() {
      if (!drag) return;
      drag = null;
      // Stored at the END of the drag, not on every frame, as the width grip stores its width.
      changed(split);
    },
    key(event) {
      const next = event && keys[event.key];
      if (!next) return;
      if (event.preventDefault) {
        event.preventDefault(); // The arrows would otherwise scroll the list behind it.
      }
      changed(next());
    },
    doubleClick(event) {
      if (!event || !onBoundary(event)) return false;
      changed(deps.defaultSplit);
      return true;
    },
  };
}

/** What the separator's listeners ask of the page. */
export interface ColumnSplitterControlsDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** A press, a move, a let-go, a key, or a double-click: the first and last say whether they claimed the boundary. */
  down(event: PointerEvent): boolean;
  move(event: PointerEvent): unknown;
  up(): unknown;
  key(event: KeyboardEvent): unknown;
  doubleClick(event: MouseEvent): boolean;
}

/** The separator's listeners, mounted. */
export interface ColumnSplitterControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the separator's controls: presses near the boundary claimed ahead of every other
 * handler (capture, so a row's own press never starts instead), the drag and the boundary's
 * cursor on the screen, the keys on the separator itself, and its double-click reset. */
export function mountColumnSplitterControls(deps: ColumnSplitterControlsDependencies): ColumnSplitterControls {
  const { listen, unmount } = createListening();
  const screen = deps.lookup("screen-main");
  listen<PointerEvent>(screen, "pointerdown", (event) => {
    if (deps.down(event)) {
      event.stopPropagation();
      event.preventDefault();
    }
  }, { capture: true });
  listen<PointerEvent>(screen, "pointermove", (event) => deps.move(event));
  listen<PointerEvent>(screen, "pointerup", () => deps.up());
  listen<PointerEvent>(screen, "pointercancel", () => deps.up());
  listen<MouseEvent>(screen, "dblclick", (event) => {
    if (deps.doubleClick(event)) {
      event.stopPropagation();
      event.preventDefault();
    }
  }, { capture: true });
  listen<KeyboardEvent>(deps.lookup("column-splitter"), "keydown", (event) => deps.key(event));
  return { unmount };
}

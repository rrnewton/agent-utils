// The layout the reader chose, and where it was chosen from (`#229 desktop-two-column`, `#281 view-keys`;
// `#624 voice-page-modularity`, moved from the legacy script into core/navigation): one column, the thread
// list on its own, or two columns; the one-column layout two columns' button goes back to; and what the
// thread list was chosen from, which Esc on it goes back to. Each is this window's own and kept for its next
// reload, and the layout the device's as well, for a brand-new window. Whether the window is wide enough,
// and drawing the layout, are the page's.

import type { StoragePort } from "../../ports/index.ts";
import { REOPENED_VIEWS } from "./uistate.ts";

/**
 * The layout chosen: "two", "list" for the thread list on its own, or anything else for one column.
 * `#281 view-keys`: this window's own, which a reload of it comes back to, as `#265
 * window-channel-memory` keeps its channel and view; and the device's, which a brand-new window opens
 * in — the layout chosen last, in any window.
 */
const COLUMNS_KEY = "vibe-talk.voice.columns";

/**
 * Where this window's layouts were chosen from: the layout two columns' button goes back to, and the view
 * Esc on the thread list goes back to. `#281 view-keys`. The window's own, as its layout is
 * (`COLUMNS_KEY`), so a reload of either still has its way back: two columns in a window too narrow for
 * them still show the thread list they came from, and the list still goes back to Main or All.
 */
const LAYOUT_BEFORE_KEY = "vibe-talk.voice.columns-before";

/** A layout: one column, the thread list on its own, or two columns. */
export type Layout = "one" | "list" | "two";

/**
 * What the thread list was chosen from, which Esc on it goes back to: two columns, with the layout their
 * button went back to then, or one column and the view it showed — a channel's Main, All, or a thread
 * with where that was opened from.
 */
export interface ListBefore {
  readonly layout: "one" | "two";
  readonly columns?: string;
  readonly channel?: string;
  readonly view?: string;
  readonly thread?: string | null;
  readonly origin?: string | null;
}

/** What the layout asks of the page. */
export interface LayoutChoiceDependencies {
  /** The device's storage: any call may throw. */
  readonly device: StoragePort;
  /** This window's storage: any call may throw. */
  readonly window: StoragePort;
}

/** The layout's commands and questions. */
export interface LayoutChoice {
  /** The layout the reader chose. Applied where it is offered, and two columns only where the window is wide enough. */
  chosen(): Layout;
  /** The one-column layout two columns were chosen from, "one" or "list": what the layout button goes back to. */
  beforeColumns(): Layout;
  /** What the thread list was chosen from, for Esc on it. */
  listBefore(): ListBefore;
  /**
   * Choose `layout` and keep it, with where it was chosen from: a one-column layout is also what two
   * columns' button goes back to from now on.
   */
  choose(layout: Layout): void;
  /**
   * The thread list is being chosen: from two columns, with the layout their button goes back to; or
   * from one column, `oneColumn()` saying the view it shows, or null where that is a thread opened from
   * the list, which is not noted.
   */
  noteListBefore(oneColumn: () => ListBefore | null): void;
  /** Two columns are being gone back to from the thread list: their button goes back to `columns` again. */
  restoreBeforeColumns(columns: unknown): void;
}

/** Whether `before` is something the thread list can go back to (see `ListBefore`). */
function validListBefore(before: unknown): before is ListBefore {
  const held = before as ListBefore | null;
  return held !== null && typeof held === "object" &&
    (held.layout === "two" || (held.layout === "one" && typeof held.channel === "string" &&
      REOPENED_VIEWS.includes(held.view as string) && (held.view !== "thread" ||
        (typeof held.thread === "string" && held.thread !== "" && ["main", "flat"].includes(held.origin as string)))));
}

/** As this window and the device last kept it. */
export function createLayoutChoice(deps: LayoutChoiceDependencies): LayoutChoice {
  /** This window's own `key`, or null where it holds none or storage is refused. */
  function windowItem(key: string): string | null {
    try {
      return deps.window.getItem(key);
    } catch (_error) {
      return null;
    }
  }

  /** Keep `value` as this window's own `key`. Nothing, where storage is refused. */
  function keepWindowItem(key: string, value: string): void {
    try {
      deps.window.setItem(key, value);
    } catch (_error) {
      // The device's copy is still written, and a reload of this window falls back on it.
    }
  }

  /** The layout this window opens in: its own choice, else the device's, else one column. */
  function storedLayoutChoice(): Layout {
    let device = null;
    try {
      device = deps.device.getItem(COLUMNS_KEY);
    } catch (_error) {
      device = null;
    }
    const own = windowItem(COLUMNS_KEY);
    const stored = own !== null ? own : device;
    return stored === "two" || stored === "list" ? stored : "one";
  }

  /** What `LAYOUT_BEFORE_KEY` holds for this window, or an empty record where it holds nothing usable. */
  function storedLayoutBefore(): { columns?: unknown; list?: unknown } {
    try {
      const held = JSON.parse(windowItem(LAYOUT_BEFORE_KEY) || "null");
      return held !== null && typeof held === "object" ? held : {};
    } catch (_error) {
      return {};
    }
  }

  let layoutChosen: Layout = storedLayoutChoice();
  const layoutBefore = storedLayoutBefore();
  let layoutBeforeColumns: Layout = ["one", "list"].includes(layoutBefore.columns as string) ? layoutBefore.columns as Layout
    : layoutChosen === "list" ? "list" : "one";
  let listBefore: ListBefore = validListBefore(layoutBefore.list) ? layoutBefore.list : { layout: "one" };

  /** Keep `layout` as the one chosen: this window's, and the device's for the next new window. */
  function keepLayoutChoice(layout: Layout): void {
    try {
      deps.device.setItem(COLUMNS_KEY, layout);
    } catch (_error) {
      // A browser that refuses storage still honours the choice for this session.
    }
    keepWindowItem(COLUMNS_KEY, layout);
  }

  /** Keep where this window's layouts were chosen from, for its next reload (`LAYOUT_BEFORE_KEY`). */
  function keepLayoutBefore(): void {
    keepWindowItem(LAYOUT_BEFORE_KEY, JSON.stringify({ columns: layoutBeforeColumns, list: listBefore }));
  }

  return {
    chosen: () => layoutChosen,
    beforeColumns: () => layoutBeforeColumns,
    listBefore: () => listBefore,
    choose(layout) {
      if (layout !== "two") layoutBeforeColumns = layout;
      layoutChosen = layout;
      keepLayoutChoice(layout);
      keepLayoutBefore();
    },
    noteListBefore(oneColumn) {
      if (layoutChosen === "two") {
        listBefore = { layout: "two", columns: layoutBeforeColumns };
        return;
      }
      const one = oneColumn();
      if (one !== null) listBefore = one;
    },
    restoreBeforeColumns(columns) {
      if (columns === "one" || columns === "list") layoutBeforeColumns = columns;
    },
  };
}

// What the page and the tests use of it.
export { COLUMNS_KEY, LAYOUT_BEFORE_KEY };

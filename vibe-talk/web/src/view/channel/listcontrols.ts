// The list's tools (`#65 scrollback-paging`, `#262 search-load-older`, `#605`-`#608 server-side-search`, `#48
// transcript-storage`, `#301 semicolon-expand-cycle`, `#221 read-modes`, `#564 message-summary-toggle`, `#820
// voice-page-action-menu`, `#880 voice-page-menu-placement`, `#886 voice-page-autoplay`; `#624 voice-page-modularity`, moved from the legacy
// script's wiring into view/channel for the plan's step 17): the ways further back, the search's ways wider, the
// action menu's items, and the chips over the list. What each does is the page's; mounted once, in the page's
// order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface ListControlsDependencies {
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /** Older messages, from the control at the top of the list. */
  loadOlder(): unknown;
  /** `#262 search-load-older`. One column on Main, a search nothing loaded matches: All has the replies. */
  searchInAll(): unknown;
  /** `#605`-`#608 server-side-search`. The server's next page of hits, and searching every channel or this one. */
  serverMore(): unknown;
  searchAllChannels(on: boolean): void;
  /** The search's walk further back. */
  searchFurther(): unknown;
  /** The call's older record. */
  loadOlderTurns(): unknown;
  /** Fold every long message, or open every one. */
  setAllFolded(folded: boolean): unknown;
  /** Show read, Fold read, Hide read, in turn. */
  cycleReadMode(): unknown;
  /** Mark the backlog read, and undo a dismissal. */
  clearBacklog(): unknown;
  undoDismissal(): unknown;
  /** `#564 message-summary-toggle`. Summaries on or off, as Settings' switch. */
  toggleSummaries(): unknown;
  /** `#880 voice-page-menu-placement`. Whether the reader's row is hidden, hiding or showing it, and the pace
   * popover, the row's, taken down. */
  readerHidden(): boolean;
  setReaderHidden(hidden: boolean): void;
  closeSpeedPopover(): void;
  /** `#886 voice-page-autoplay`. Whether autoplay is on, and turning it on or off. */
  autoplayOn(): boolean;
  setAutoplay(on: boolean): void;
  /** `#820 voice-page-action-menu`. Whether the focus is in the action menu, and closing it. */
  focusInActionMenu(): boolean;
  closeActionMenu(): void;
  /** To the newest message. */
  jumpToNewest(): unknown;
}

/** The listeners, mounted. */
export interface ListControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the list's tools' listeners. */
export function mountListControls(deps: ListControlsDependencies): ListControls {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  const leftEl = (id: string): HTMLElement => deps.leftById(id);
  listen(el("load-older"), "click", () => deps.loadOlder());
  // `#262 search-load-older`. One column on Main, a search nothing loaded matches: All has the replies.
  listen(leftEl("search-in-all"), "click", () => deps.searchInAll());
  // `#605`-`#608 server-side-search`.
  listen(el("server-more"), "click", () => deps.serverMore());
  listen(el("search-all-channels"), "change", () => deps.searchAllChannels((el("search-all-channels") as HTMLInputElement).checked));
  listen(el("search-further"), "click", () => deps.searchFurther());
  listen(el("load-older-turns"), "click", () => deps.loadOlderTurns());
  listen(el("collapse-all"), "click", () => deps.setAllFolded(true));
  listen(el("expand-all"), "click", () => deps.setAllFolded(false));
  listen(el("todo-filter"), "click", () => deps.cycleReadMode());
  listen(el("clear-backlog"), "click", () => deps.clearBacklog());
  listen(el("undo-dismiss"), "click", () => deps.undoDismissal());
  // `#564 message-summary-toggle`. The chip is Settings' switch, kept and synced like it.
  listen(el("summarise"), "click", () => deps.toggleSummaries());
  // `#880 voice-page-menu-placement`. Hide reader, Show reader. The pace popover is the row's, and goes with it rather
  // than staying open, undrawn, as a level Escape and the back gesture would close first.
  listen(el("reader-toggle"), "click", () => {
    deps.setReaderHidden(!deps.readerHidden());
    if (deps.readerHidden()) deps.closeSpeedPopover();
  });
  // `#886 voice-page-autoplay`. Autoplay on or off, as the read strip's toggle.
  listen(el("autoplay-toggle"), "click", () => deps.setAutoplay(!deps.autoplayOn()));
  // `#820 voice-page-action-menu`. Those are the action menu's items: choosing one closes it, after the
  // item's own act (registered just above), and gives the keys back to its opener rather than to nothing.
  for (const id of ["expand-all", "collapse-all", "summarise", "reader-toggle", "autoplay-toggle", "discuss-menu-open"]) {
    listen(el(id), "click", () => {
      const inside = deps.focusInActionMenu();
      deps.closeActionMenu();
      if (inside) el("action-menu-open").focus({ preventScroll: true });
    });
  }
  listen(el("jump-newest"), "click", () => deps.jumpToNewest());
  return { unmount };
}

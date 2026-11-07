// The right column list's own controls (`#262 search-load-older`, `#338 column-overscroll`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/channel for the plan's step 17): the step back
// to older messages, clearing the backlog, the way to the newest line, and the list scrolled, each act done in that
// column. The left column's are `mountListControls`'s. What each does is the page's; mounted once, in the page's order,
// and taken down again.

import { createListening } from "../dom/index.ts";

export interface SideListControlsDependencies {
  /** An element of the column the controls are mounted for, by id. */
  lookup(id: string): HTMLElement;
  /** Older messages read, the backlog cleared, and the newest line reached. */
  loadOlder(): unknown;
  clearBacklog(): unknown;
  jumpNewest(): unknown;
  /** The list scrolled: the jump, the way back to a reply, the step back at the top, and the summaries in view. */
  listScrolled(): unknown;
}

/** The listeners, mounted. */
export interface SideListControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the right column list's controls in the page's order. */
export function mountSideListControls(deps: SideListControlsDependencies): SideListControls {
  const { listen, unmount } = createListening();
  listen<Event>(deps.lookup("load-older"), "click", () => deps.loadOlder());
  listen<Event>(deps.lookup("clear-backlog"), "click", () => deps.clearBacklog());
  // `#338 column-overscroll`. The reader's own way to the newest line lands on the foot, as End does.
  listen<Event>(deps.lookup("jump-newest"), "click", () => deps.jumpNewest());
  listen<Event>(deps.lookup("scroll-area"), "scroll", () => deps.listScrolled());
  return { unmount };
}

// The ways back to a place in the list (`#240 reading-bookmark`, the jump to where the reader was reading, and back
// to a reply; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/channel for the plan's
// step 17). What each does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface ReadingJumpsDependencies {
  /** An element of the page by id, and one of the left column's (where the reading chip is, as b is). */
  lookup(id: string): HTMLElement;
  leftLookup(id: string): HTMLElement;
  /** To the bookmark, to where the reader was reading, and back to the reply left. */
  jumpToBookmark(): unknown;
  jumpToUnread(): unknown;
  jumpToReading(): unknown;
  backToReply(): unknown;
}

/** The listeners, mounted. */
export interface ReadingJumps {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the ways back in the page's order. */
export function mountReadingJumps(deps: ReadingJumpsDependencies): ReadingJumps {
  const { listen, unmount } = createListening();
  listen<Event>(deps.lookup("jump-bookmark"), "click", () => deps.jumpToBookmark());
  listen<Event>(deps.lookup("jump-unread"), "click", () => deps.jumpToUnread());
  listen<Event>(deps.leftLookup("jump-reading"), "click", () => deps.jumpToReading());
  listen<Event>(deps.lookup("back-to-reply"), "click", () => deps.backToReply());
  return { unmount };
}

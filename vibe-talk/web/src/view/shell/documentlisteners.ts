// The page's last document-level listeners (`#54 resume-recovery`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/shell for the plan's step 17):
// the page going hidden or coming back, and a pointer down outside a row's menu. What each does is
// the page's; mounted once, in the page's order, and taken down again. The mouse leaving the window
// used to be heard here to drop a glide's wait; hover scrolls nothing now (`#1144`).

import { createListening } from "../dom/index.ts";

export interface DocumentListenersDependencies {
  /** The page went away or came back (`#54`). */
  onVisibility(event: Event): void;
  /** A pointer down: a row's menu open elsewhere goes. */
  closeRowMenuOutside(event: Event): void;
}

/** The listeners, mounted. */
export interface DocumentListeners {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the document-level listeners, in the page's order. */
export function mountDocumentListeners(deps: DocumentListenersDependencies): DocumentListeners {
  const { listen, unmount } = createListening();
  // `#54 resume-recovery`. The one signal a browser gives for "this page went away".
  listen<Event>(document, "visibilitychange", (event) => deps.onVisibility(event));
  listen<Event>(document, "pointerdown", (event) => deps.closeRowMenuOutside(event));
  return { unmount };
}

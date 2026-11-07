// The page's last document-level listeners (`#54 resume-recovery`, `#280 scroll-stability`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/shell for the plan's step 17):
// the page going hidden or coming back, a pointer down outside a row's menu, and the mouse leaving the
// window. What each does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface DocumentListenersDependencies {
  /** The document's root element, which hears the mouse leave the window. */
  root(): HTMLElement;
  /** The page went away or came back (`#54`). */
  onVisibility(event: Event): void;
  /** A pointer down: a row's menu open elsewhere goes. */
  closeRowMenuOutside(event: Event): void;
  /** The mouse left the window: the wait before a glide between the columns drops (`#280`). */
  onPointerLeavePage(event: Event): void;
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
  // `#280 scroll-stability`. The mouse leaving the window drops the wait before a glide.
  listen<Event>(deps.root(), "pointerleave", (event) => deps.onPointerLeavePage(event));
  return { unmount };
}

// The thread's back gesture and the right column's controls (`#304 back-gesture-in-app`, `#229 desktop-two-column`,
// `#388 thread-header-collapse`, `#281 view-keys`, `#262 search-load-older`, `#404 threads-unpack-all`, `#419
// threads-view-main-messages`, `#426 threads-sort-toggle`; `#624 voice-page-modularity`, moved from the legacy
// script's wiring into view/channel for the plan's step 17): a swipe right out of a thread, the layout button, the
// X, the card at the top of a thread, the card's flight ending, and the Threads view's own chips. What each does is
// the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";
import { SWIPE_COMMIT_PX } from "./gestures.ts";

/** Where a back gesture began. */
export interface GestureStart {
  x: number;
  y: number;
}

export interface SideControlsDependencies {
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /** Whether a thread is the view in the column. */
  inThread(): boolean;
  /** The column's back gesture under way, and setting or clearing it (`entryState().backGesture`). */
  backGesture(): GestureStart | null;
  setBackGesture(start: GestureStart | null): void;
  /** Take the click a swipe ends in, so it presses nothing (`rowGestures.claimClick`). */
  claimClick(): void;
  /** Leave the thread. */
  closeThread(): unknown;
  /** One column or two, and the right column's X. */
  toggleColumns(): unknown;
  closeSide(): unknown;
  /** `#388 thread-header-collapse`. The cards at the top of a thread, one per column, and a press on one. */
  threadCardIds: readonly string[];
  threadCardPressed(card: HTMLElement, event: MouseEvent): unknown;
  /** The card moving up as its thread opens: one of its parts ended (`endCardFlightPart`). */
  endCardFlightPart(node: HTMLElement, event: AnimationEvent): unknown;
  /** `#281 view-keys`. The next page of older threads. */
  loadOlderThreads(): unknown;
  /** `#262 search-load-older`. The cards' own way further back for the search, and Stop while it walks. */
  searchReplies(): unknown;
  /** `#404`, `#419`, `#426`: Unpack, Main channel and the order, each the next in turn. */
  nextUnpack(): unknown;
  toggleMainRows(): unknown;
  nextOrder(): unknown;
}

/** The listeners, mounted. */
export interface SideControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the thread's back gesture and the right column's controls. */
export function mountSideControls(deps: SideControlsDependencies): SideControls {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  const leftEl = (id: string): HTMLElement => deps.leftById(id);
  // A thread's back gesture under way: a column's `entryState().backGesture` (`#624 voice-page-modularity`).
  listen<PointerEvent>(el("scroll-area"), "pointerdown", (event) => {
    if (!deps.inThread() || event.pointerType === "mouse") return;
    if (["TEXTAREA", "INPUT", "BUTTON"].includes(String(event.target && (event.target as Element).tagName).toUpperCase())) return;
    deps.setBackGesture({ x: event.clientX, y: event.clientY });
  });
  listen<PointerEvent>(el("scroll-area"), "pointerup", (event) => {
    const start = deps.backGesture();
    deps.setBackGesture(null);
    if (!start || !deps.inThread()) return;
    const dx = event.clientX - start.x;
    const dy = event.clientY - start.y;
    if (dx >= SWIPE_COMMIT_PX && dx > Math.abs(dy)) {
      deps.claimClick();
      deps.closeThread();
    }
  });
  listen(el("scroll-area"), "pointercancel", () => deps.setBackGesture(null));
  // `#229 desktop-two-column`. The layout button, and the right column's own controls: each acts in
  // the right column, as its counterpart in #pane-discord acts in the left.
  listen(el("column-toggle"), "click", () => deps.toggleColumns());
  // The X's one act, which the back gesture's is too (`#304 back-gesture-in-app`).
  listen(el("side-close"), "click", () => deps.closeSide());
  // `#388 thread-header-collapse`. A press on the card at the top of a thread, in either column, folds it
  // to a bar or opens it again.
  for (const id of deps.threadCardIds) {
    listen<MouseEvent>(leftEl(id), "click", (event) => deps.threadCardPressed(leftEl(id), event));
  }
  // The card moving up as its thread opens ends with its animation, or the thread's fading in with its
  // own: each ends its own part (`endCardFlightPart`).
  for (const id of ["thread-card", "side-thread-card", "pane-discord", "side-pane"]) {
    listen<AnimationEvent>(leftEl(id), "animationend", (event) => deps.endCardFlightPart(leftEl(id), event));
  }
  // `#281 view-keys`. The next page of older threads, read where the cards are.
  listen(el("side-more-threads"), "click", () => deps.loadOlderThreads());
  // `#262 search-load-older`. The cards' own way further back for the search, and Stop while it walks.
  listen(leftEl("side-search-replies"), "click", () => deps.searchReplies());
  // `#404 threads-unpack-all`. Every thread's messages under its card, in the Threads view.
  listen(leftEl("side-unpack"), "click", () => deps.nextUnpack());
  listen(leftEl("side-main"), "click", () => deps.toggleMainRows());
  // `#426 threads-sort-toggle`. The cards' order, in the Threads view.
  listen(leftEl("side-order"), "click", () => deps.nextOrder());
  return { unmount };
}

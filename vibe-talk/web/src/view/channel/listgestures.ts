// The lists' gestures and scrolls (`#68 pull-to-refresh`, `#188 pull-refresh-bottom`, `#261 two-column-connectors`,
// `#338 column-overscroll`, `#460 pack-gap-slide`; `#624 voice-page-modularity`, moved from the legacy script's
// wiring into view/channel for the plan's step 17): the pull on Main's list, each column's scroll moving the links'
// ends, the wheel at the right column's foot, the reader's own wheel or key ending a slide, and the room past the
// foot following the scroll. Every listener on a list only reads, so every one of those is passive. What each
// does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

/** A touch on the list, as far as the pull reads it. */
export type PulledTouch = TouchEvent;

export interface ListGesturesDependencies {
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /**
   * The pull to refresh: a touch starting, moving, ending (a failure said, ending nothing else; what it answers, the
   * refresh's promise, is the touchend listener's answer, which the page suite waits for), and taken away.
   */
  pullStart(event: PulledTouch): void;
  pullMove(event: PulledTouch): void;
  pullEnd(): unknown;
  pullCancel(): void;
  /** Draw the links between the columns again, once per frame (`#261`). */
  scheduleColumnLinks(): void;
  /** A turn of the wheel on the right column, and its scroll at `timeStamp` (`#338`, `footRooms`). */
  footWheel(event: WheelEvent): void;
  footScrolled(timeStamp: number): void;
  /** End a slide of the pack gap at once (`#460`). */
  finishPackSlide(): void;
}

/** The listeners, mounted. */
export interface ListGestures {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the lists' gesture and scroll listeners. */
export function mountListGestures(deps: ListGesturesDependencies): ListGestures {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  const leftEl = (id: string): HTMLElement => deps.leftById(id);
  // `#68 pull-to-refresh`. On #scroll-area rather than on the document, because the gesture is about
  // THIS list and because the page's other three scroll gestures already live here. Nothing calls
  // `preventDefault`: the pull only ever begins where the element has nothing left to scroll, so
  // there is no browser behaviour to suppress — `overscroll-behavior: contain` has already stopped
  // the drag becoming the browser's own page-level refresh.
  //
  // So all four are PASSIVE, and say so. A listener that might cancel makes the browser hold every
  // scroll that starts on this list until the script has answered, and this script is often busy —
  // drawing a page of rows, decoding a stream. The gesture only ever reads the touches; declaring
  // that lets the list scroll first and tell the page afterwards. `#188 pull-refresh-bottom`.
  const PASSIVE = { passive: true };
  listen<TouchEvent>(el("scroll-area"), "touchstart", (event) => deps.pullStart(event), PASSIVE);
  listen<TouchEvent>(el("scroll-area"), "touchmove", (event) => deps.pullMove(event), PASSIVE);
  listen(el("scroll-area"), "touchend", () => deps.pullEnd(), PASSIVE);
  // A cancel is the browser taking the gesture away — a phone call arriving, a system gesture
  // winning. It must not leave the affordance standing on the screen saying "release to refresh".
  listen(el("scroll-area"), "touchcancel", () => deps.pullCancel(), PASSIVE);
  // `#261 two-column-connectors`. Each column's scroll moves the links' ends: one frame per burst, asked
  // for by listeners that only read (`scheduleColumnLinks`), and nothing at all in one column. Coming back
  // to the page draws them again, as a hidden page draws none.
  listen(leftEl("scroll-area"), "scroll", () => deps.scheduleColumnLinks(), PASSIVE);
  listen(leftEl("side-scroll"), "scroll", () => deps.scheduleColumnLinks(), PASSIVE);
  // `#338 column-overscroll`. A turn of the wheel down at the right column's foot opens the room past it, once
  // both the wheel and the list have rested there (`OVERSCROLL_REST_MS`): the scroll that brought the reader to
  // the foot, a fling's included, stops on it. Passive, so the browser scrolls for the wheel without waiting for
  // this page: it has already found the list at its end and moved nothing by the time the room opens, so the
  // turn that opened it is scrolled here, by as much as the browser would have; the turns after it the browser
  // scrolls into the room itself.
  listen<WheelEvent>(leftEl("side-scroll"), "wheel", (event) => deps.footWheel(event), PASSIVE);
  // `#460 pack-gap-slide`. The reader's own wheel or key on the list ends a slide at once: what they act on is
  // where it is laid out. Not a scroll, which the page's own place-keeping makes too; nor a press or a touch,
  // whose click lands on what was under the pointer as it went down, which ending the slide would move.
  for (const type of ["wheel", "keydown"]) {
    listen(leftEl("side-scroll"), type, () => deps.finishPackSlide(), PASSIVE);
  }
  // ...and goes back below the screen as the reader goes back up past the foot, or once they have rested at
  // the foot without going into it (`waitOverscrollIdle`). A reader in it is noted where they have the last
  // card, for `holdOverscrollReader`.
  listen<Event>(leftEl("side-scroll"), "scroll", (event) => {
    // A browser's scroll event always carries its time; the page suite fires its scroll listeners with none.
    deps.footScrolled(event ? event.timeStamp : -Infinity);
  }, PASSIVE);
  listen(document, "visibilitychange", () => deps.scheduleColumnLinks());
  return { unmount };
}

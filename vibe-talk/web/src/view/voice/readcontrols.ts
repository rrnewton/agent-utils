// The read-aloud controls (`#567 pace-stepper`, `#376 read-aloud-blocks`, `#627 scrubber-thumb-arrows`, `#442
// read-strip-order`, `#434 read-aloud-pause`, `#451 read-strip-followups`, `#886 voice-page-autoplay`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/voice for the plan's step 17): the pace's
// steps and box, the strip's arrows, its grip on a desk, its Autoplay toggle, the window resizing under it, Read and
// Stop under the list. What each does is the read-aloud owner's and the page's; mounted once, in the page's order,
// and taken down again.

import { createListening } from "../dom/index.ts";

export interface ReadControlsDependencies {
  /** An element of the page by id. */
  byId(id: string): HTMLElement;
  /** The window, where it can be listened to, for its resizes; null where it cannot. */
  win(): EventTarget | null;
  /** The pace: a step, and the box's value taken up. */
  stepReadSpeed(direction: number): unknown;
  commitReadSpeedBox(): unknown;
  /** A step through the read's blocks, or its sections. */
  stepReading(delta: number): unknown;
  stepSection(delta: number): unknown;
  /** The strip's grip on a desk: pressed, moved, let go, its keys, and the strip put back. */
  onReadGripDown(event: PointerEvent): unknown;
  onReadGripMove(event: PointerEvent): unknown;
  onReadGripUp(): unknown;
  onReadGripKey(event: KeyboardEvent): unknown;
  moveReadSteer(offset: null): unknown;
  /** Hold the strip inside the window. */
  placeReadSteer(): unknown;
  /** `#886 voice-page-autoplay`. Whether autoplay is on, turning it on or off, and forgetting a Hang up's halt. */
  autoplayOn(): boolean;
  setAutoplay(on: boolean): unknown;
  forgetAutoplayHalt(): unknown;
  /** Whether a read plays or is held, its Pause and Resume, and the message still being fetched, if any. */
  readInProgress(): boolean;
  toggleReadingPaused(): unknown;
  pendingRead(): string | null;
  /** Whether reading mode is on, turning it on or off, and leaving it with the read (Stop). */
  readingMode(): boolean;
  setReadingMode(on: boolean): unknown;
  leaveReadingMode(): unknown;
}

/** The listeners, mounted. */
export interface ReadControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the read-aloud controls' listeners. */
export function mountReadControls(deps: ReadControlsDependencies): ReadControls {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  listen(el("read-speed-down"), "click", () => deps.stepReadSpeed(-1));
  listen(el("read-speed-up"), "click", () => deps.stepReadSpeed(1));
  listen(el("read-speed-box"), "change", () => deps.commitReadSpeedBox());
  listen(el("read-speed-box"), "blur", () => deps.commitReadSpeedBox());
  listen<KeyboardEvent>(el("read-speed-box"), "keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      deps.commitReadSpeedBox();
    } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      event.preventDefault();
      deps.stepReadSpeed(event.key === "ArrowUp" ? 1 : -1);
    }
  });
  // `#376 read-aloud-blocks`. Through the blocks of the read in progress.
  listen(el("read-prev"), "click", () => { deps.stepReading(-1); });
  listen(el("read-next"), "click", () => { deps.stepReading(1); });
  listen(el("read-section-prev"), "click", () => { deps.stepSection(-1); });
  listen(el("read-section-next"), "click", () => { deps.stepSection(1); });
  // `#442 read-strip-order`. The strip moved on a desk, by its grip: pointer, keys, double-click home.
  listen<PointerEvent>(el("read-steer-grip"), "pointerdown", (event) => deps.onReadGripDown(event));
  listen<PointerEvent>(el("read-steer-grip"), "pointermove", (event) => deps.onReadGripMove(event));
  listen(el("read-steer-grip"), "pointerup", () => deps.onReadGripUp());
  listen(el("read-steer-grip"), "pointercancel", () => deps.onReadGripUp());
  listen(el("read-steer-grip"), "lostpointercapture", () => deps.onReadGripUp());
  listen<KeyboardEvent>(el("read-steer-grip"), "keydown", (event) => deps.onReadGripKey(event));
  listen(el("read-steer-grip"), "dblclick", () => { deps.moveReadSteer(null); });
  // `#886 voice-page-autoplay`. The strip's Autoplay toggle.
  listen(el("read-autoplay"), "click", () => deps.setAutoplay(!deps.autoplayOn()));
  const win = deps.win();
  if (win !== null) listen(win, "resize", () => deps.placeReadSteer());
  listen(el("read-aloud"), "click", () => {
    // `#434 read-aloud-pause`: while a read is playing or held, this button is its Pause/Resume —
    // the media keys by another reach. Leaving the mode is what this button does with nothing
    // playing, and what Stop beside it does while a read plays
    // (`#442 read-strip-order`). `#451 read-strip-followups`: a read started with s outside the
    // mode pauses here too, where it used to enter the mode over a read still playing.
    if (deps.readInProgress()) {
      deps.toggleReadingPaused();
      return;
    }
    // A read still being fetched: this button says Stop there, and stops it — leaving the mode with
    // it where the mode is on, as its Stop always has (`leaveReadingMode`).
    if (deps.pendingRead() !== null) {
      deps.leaveReadingMode();
      return;
    }
    deps.setReadingMode(!deps.readingMode());
  });
  listen(el("read-aloud-stop"), "click", () => deps.leaveReadingMode());
  // `#886 voice-page-autoplay`. Stop forgets a Hang up's halt, as nothing it stopped will end and go on.
  listen(el("read-aloud-stop"), "click", () => deps.forgetAutoplayHalt());
  return { unmount };
}

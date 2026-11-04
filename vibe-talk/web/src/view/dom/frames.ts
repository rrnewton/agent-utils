// Work asked for many times between two frames, done once in the next (`#624 voice-page-modularity`, moved
// from the legacy script into view/dom): the window's resize, which fires for every step of a drag, asks the
// layout again once per frame rather than once per event. What the work is, and what asks for it, are the
// page's.

/** The browser's next frame (`requestAnimationFrame`), or null where there is none: the work runs at once. */
export type FrameScheduler = ((run: () => void) => unknown) | null;

/** Work done at most once per frame. */
export interface OncePerFrame {
  /** Ask for the work: in the next frame, unless it is asked for already. */
  ask(): void;
  /** Whether it is asked for and not done yet. */
  due(): boolean;
}

/** Nothing asked for. `frame` is asked for the browser's scheduler each time, so a page that gains one uses it. */
export function createOncePerFrame(run: () => void, frame: () => FrameScheduler): OncePerFrame {
  let asked = false;
  const done = () => {
    asked = false;
    run();
  };
  return {
    ask() {
      if (asked) return;
      asked = true;
      const next = frame();
      if (next) next(done);
      else done();
    },
    due: () => asked,
  };
}

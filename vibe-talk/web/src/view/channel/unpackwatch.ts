// Watching the unpacked blocks come near the screen (`#404 threads-unpack-all`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): a block under its card reads its thread as it nears the
// screen, so the reads go to what the reader is about to see, one observer over the cards' scroller made when
// first wanted. Where nothing can watch, the read is asked for at once. The reads, and the rows, are the page's.

/** How far below the cards on screen a block starts its read, in px: about one screenful ahead. */
const UNPACK_NEAR_PX = 400;

/** What watching the blocks needs from the page. */
export interface UnpackWatchDependencies {
  /** The cards' scroller, which the blocks near. */
  root(): Element;
  /** Read thread `id` for its block. */
  read(id: string): void;
}

/** The watch's commands. */
export interface UnpackWatch {
  /** Read thread `id` for its block as `row` nears the screen; at once where nothing can watch. */
  watch(row: Element, id: string): void;
  /** `row` is leaving the list: no longer watched. */
  unwatch(row: Element): void;
  /** Watch nothing: the blocks are drawn anew, or the channel is another. */
  stop(): void;
}

/** Watching nothing yet. */
export function createUnpackWatch(deps: UnpackWatchDependencies): UnpackWatch {
  /** The observer that reads a thread as its block nears the screen, made when first wanted. */
  let observer: IntersectionObserver | null = null;

  return {
    watch(row, id) {
      if (typeof IntersectionObserver !== "function") {
        deps.read(id);
        return;
      }
      if (observer === null) {
        observer = new IntersectionObserver((entries) => {
          for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            // Whichever observer is held when the entry comes.
            (observer as IntersectionObserver).unobserve(entry.target);
            deps.read(String(entry.target.getAttribute("data-unpack-thread")));
          }
        }, { root: deps.root(), rootMargin: `${UNPACK_NEAR_PX}px` });
      }
      observer.observe(row);
    },
    unwatch(row) {
      if (observer !== null) observer.unobserve(row);
    },
    stop() {
      if (observer !== null) {
        observer.disconnect();
        observer = null;
      }
    },
  };
}

// What the page and the tests use of it.
export { UNPACK_NEAR_PX };

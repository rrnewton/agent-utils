// Which rows are being built (`#419 threads-view-main-messages`, the Pinned filter; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): the Pinned filter's rows, and Main's messages among the
// thread cards. Each offers the way into its thread, as All's rows do, and points at nothing. A flag rather
// than a parameter, because every other caller hands the row builder (`discordNode`) to `map`, which would
// pass it an index. Building the rows is the page's.

/** The flags' commands and questions. */
export interface RowDrawing {
  /** True while the Pinned filter's rows are built (`syncPinnedList`). */
  pinned(): boolean;
  /** True while Main's rows among the cards are built (`mainRows`). */
  main(): boolean;
  /** Build the Pinned filter's rows with `fn`, and stop however it returns. */
  whilePinned<T>(fn: () => T): T;
  /** Build Main's rows among the cards with `fn`, and stop however it returns. */
  whileMain<T>(fn: () => T): T;
}

/** Building neither. */
export function createRowDrawing(): RowDrawing {
  let drawingPinnedRows = false;
  let drawingMainRows = false;

  return {
    pinned: () => drawingPinnedRows,
    main: () => drawingMainRows,
    whilePinned(fn) {
      drawingPinnedRows = true;
      try {
        return fn();
      } finally {
        drawingPinnedRows = false;
      }
    },
    whileMain(fn) {
      drawingMainRows = true;
      try {
        return fn();
      } finally {
        drawingMainRows = false;
      }
    },
  };
}

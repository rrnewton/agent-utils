// The Undo a removed bookmark offers (`#624 voice-page-modularity`, moved from the legacy script into
// core/annotations): which removal the chip takes back while it is up — the channel, where the line was,
// the removal's own staged change, the stamp that orders it among the page's offers — and the timer that
// takes the chip away after `BOOKMARK_UNDO_MS`. Staging and sending the bookmark, and drawing the chip, are
// the page's.

import type { BookmarkAnchor } from "../../contract/index.ts";
import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long the Undo chip offers to put a removed bookmark back. */
const BOOKMARK_UNDO_MS = 10000;

/** The removal the Undo chip takes back while it is up. */
export interface BookmarkRemoval {
  readonly channel: string;
  readonly anchor: BookmarkAnchor;
  /** The removal's own staged change (`stageBookmark`), which its outcome is matched to. */
  readonly token: object;
  /** Its place among the page's Undo offers (`nextUndoAt`). */
  readonly at: number;
}

/** What the Undo asks of the page. */
export interface BookmarkUndoDependencies {
  readonly clock: ClockPort;
  /** The chip's time is up, or the offer ended: draw the chip again (`renderBookmarkUndo`). */
  ended(): void;
}

/** The Undo's commands and questions. */
export interface BookmarkUndo {
  /** The removal the chip takes back, or null with none up. */
  current(): BookmarkRemoval | null;
  /** Offer Undo for `removal` for `BOOKMARK_UNDO_MS`, replacing any offer up. */
  offer(removal: BookmarkRemoval): void;
  /** End the offer, if one is up, and draw the chip again. */
  end(): void;
}

/** No offer up. */
export function createBookmarkUndo(deps: BookmarkUndoDependencies): BookmarkUndo {
  /**
   * The removal the Undo chip takes back while it is up, and the chip's timer.
   */
  let bookmarkUndo: (BookmarkRemoval & { timer: TimerHandle }) | null = null;

  function endBookmarkUndo(): void {
    if (bookmarkUndo) deps.clock.clearTimeout(bookmarkUndo.timer);
    bookmarkUndo = null;
    deps.ended();
  }

  return {
    current: () => bookmarkUndo,
    offer(removal) {
      if (bookmarkUndo) deps.clock.clearTimeout(bookmarkUndo.timer);
      bookmarkUndo = { ...removal, timer: deps.clock.setTimeout(endBookmarkUndo, BOOKMARK_UNDO_MS) };
    },
    end: endBookmarkUndo,
  };
}

// What the page and the tests use of it.
export { BOOKMARK_UNDO_MS };

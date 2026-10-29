// What the reader has done to messages — the reading bookmark, Done, pins — as this page holds it,
// apart from how it is drawn. Its public interface. `#624 voice-page-modularity`.

export { anchorOfMessage, createBookmarks, isBookmarkState, sameAnchor } from "./bookmarks.ts";
export type { BookmarkMove, BookmarkState, Bookmarks, BookmarksDependencies, HeldBookmark } from "./bookmarks.ts";
export { createReadLedger } from "./reads.ts";
export type { ReadLedger } from "./reads.ts";

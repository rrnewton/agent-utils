// Where the reading bookmark's line stands among a list's rows (`#420 bookmark-clear` and the
// bookmark's own issues; `#624 voice-page-modularity`, moved from the legacy script into present/rows).
// The decision only: given each row's messages and times, and the bookmark's anchor, the row the line
// stands above, or below for a bookmark past the last row shown. Drawing the line is view/channel's.

/** One row of the list, as far as the bookmark reads it. */
export interface BookmarkRow {
  /** The ids of the messages it stands for, oldest first. */
  readonly ids: readonly string[];
  /** When its first message was sent, in ms, or NaN. */
  readonly sentMs: number;
  /** When its newest message was sent, in ms, or NaN. */
  readonly newestMs: number;
  /** Whether it is shown: not filtered out, not hidden as read. */
  readonly shown: boolean;
}

/** The bookmark's anchor: a message, and whether the bookmark stands above or below it. */
export interface BookmarkAnchorFacts {
  readonly message_id: unknown;
  readonly place?: unknown;
  readonly timestamp?: unknown;
}

/**
 * The index of the row the line stands on and on which side, or null for no line. The anchor's message
 * where a row holds it (the row after it for a bookmark below a row's last message); otherwise the
 * first row at or past the anchor's time, unless that time is before everything loaded and the to-do
 * window does not hold it (`holdsTime`). Then the first row shown from there, with the line above it;
 * or, with none shown after, the last row shown, with the line below it.
 */
function bookmarkSpot(rows: readonly BookmarkRow[], anchor: BookmarkAnchorFacts, holdsTime: (at: number) => boolean): { index: number; place: "above" | "below" } | null {
  if (rows.length === 0) return null;
  const id = String(anchor.message_id);
  let start = rows.findIndex((row) => row.ids.includes(id));
  if (start >= 0) {
    const held = (rows[start] as BookmarkRow).ids;
    if (anchor.place === "below" && held[held.length - 1] === id) start += 1;
  } else {
    const at = Date.parse(String(anchor.timestamp));
    if (!Number.isFinite(at)) return null;
    const first = (rows[0] as BookmarkRow).sentMs;
    if (Number.isFinite(first) && at < first && !holdsTime(at)) return null;
    start = rows.findIndex((row) => (anchor.place === "below" ? row.newestMs > at : row.newestMs >= at));
    if (start < 0) start = rows.length;
  }
  for (let index = start; index < rows.length; index += 1) {
    if ((rows[index] as BookmarkRow).shown) return { index, place: "above" };
  }
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    if ((rows[index] as BookmarkRow).shown) return { index, place: "below" };
  }
  return null;
}

// What the page and the tests use of it.
export { bookmarkSpot };

// The ranges the device's numeric settings are held to, and the clamps that hold them (`#624
// voice-page-modularity`, moved from the legacy script). Storage is not a trusted input: it is
// shared with whatever else runs on this origin, it survives a version of the page that had
// different limits, and a hand-edited entry is a thing people do. So a kept value is clamped on the
// way IN as well as on the way out, and anything that is not a number at all falls back to the
// default rather than to NaN, which CSS would ignore silently.

/**
 * The reader's own type size for message text, as a PERCENTAGE of the stylesheet's own.
 *
 * Bounded rather than free: under about eighty per cent the text stops being readable at arm's
 * length on a phone, and over about a hundred and fifty the fold control and the row's own
 * controls no longer line up with the words they belong to.
 */
const MIN_MSG_SCALE = 80;
const MAX_MSG_SCALE = 150;
const DEFAULT_MSG_SCALE = 100;

/** A type size this page will apply: the default for anything that is not a number. */
const clampMsgScale = (value: unknown): number => {
  const n = Math.round(Number(value));
  if (!Number.isFinite(n)) {
    return DEFAULT_MSG_SCALE;
  }
  return Math.min(MAX_MSG_SCALE, Math.max(MIN_MSG_SCALE, n));
};

/**
 * `#425 collapsed-lines`. The lines a fold shows: the reader's own count, per device, three until
 * they say otherwise. 0 is the header alone, 8 a generous opening; past that the fold is barely a
 * fold. The stylesheet reads the same count as `--collapsed-lines` (web/voice.css), and this model
 * counts hidden words against it, so the two cannot disagree about where the cut falls — which is
 * what the fixed three this replaces held by being one number in two files.
 */
const MIN_COLLAPSED_LINES = 0;
const MAX_COLLAPSED_LINES = 8;
const DEFAULT_COLLAPSED_LINES = 3;

/**
 * `#425 collapsed-lines`. A lines count this page will actually apply, for the reason
 * `clampReadingWidth` gives: storage is not a trusted input. A blank entry is ABSENT, not zero —
 * `Number("")` is 0, and 0 is a real count now (the header alone), so taking a blank at its word
 * would put a reader who never touched the control into the tightest list there is.
 */
function clampCollapsedLines(value: unknown): number {
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) {
    return DEFAULT_COLLAPSED_LINES;
  }
  const n = Number(text);
  if (!Number.isFinite(n)) {
    return DEFAULT_COLLAPSED_LINES;
  }
  return Math.min(MAX_COLLAPSED_LINES, Math.max(MIN_COLLAPSED_LINES, Math.round(n)));
}

/**
 * `#522 thread-card-max-lines`. K, the most lines a thread card shows of its root: the server
 * reads it too (src/threads.rs, the same bounds and default).
 */
const MIN_CARD_MAX_LINES = 1;
const MAX_CARD_MAX_LINES = 12;
const DEFAULT_CARD_MAX_LINES = 5;

/** A K this page will apply, read as `clampCollapsedLines` reads its count, and as the server reads it. */
function clampCardMaxLines(value: unknown): number {
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) return DEFAULT_CARD_MAX_LINES;
  const n = Number(text);
  if (!Number.isFinite(n)) return DEFAULT_CARD_MAX_LINES;
  return Math.min(MAX_CARD_MAX_LINES, Math.max(MIN_CARD_MAX_LINES, Math.round(n)));
}

/**
 * `#728 vt-quote-read-threshold`. The least of a message, as a percentage of its words, a reply's
 * quote must cover for the reply to mark the message read. 0 marks it read whatever is quoted, as
 * `#300 reply-marks-read` always did; 100 only for a quote of the whole message.
 */
const MIN_QUOTE_READ_PERCENT = 0;
const MAX_QUOTE_READ_PERCENT = 100;
const DEFAULT_QUOTE_READ_PERCENT = 5;

/** A percentage this page will apply: whole numbers from 0 to 100, the default for anything else. */
function clampQuoteReadPercent(value: unknown): number {
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) return DEFAULT_QUOTE_READ_PERCENT;
  const n = Number(text);
  if (!Number.isFinite(n)) return DEFAULT_QUOTE_READ_PERCENT;
  return Math.min(MAX_QUOTE_READ_PERCENT, Math.max(MIN_QUOTE_READ_PERCENT, Math.round(n)));
}

/** The reading column's width, in characters. */
const MIN_READING_CH = 45;
const MAX_READING_CH = 120;
const DEFAULT_READING_CH = 72;

/**
 * A width this page will actually apply.
 *
 * A stored 4000 must become a column, not a window with no margins, so the value is clamped on the
 * way IN as well as on the way out — and anything that is not a number at all falls back to the
 * default rather than to NaN, which CSS would ignore silently.
 */
function clampReadingWidth(value: unknown): number {
  // A blank entry is ABSENT, not zero. `Number("")` is 0 and `Number(null)` is 0, and taking
  // either at its word would clamp an empty storage slot to the narrowest column the page allows
  // — a reader who had never touched the control would find it already moved.
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) {
    return DEFAULT_READING_CH;
  }
  const ch = Number(text);
  if (!Number.isFinite(ch)) {
    return DEFAULT_READING_CH;
  }
  return Math.min(MAX_READING_CH, Math.max(MIN_READING_CH, Math.round(ch)));
}

/**
 * `#1186 two-column-splitter`. Where the separator between the desk's two columns stands, as a
 * PERCENTAGE of the main screen's width taken by the left column. Fifty until the reader drags
 * it; the ends keep each column at least 300px wide in the narrowest window that shows two
 * columns at all (1000px), and the drag clamps to 300px a side at the window's actual width.
 */
const MIN_TWO_COLUMN_SPLIT = 30;
const MAX_TWO_COLUMN_SPLIT = 70;
const DEFAULT_TWO_COLUMN_SPLIT = 50;

/** A split this page will actually apply, read as `clampReadingWidth` reads its width. */
function clampTwoColumnSplit(value: unknown): number {
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) {
    return DEFAULT_TWO_COLUMN_SPLIT;
  }
  const pct = Number(text);
  if (!Number.isFinite(pct)) {
    return DEFAULT_TWO_COLUMN_SPLIT;
  }
  return Math.min(MAX_TWO_COLUMN_SPLIT, Math.max(MIN_TWO_COLUMN_SPLIT, Math.round(pct)));
}

/**
 * `#1344 discuss-panel-resize`. The Discuss overlay's width on a desk, in CSS pixels. Wider by
 * default than slice 1's 24rem (the owner: "too narrow"); the reader drags the panel's left edge
 * to change it and the choice is kept per device. The ends keep the panel usable: never narrower
 * than a reading column, never wider than a desk can show beside the list; the stylesheet still
 * caps it at the viewport, and a phone never reads it (full-width sheet there).
 */
const MIN_DISCUSS_WIDTH = 320;
const MAX_DISCUSS_WIDTH = 960;
const DEFAULT_DISCUSS_WIDTH = 560;

/** A Discuss width this page will actually apply, read as `clampTwoColumnSplit` reads its split. */
function clampDiscussWidth(value: unknown): number {
  const text = typeof value === "string" ? value.trim() : value;
  if (text === "" || text === null || text === undefined) {
    return DEFAULT_DISCUSS_WIDTH;
  }
  const px = Number(text);
  if (!Number.isFinite(px)) {
    return DEFAULT_DISCUSS_WIDTH;
  }
  return Math.min(MAX_DISCUSS_WIDTH, Math.max(MIN_DISCUSS_WIDTH, Math.round(px)));
}

/**
 * How fast a message is read, as a PERCENTAGE, or null for "however the agent speaks".
 *
 * The bounds match the server's own clamp: below half the words stop being words, above double the
 * audio outruns following it, and both ends are a vendor request nobody wanted to pay for.
 */
const MIN_READ_SPEED = 50;
const MAX_READ_SPEED = 200;

/** A pace this page will send: null, no preference, for anything that is not a number. */
const clampReadSpeed = (value: unknown): number | null => {
  const n = Math.round(Number(value));
  return Number.isFinite(n) ? Math.min(MAX_READ_SPEED, Math.max(MIN_READ_SPEED, n)) : null;
};

export {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_DISCUSS_WIDTH,
  DEFAULT_MSG_SCALE,
  DEFAULT_QUOTE_READ_PERCENT,
  DEFAULT_READING_CH,
  DEFAULT_TWO_COLUMN_SPLIT,
  MAX_CARD_MAX_LINES,
  MAX_COLLAPSED_LINES,
  MAX_DISCUSS_WIDTH,
  MAX_MSG_SCALE,
  MAX_QUOTE_READ_PERCENT,
  MAX_READING_CH,
  MAX_READ_SPEED,
  MAX_TWO_COLUMN_SPLIT,
  MIN_CARD_MAX_LINES,
  MIN_COLLAPSED_LINES,
  MIN_DISCUSS_WIDTH,
  MIN_MSG_SCALE,
  MIN_QUOTE_READ_PERCENT,
  MIN_READING_CH,
  MIN_READ_SPEED,
  MIN_TWO_COLUMN_SPLIT,
  clampCardMaxLines,
  clampCollapsedLines,
  clampDiscussWidth,
  clampMsgScale,
  clampQuoteReadPercent,
  clampReadSpeed,
  clampReadingWidth,
  clampTwoColumnSplit,
};

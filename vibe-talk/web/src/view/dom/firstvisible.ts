// The first thing on screen a Read starts at (`#2003
// vibe-talk-read-starts-on-screen`): pure geometry over viewport-pixel
// edges, shared by the channel list and the Documents reader, so the
// rule is testable without a browser; callers supply the live rects.

/**
 * How much of an item must show in the band for Read to start there: the
 * first item (in reading order) whose visible part is meaningful — at
 * least this many pixels, or at least half of a shorter item whole. A
 * sliver of the item above does not count, and an item taller than the
 * band counts by its visible part alone.
 */
export const MEANINGFUL_VISIBLE_PX = 40;

/**
 * The first item whose visible part in `band` is meaningful, or -1 where
 * none is: items and band as viewport-pixel edges, items in reading order.
 */
export function firstMeaningfulVisible(
  items: readonly { top: number; bottom: number }[],
  band: { top: number; bottom: number },
): number {
  for (let at = 0; at < items.length; at += 1) {
    const item = items[at]!;
    const visible = Math.min(item.bottom, band.bottom) - Math.max(item.top, band.top);
    const height = item.bottom - item.top;
    if (visible > 0 && visible >= Math.min(MEANINGFUL_VISIBLE_PX, height / 2)) return at;
  }
  return -1;
}

// The Documents view state (`#2005 vibe-talk-documents-keep-place`): where the
// reader was in the open document and in the list behind it, kept so toggling
// Documents -> channel -> Documents restores exactly the prior screen. The
// state itself lives in `index.ts` (the open chain, the list anchor, and
// whether the view was ever entered); this module owns only the anchor
// shapes and their arithmetic, pure over measured spans so the boundaries
// are testable without a scroller.
//
// An anchor is never a bare scroll offset: it names the paragraph (drawn
// block) or card at the top of the screen and that item's pixel offset from
// the scroller's top, so a re-layout that grows or shrinks content above it
// cannot shift where the reader lands — the landing is recomputed from the
// item's new place. A raw offset survives in `index.ts` only as the fallback
// for hosts where no geometry can be measured at all.

/** Where the reader was in one open document: a drawn block and its offset. */
export interface ReaderAnchor {
  /** The drawn block at the top of the screen (the reader's block order). */
  readonly block: number;
  /** That block's top less the scroller's top, in px (negative mid-block). */
  readonly offset: number;
}

/** Where the reader was in the list: a card, its offset, and the way back down. */
export interface ListAnchor {
  /** The first card at or below the scroller's top. */
  readonly key: string;
  /** Every drawn card's offset (top less the scroller's top), by key. */
  readonly offsets: Readonly<Record<string, number>>;
  /** Every drawn card's key in draw order: the nearest-survivor fallback. */
  readonly order: readonly string[];
  /** Whether the list was parked at its bottom (newest may have arrived). */
  readonly follow: boolean;
}

/** One measured item's vertical span, in viewport coordinates. */
export interface AnchorSpan {
  readonly top: number;
  readonly bottom: number;
}

/**
 * The reader anchor over `spans` (drawn blocks in document order): the first
 * whose bottom is below the band's top, with its offset from that top. Null
 * where nothing is drawn below it (an empty or unmeasured body).
 */
export function readerAnchorOf(spans: readonly AnchorSpan[], bandTop: number): ReaderAnchor | null {
  for (let block = 0; block < spans.length; block += 1) {
    const span = spans[block]!;
    if (span.bottom > bandTop) return { block, offset: span.top - bandTop };
  }
  return null;
}

/**
 * How far the scroller must move (added to its offset) for `anchor`'s block
 * to stand at its recorded offset again, or null where that block is no
 * longer drawn or was never measurable.
 */
export function readerAnchorDelta(
  anchor: ReaderAnchor,
  spans: readonly AnchorSpan[],
  bandTop: number,
): number | null {
  const span = spans[anchor.block];
  if (!span) return null;
  return span.top - bandTop - anchor.offset;
}

/**
 * The list anchor over `cards` (drawn cards in draw order): the first at or
 * below the band's top, its offset, the full order, and whether the list was
 * parked at its bottom. Null where no card stands below the top.
 */
export function listAnchorOf(
  cards: readonly { key: string; top: number; bottom: number }[],
  bandTop: number,
  follow: boolean,
): ListAnchor | null {
  let key: string | null = null;
  const offsets: Record<string, number> = {};
  const order: string[] = [];
  for (const card of cards) {
    order.push(card.key);
    offsets[card.key] = card.top - bandTop;
    if (key === null && card.bottom > bandTop) key = card.key;
  }
  return key === null ? null : { key, offsets, order, follow };
}

/**
 * How far the scroller must move for `anchor`'s card to stand at its recorded
 * offset again: the anchor card where it survives, else the nearest surviving
 * card in the recorded order (after it first, then before), each at its own
 * recorded height — the redraw that took the anchor must not land the list
 * at its top. Null where no recorded card survives.
 */
export function listAnchorDelta(
  anchor: ListAnchor,
  cards: readonly { key: string; top: number }[],
  bandTop: number,
): number | null {
  const tops = new Map<string, number>();
  for (const card of cards) tops.set(card.key, card.top);
  let key: string | null = tops.has(anchor.key) ? anchor.key : null;
  if (key === null) {
    const at = anchor.order.indexOf(anchor.key);
    for (let i = at + 1; key === null && i < anchor.order.length; i += 1) {
      if (tops.has(anchor.order[i]!)) key = anchor.order[i]!;
    }
    for (let i = at - 1; key === null && i >= 0; i -= 1) {
      if (tops.has(anchor.order[i]!)) key = anchor.order[i]!;
    }
  }
  if (key === null) return null;
  const before = anchor.offsets[key];
  if (before === undefined) return null;
  return tops.get(key)! - bandTop - before;
}

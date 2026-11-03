// How far back the rows of a channel without threads run with no gaps (`#578 search-status-clarity`; `#624
// voice-page-modularity`, moved from the legacy script into core/channels): the instant of the oldest
// message of the newest read that did not reach down into the rows already loaded, and whether the rows on
// screen were drawn from this device with no read landed since. A channel with threads proves its stretch
// by its covers instead; saying any of it is the page's.

/** The plain channel's reach's commands and questions. */
export interface PlainReach {
  /** Below this instant the rows loaded are scattered, a gap between; null while every row is one stretch. */
  gapFrom(): number | null;
  /** Whether the rows on screen were drawn from this device, and no read has landed since. */
  saved(): boolean;
  /** Put back the gap a saved copy of the channel recorded (`gapFrom` as saved), or none. */
  restore(gapFrom: unknown): void;
  /**
   * A newest page of `count` messages was put on screen: `saved` where it came from this device; `kept`
   * message rows left below it of `before` there were, every one older than the whole page; `firstTimestamp`
   * its oldest message's. Kept rows mean messages may lie between that no read brought. Once a gap, always one, until a
   * fresh list.
   */
  noteNewest(saved: boolean, kept: number, before: number, count: number, firstTimestamp: unknown): void;
}

/** One stretch, read from the server. */
export function createPlainReach(): PlainReach {
  /**
   * Without threads: the instant of the oldest message of the newest read that did not reach down into
   * the rows already loaded, which are scattered below it; null while every row loaded is one stretch.
   */
  let plainGapFrom: number | null = null;
  /** Without threads: whether the rows on screen were drawn from this device, and no read has landed since. */
  let plainSaved = false;
  return {
    gapFrom: () => plainGapFrom,
    saved: () => plainSaved,
    restore(gapFrom) {
      plainGapFrom = Number.isFinite(gapFrom) ? gapFrom as number : null;
    },
    noteNewest(saved, kept, before, count, firstTimestamp) {
      plainSaved = saved;
      if (kept === 0) plainGapFrom = null;
      else if (kept === before && count > 0) {
        const from = Date.parse(String(firstTimestamp || ""));
        if (Number.isFinite(from)) plainGapFrom = plainGapFrom === null ? from : Math.max(plainGapFrom, from);
      }
    },
  };
}

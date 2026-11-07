// One step back through a channel's history: what it asks for, when it is owed, and what a page
// said about what is above (`#65 scrollback-paging`, `#62 message-count-accuracy`; `#624
// voice-page-modularity`, moved from the legacy script into core/channels; mu-readcore C3).
// The per-column record itself (`freshOlderPages`/`olderPages`: `moreAbove`, `cursor`, `inFlight`,
// `step`, `deferred`) stays in the legacy script, on the column: this owner is only the pure
// arithmetic over its `moreAbove` and over snowflake ids, and the two constants that size a step.

// What one step asks for. The server clamps it by its own `discord.max_fetch_limit`, so this is a
// ceiling on what the page WANTS rather than a promise about what it gets — which is why the walk
// is driven by the cursor the server hands back and never by arithmetic on this number.
export const DISCORD_PAGE_LIMIT = 50;

// How close to the top counts as "the reader is looking for something older".
export const OLDER_TRIGGER_PX = 80;

/** The pure questions a step back asks; see `createOlderPager`. */
export interface OlderPager {
  /** `has_more` absent means a server too old to say — which is UNKNOWN, and never "no". */
  normaliseHasMore(hasMore: unknown): boolean | undefined;
  /**
   * Is what is loaded the WHOLE channel, from `moreAbove` (`freshOlderPages`' three values:
   * `true` more above, `false` the beginning, `undefined` the server did not say)? Read from the
   * page's own state rather than from a payload, so that a re-read of the newest page cannot
   * contradict a walk that is still holding older rows.
   */
  loadedIsWhole(moreAbove: boolean | undefined): boolean | undefined;
  /**
   * Is snowflake `a` older than snowflake `b`? Discord ids are timestamps, so ORDER is
   * comparison — but they are decimal strings of differing length, and `"9" < "10"` is false
   * lexicographically. Length first, then the string.
   */
  snowflakeOlder(a: unknown, b: unknown): boolean;
}

/** The step-back questions. Stateless: every fact arrives as an argument, as it is needed. */
export function createOlderPager(): OlderPager {
  return {
    normaliseHasMore: (hasMore) => (typeof hasMore === "boolean" ? hasMore : undefined),
    loadedIsWhole: (moreAbove) => (moreAbove === undefined ? undefined : !moreAbove),
    snowflakeOlder(a, b) {
      const x = String(a === null || a === undefined ? "" : a);
      const y = String(b === null || b === undefined ? "" : b);
      if (!x || !y) {
        return false;
      }
      return x.length === y.length ? x < y : x.length < y.length;
    },
  };
}

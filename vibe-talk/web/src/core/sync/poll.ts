// When the channel is read again on its own (`#617 cold-load-first-paint` and the poll; `#624
// voice-page-modularity`, moved from the legacy script; plan step 14). Arming the timer, and pausing it
// while the page is hidden, stay with the page (`scheduleDiscordPoll`).

/** How long between the channel's own reads, the authoritative fallback behind any pushed change. */
const DISCORD_POLL_MS = 45000;

/**
 * After a page the server answered from its own copy, the next reads come this soon, one after the
 * other, rather than at the next poll, forty-five seconds on: the read upstream it stood in for runs
 * on, and has usually landed by then.
 */
const STAND_IN_RETRY_MS = [3000, 10000];

/** The stand-in reads' quick retries, held between one read landing and the next poll being armed. */
export interface StandInRetries {
  /**
   * A read landed, answered from the server's own copy (`failed`) or not. Answers whether the next poll
   * should come after the next quick retry: never once they are spent, and never after a current read,
   * which starts them again.
   */
  noteRead(failed: boolean): boolean;
  /** The wait for a poll armed for `delay`: shortened to the next quick retry when one is wanted, which it spends. */
  take(delay: number): number;
}

/** No quick retries wanted, none spent. */
export function createStandInRetries(): StandInRetries {
  /** How many of `STAND_IN_RETRY_MS` the reads since the last current one have taken. */
  let standInRetries = 0;
  /** Whether the next poll armed should come after the next of `STAND_IN_RETRY_MS`. */
  let standInRetryWanted = false;
  return {
    noteRead(failed) {
      if (failed) {
        standInRetryWanted = standInRetries < STAND_IN_RETRY_MS.length;
      } else {
        standInRetries = 0;
        standInRetryWanted = false;
      }
      return standInRetryWanted;
    },
    take(delay) {
      if (!standInRetryWanted) return delay;
      const wait = Math.min(delay, STAND_IN_RETRY_MS[standInRetries] as number);
      standInRetries += 1;
      standInRetryWanted = false;
      return wait;
    },
  };
}

export { DISCORD_POLL_MS, STAND_IN_RETRY_MS };

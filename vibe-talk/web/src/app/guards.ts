// The page's quiet guards (`#624 voice-page-modularity`, moved from the legacy script):
// run a thing, say what failed, and never tear down a live conversation for it. Words
// and order are as they were in the script. The banner and the failure's words are the
// page's, handed in.

/** A failure, as the guards read it. */
export interface GuardedFailure {
  message?: string;
  shownInPill?: unknown;
}

/** What the quiet guards ask of the page. */
export interface PageGuardsDependencies {
  showError(text: string): void;
  readableChannelFailure(error: GuardedFailure): string;
}

/** The quiet guards. */
export interface PageGuards {
  guardQuietly<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => Promise<Awaited<R> | undefined>;
  refreshQuietly<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => Promise<Awaited<R> | undefined>;
}

/** The page's quiet guards over `deps`. */
export function createPageGuards(deps: PageGuardsDependencies): PageGuards {
  /**
   * Like `guard`, but for things that are not a call: it reports, and it does NOT tear down a live
   * conversation. Reading the channel list must never be able to hang up on the owner.
   */
  function guardQuietly<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => Promise<Awaited<R> | undefined> {
    return (...args) =>
      Promise.resolve(fn(...args)).catch((error) => {
        deps.showError(error.message);
      }) as Promise<Awaited<R> | undefined>;
  }

  /**
   * Like `guardQuietly`, for a re-read of the channel nobody asked for: the poll, the live feed's
   * catch-up. `#195 send-resilience`.
   *
   * Its failure is already said over the rows it is about, by the freshness pill — "Refresh failed ·
   * showing messages from 10:17". The banner on top of that was the same news a second time, in the
   * server's diagnostic words, back on every poll for as long as the service stayed slow, and it
   * read as though something the reader did had failed. The banner stays for what the reader asked
   * for, and for a failure with no rows on screen to carry it — reworded, never raw.
   */
  function refreshQuietly<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => Promise<Awaited<R> | undefined> {
    return (...args) =>
      Promise.resolve(fn(...args)).catch((error) => {
        if (error && error.shownInPill) return;
        deps.showError(deps.readableChannelFailure(error));
      }) as Promise<Awaited<R> | undefined>;
  }

  return { guardQuietly, refreshQuietly };
}

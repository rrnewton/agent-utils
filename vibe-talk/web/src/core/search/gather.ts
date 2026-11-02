// Gathering a channel's older history for the search (`#608 server-side-search`; `#624
// voice-page-modularity`, moved from the legacy script into core/search). "Search further back" asks the
// server to put the channel's history walk first; this owner holds that ask: the channel, a run number
// (so an answer to an earlier ask is dropped), the walk's last known state, why it failed, and the poll
// every `SEARCH_FURTHER_POLL_MS` while the walk goes on. The requests, drawing it and what the search
// does when the copy grows are the page's, passed in.

import type { ChannelHistoryState } from "../../contract/index.ts";
import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How often a gathering under way is asked how far it has come, in milliseconds. */
const SEARCH_FURTHER_POLL_MS = 5000;

/** The gathering as it stands. */
export interface GatheringState {
  /** The channel it gathers, or "" for none. */
  channel: string;
  /** Which ask this is: an answer to another is dropped. */
  run: number;
  /** The walk's state as last answered, or null before the first answer. */
  state: ChannelHistoryState | null;
  /** The poll waiting to ask again, or null. */
  timer: TimerHandle | null;
  /** Why the last ask failed, or "". */
  failed: string;
}

/** A failure as the page's API client throws it. */
interface Refusal {
  readonly code?: unknown;
  readonly detail?: unknown;
  readonly message?: unknown;
}

/** What the gathering asks of the page. */
export interface GatheringDependencies {
  /** The timer the poll runs on. */
  readonly clock: ClockPort;
  /** Ask the server to put `channel`'s walk first; resolves with the walk's state. */
  prioritize(channel: string): Promise<ChannelHistoryState>;
  /** Ask how far `channel`'s walk has come. */
  fetchState(channel: string): Promise<ChannelHistoryState>;
  /** Draw the search, and the gathering's line in it, again. */
  render(): void;
  /** The copy the search reads grew: the same search, asked again from the start. */
  grew(): void;
  /** Run `fn` from a timer, reporting a failure quietly. */
  guard(fn: () => Promise<void>): void;
}

/** The gathering's commands. */
export interface Gathering {
  /** The gathering as it stands: read, never written. */
  state(): Readonly<GatheringState>;
  /** Start gathering `channel`: ask the server, draw it, and poll while the walk goes on. */
  start(channel: string): Promise<void>;
  /** Stop: forget the gathering and its poll, and draw the search again. */
  stop(): void;
}

/** The words a refusal of the gathering says. */
const refusalWords = (error: unknown): string => {
  const said = error as Refusal | null;
  return String((said && (said.detail || said.message)) || "the server did not answer");
};

/** Nothing gathered. */
export function createGathering(deps: GatheringDependencies): Gathering {
  let searchFurther: GatheringState = { channel: "", run: 0, state: null, timer: null, failed: "" };
  let searchFurtherRuns = 0;

  function pollSearchFurther(channel: string, run: number): void {
    searchFurther.timer = deps.clock.setTimeout(() => deps.guard(async () => {
      if (searchFurther.run !== run) return;
      const before = searchFurther.state;
      let state: ChannelHistoryState | null = null;
      try {
        state = await deps.fetchState(channel);
      } catch (error) {
        if (searchFurther.run !== run) return;
        searchFurther.failed = refusalWords(error);
        deps.render();
        pollSearchFurther(channel, run);
        return;
      }
      if (searchFurther.run !== run) return;
      searchFurther.state = state;
      searchFurther.failed = "";
      const older = (held: ChannelHistoryState | null): number | null =>
        (held && typeof held.oldest_ms === "number" ? held.oldest_ms : null);
      if (older(state) !== older(before) || state.complete !== Boolean(before && before.complete)) {
        // The copy grew: the same search, asked again from the start.
        deps.grew();
      } else {
        deps.render();
      }
      const going = state.walking || (state.queue_position !== null && state.queue_position !== undefined) ||
        (typeof state.held_until_ms === "number");
      if (!state.complete && going) pollSearchFurther(channel, run);
      else searchFurther.timer = null;
    }), SEARCH_FURTHER_POLL_MS);
  }

  return {
    state: () => searchFurther,
    async start(channel) {
      searchFurtherRuns += 1;
      const run = searchFurtherRuns;
      searchFurther = { channel, run, state: null, timer: null, failed: "" };
      deps.render();
      try {
        const state = await deps.prioritize(channel);
        if (searchFurther.run !== run) return;
        searchFurther.state = state;
      } catch (error) {
        if (searchFurther.run !== run) return;
        const code = (error as Refusal | null)?.code;
        searchFurther.failed = error && (code === "history_backfill_off" || code === "history_not_cached")
          ? "the server does not gather history for this channel"
          : refusalWords(error);
      }
      deps.render();
      if (!searchFurther.failed) pollSearchFurther(channel, run);
    },
    stop() {
      if (searchFurther.timer !== null) deps.clock.clearTimeout(searchFurther.timer);
      searchFurther = { channel: "", run: 0, state: null, timer: null, failed: "" };
      deps.render();
    },
  };
}

// What the page and the tests use of it.
export { SEARCH_FURTHER_POLL_MS };

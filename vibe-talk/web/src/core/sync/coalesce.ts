// Coalescing a burst of live edits and removals into authoritative re-reads without leaving a stale tail
// (`#624 voice-page-modularity`, moved from the legacy script into core/sync): whether a re-read is owed,
// whether one is running, and the loop that waits out an ordinary read already in flight and reads again
// while more were asked for. The read itself, and whether a hidden page asks at all, are the page's.

import type { ClockPort } from "../../ports/index.ts";

/** What the re-reads ask of the page. */
export interface MutationRefreshDependencies {
  readonly clock: ClockPort;
  /** Whether an ordinary page read is on the wire now. */
  busy(): boolean;
  /** One authoritative re-read: the channel's view, then the thread on the right. */
  read(): Promise<void>;
  /** Run the loop, its failures caught quietly (`refreshQuietly`). */
  launch(run: () => Promise<void>): void;
}

/** The re-reads' one command. */
export interface MutationRefresh {
  /** An edit or a removal arrived: re-read, once for a whole burst, and again for one arriving during it. */
  request(): void;
}

/** Nothing owed, nothing running. */
export function createMutationRefresh(deps: MutationRefreshDependencies): MutationRefresh {
  let liveMutationRefreshRunning = false;
  let liveMutationRefreshNeeded = false;

  return {
    request() {
      liveMutationRefreshNeeded = true;
      if (liveMutationRefreshRunning) {
        return;
      }
      liveMutationRefreshRunning = true;
      deps.launch(async () => {
        try {
          while (liveMutationRefreshNeeded) {
            liveMutationRefreshNeeded = false;
            // If an ordinary page read is already in flight, wait for it. It may have started before
            // the adapter applied this mutation, so treating it as the refresh could preserve a stale
            // row forever.
            while (deps.busy()) {
              await new Promise<void>((resolve) => {
                deps.clock.setTimeout(resolve, 50);
              });
            }
            await deps.read();
          }
        } finally {
          liveMutationRefreshRunning = false;
        }
      });
    },
  };
}

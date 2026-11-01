// Main's messages cut into the runs that go before each card (`#624 voice-page-modularity`, moved from
// the legacy script into present/rows; `#261 two-column-connectors`).

import { timeOf } from "../../core/channels/index.ts";

/**
 * Main's messages `main`, oldest first, cut into the runs that go before each of the cards whose times
 * are `cards` (oldest first), and a last run after them all: `cards.length + 1` runs.
 */
function mainRunsAmong<M>(main: readonly M[], cards: readonly number[]): M[][] {
  const runs: M[][] = cards.map(() => []);
  runs.push([]);
  let card = 0;
  for (const message of main) {
    const at = timeOf(message, "timestamp");
    while (card < cards.length && Number.isFinite(at) && (cards[card] as number) <= at) card += 1;
    (runs[card] as M[]).push(message);
  }
  return runs;
}

export { mainRunsAmong };

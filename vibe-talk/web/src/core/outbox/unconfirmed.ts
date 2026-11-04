// Every unconfirmed send, asked of the server once per load (`#468 send-dedupe`; `#624
// voice-page-modularity`, moved from the legacy script into core/outbox). What asking one is, and when it is
// left alone, is the page's.

/** What asking needs: the outbox's ids, and the ask for one of them. */
export interface UnconfirmedSendsDependencies {
  ids(): Iterable<string>;
  settle(id: string): void;
}

/** Once per load. */
export interface UnconfirmedSends {
  /** Ask about every send in the outbox, the first time only; later calls do nothing. */
  settleOnce(): void;
}

export function createUnconfirmedSends(deps: UnconfirmedSendsDependencies): UnconfirmedSends {
  let unconfirmedSettled = false;
  return {
    settleOnce() {
      if (unconfirmedSettled) return;
      unconfirmedSettled = true;
      for (const id of [...deps.ids()]) deps.settle(id);
    },
  };
}

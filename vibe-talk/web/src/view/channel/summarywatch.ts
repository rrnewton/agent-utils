// The one timer for the cards waiting on a summary (`#528 thread-card-markdown-and-cap`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): each source that draws cards (the cards, the card at the
// top of a thread) says the next moment its cards are owed something, a read or the end of a wait, and one
// timer is armed for the soonest of all, again only where that moment changed. When it fires, every source
// is forgotten, to be told again as the cards are drawn. Working out what each card is owed, and what the
// timer does, are the page's (`watchPendingSummaries`, `summaryWatchFired`).

/** What the timer asks of the page. */
export interface SummaryWatchDependencies {
  /** Arm a timer (`setTimeout`). */
  setTimer(run: () => void, ms: number): unknown;
  /** Disarm one (`clearTimeout`). */
  clearTimer(timer: unknown): void;
  /** The soonest moment came. */
  fired(): void;
}

/** The timer's commands. */
export interface SummaryWatch {
  /** `source`'s cards are next owed something at `owed` (Infinity: never), `now` being the time now. */
  owe(source: string, owed: number, now: number): void;
  /** The moment the timer is armed for, or Infinity. */
  at(): number;
}

/** No timer armed. */
export function createSummaryWatch(deps: SummaryWatchDependencies): SummaryWatch {
  let summaryWatch: { at: number; timer: unknown } = { at: Infinity, timer: null };
  const summaryWatchNeeds = new Map<string, number>();
  return {
    owe(source, owed, now) {
      summaryWatchNeeds.set(source, owed);
      const next = Math.min(...summaryWatchNeeds.values());
      if (next === summaryWatch.at) return;
      if (summaryWatch.timer !== null) deps.clearTimer(summaryWatch.timer);
      summaryWatch = { at: next, timer: null };
      if (next === Infinity) return;
      summaryWatch.timer = deps.setTimer(() => {
        summaryWatch = { at: Infinity, timer: null };
        summaryWatchNeeds.clear();
        deps.fired();
      }, Math.max(0, next - now));
    },
    at: () => summaryWatch.at,
  };
}

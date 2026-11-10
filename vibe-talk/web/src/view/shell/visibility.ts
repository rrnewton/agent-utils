// The page going hidden and coming back (`#203 incremental-refresh`, `#226 page-energy-profile`;
// `#624 voice-page-modularity`, moved from the legacy script into view/shell beside
// documentlisteners.ts): the away clock, what a hidden page owes on return, and the refresh a long
// absence earns. What each act does is the page's, handed in as it was taken here.

/** What the page's visibility answer asks of the page. */
export interface PageVisibilityDependencies {
  /** Keep the call's screen wake lock in step with the page. */
  syncScreenWakeLock(): void;
  /** Whether the page is hidden now. */
  hidden(): boolean;
  /** Note the page went hidden at `at`. */
  hide(at: number): void;
  /** Whether a call's socket is open. */
  callSocketOpen(): boolean;
  /** Note a call was open while the page hid. */
  noteHiddenDuringCall(): void;
  /** Stop reading a channel nobody can see. */
  pauseWhileHidden(): void;
  /** Note the page is back at `at`. */
  noteVisibleAt(at: number): void;
  /** How long the reader was away, the page back at `at`. */
  awayMs(at: number): number;
  /** A send whose wait ran out while the reader was away goes now. */
  resumeOutgoingRetries(): void;
  /** What the stream delivered while the page was hidden, or null. */
  catchUpHiddenArrivals(): string | null;
  /** Whether reply arrows are owed a measure. */
  replyArrowsOwed(): boolean;
  /** Measure the reply arrows. */
  measureReplyArrows(): void;
  /** Run `act` in the other column. */
  inOtherColumn(act: () => void): void;
  /** The live stream's held resume, taken. */
  takeStreamResume(): unknown;
  /** Whether the channel poll is paused. */
  pollPaused(): boolean;
  /** When the channel poll is due. */
  pollDueAt(): number;
  /** How long away earns a refresh on return. */
  visibleRefreshMs: number;
  /** The view the main screen shows ("voice" or "discord"). */
  view(): string;
  /** Whether the chosen service threads. */
  threading(): boolean;
  /** Whether the server's client config has been applied. */
  configApplied(): boolean;
  /** The channel chosen now. */
  selectedChannelId(): unknown;
  /** Run a quiet refresh of the channel. */
  refreshQuietly(run: () => Promise<void>): () => void;
  /** An edit the stream delivered is read for wherever the reader is. */
  refreshAfterLiveMutation(): void;
  /** When replay began. */
  replayStarted(): unknown;
  /** Read the channel again, keeping the reader's place. */
  loadDiscordVisible(): Promise<void>;
  /** Read the side column again. */
  readSideColumn(reason: string, since: unknown): Promise<void>;
  /** Bring the live stream back from what it held. */
  resumeLiveStream(resume: unknown): void;
  /** Resume polling, fresh, now or at rest. */
  resumePolling(mode: string): void;
  /** The clock, as the page reads it. */
  now(): number;
}

/** The page's visibility answer: its one command, heard on `visibilitychange`. */
export interface PageVisibility {
  /** The page went hidden, or came back. */
  onVisibility(): void;
}

/** Made once, where the page wires its document listeners. */
export function createPageVisibility(deps: PageVisibilityDependencies): PageVisibility {
  function onVisibility(): void {
    deps.syncScreenWakeLock();
    if (deps.hidden()) {
      deps.hide(deps.now());
      // Only a call can be suspended. Hiding an idle page is not an event.
      if (deps.callSocketOpen()) {
        deps.noteHiddenDuringCall();
      }
      // `#226 page-energy-profile`. Nor is refreshing a channel nobody can see.
      deps.pauseWhileHidden();
      return;
    }
    const visibleAt = deps.now();
    deps.noteVisibleAt(visibleAt);
    const away = deps.awayMs(visibleAt);
    // `#195 send-resilience`. A phone holds a hidden page's timers; a send whose wait ran out
    // while the reader was away goes now, not a further wait after they are back.
    deps.resumeOutgoingRetries();
    // `#226 page-energy-profile`. What the stream delivered while the page was hidden is drawn
    // now, from the store, and the arrows a hidden page did not measure are measured.
    const owed = deps.catchUpHiddenArrivals();
    if (deps.replyArrowsOwed()) deps.measureReplyArrows();
    deps.inOtherColumn(() => {
      if (deps.replyArrowsOwed()) deps.measureReplyArrows();
    });
    const resume = deps.takeStreamResume();
    const pollDue = deps.pollPaused() && deps.pollDueAt() <= visibleAt;
    // `#203 incremental-refresh`. Coming back to the channel after a while is the moment a
    // reader most wants it current, and a phone held the poll's timer the whole time it was
    // away. Only what changed is asked for, so this costs one small read rather than the next
    // poll's wait. `#226`: and after any absence the stream delivered something in, the poll
    // came due in, or that let the stream go.
    if ((away >= deps.visibleRefreshMs || owed !== null || pollDue || resume !== null) &&
        deps.view() === "discord" && deps.threading() && deps.configApplied() && deps.selectedChannelId()) {
      deps.refreshQuietly(async () => {
        try {
          if (owed === "mutation") deps.refreshAfterLiveMutation();
          else {
            const since = deps.replayStarted();
            await deps.loadDiscordVisible();
            await deps.readSideColumn("visible", since);
          }
        } finally {
          // The stream comes back once that read has brought what it missed, so what it replays
          // from the last event it saw is held already rather than drawn a row at a time.
          deps.resumeLiveStream(resume);
        }
      })();
      deps.resumePolling("fresh");
      return;
    }
    // An edit the stream delivered is read for wherever the reader is, as it was when it arrived.
    if (owed === "mutation") deps.refreshAfterLiveMutation();
    deps.resumeLiveStream(resume);
    deps.resumePolling(pollDue ? "now" : "rest");
  }

  return { onVisibility };
}

// Taking the pace popover down and turning read-aloud reading mode on and off (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they
// were in the script. The read-aloud owner, the banner and the status are the page's,
// handed in. This is the READ-ALOUD mode, not the message read modes (`setReadMode`).

/** The read-aloud owner, as turning its mode on and off uses it. */
export interface ReadingActsOwner {
  readingMode: boolean;
  stopReading(): void;
  setReadState(state: string): void;
  readAloudPlayback: string;
  browserSpeechProblem(): string;
  prepareSpeech: () => unknown;
  forgetPreparedSpeech(): void;
  readAloudLabel: string;
}

/** What the reading-mode acts ask of the page. */
export interface ReadingModeActsDependencies {
  el(id: string): HTMLElement;
  scheduleBackSync(): void;
  readAloudOwner(): ReadingActsOwner;
  clearError(): void;
  showError(text: string): void;
  guardQuietly(fn: () => unknown): () => unknown;
  setStatus(text: string): void;
  renderControls(): void;
  renderChannelRows(): void;
}

/** The pace popover down, and reading mode on, off, and left. */
export interface ReadingModeActs {
  closeSpeedPopover(): void;
  leaveReadingMode(): void;
  setReadingMode(on: boolean): void;
}

/** The reading-mode acts over `deps`. */
export function createReadingModeActs(deps: ReadingModeActsDependencies): ReadingModeActs {
  /** Take the pace popover down. Safe to call when it is already down. */
  function closeSpeedPopover(): void {
    deps.el("speed-popover").hidden = true;
    deps.el("read-speed").setAttribute("aria-expanded", "false");
    deps.scheduleBackSync();
  }

  /**
   * `#442 read-strip-order`. Stop: the read ends AND reading mode is left, in one tap, by the path
   * the Read button has always left it by. After #434 made Read the Pause of a read in progress, the
   * only ways out were a tap on the row being read or the media keys' stop, which a reader holding a
   * phone in a car does not find. The button pressed goes as the mode does, so focus goes to Read.
   */
  function leaveReadingMode(): void {
    const from = document.activeElement;
    if (deps.readAloudOwner().readingMode) setReadingMode(false);
    else deps.readAloudOwner().stopReading();
    if (from && from === deps.el("read-aloud-stop")) {
      const read = deps.el("read-aloud");
      if (!read.hidden && typeof read.focus === "function") read.focus();
    }
  }

  /** Turn reading mode on or off. The one path in and out of it, whichever control asked. */
  function setReadingMode(on: boolean): void {
    deps.readAloudOwner().readingMode = on;
    // Turning it OFF stops what is playing. Leaving audio running under a reader who has just said
    // "not this any more" is the kind of thing that gets a tab muted and never opened again.
    if (!deps.readAloudOwner().readingMode) {
      deps.readAloudOwner().stopReading();
    }
    deps.readAloudOwner().setReadState(deps.readAloudOwner().readingMode ? "ready" : "idle");
    if (deps.readAloudOwner().readingMode) {
      // A failed conversation banner belongs to the call that produced it. Leaving it over the
      // independent device-speech mode makes a working Read control look broken; if device speech
      // itself is unavailable, replace it immediately with that actionable reason.
      deps.clearError();
      const speechProblem = deps.readAloudOwner().readAloudPlayback === "browser" ? deps.readAloudOwner().browserSpeechProblem() : "";
      if (speechProblem) {
        deps.readAloudOwner().setReadState("failed");
        deps.showError(speechProblem);
      }
      // AHEAD OF THE TAP. This is the request that makes every later tap cheap, and it is issued the
      // moment the mode is entered rather than when a message is chosen, because the reader is
      // already looking at the text and has not decided yet which line they want.
      deps.guardQuietly(deps.readAloudOwner().prepareSpeech)();
    } else {
      deps.readAloudOwner().forgetPreparedSpeech();
    }
    deps.setStatus(
      deps.readAloudOwner().readingMode
        ? deps.readAloudOwner().readAloudPlayback === "browser"
          ? deps.readAloudOwner().browserSpeechProblem() || `${deps.readAloudOwner().readAloudLabel}: tap a message to hear it, and it archives when it finishes.`
          : "reading mode: tap a message to hear it, and it archives when it finishes."
        : "reading mode off."
    );
    deps.renderControls();
    deps.renderChannelRows();
  }

  return { closeSpeedPopover, leaveReadingMode, setReadingMode };
}

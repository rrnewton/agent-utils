// What the live stream holds while the page is hidden (`#226 page-energy-profile`; `#624
// voice-page-modularity`, moved from the legacy script into core/sync): whether a message, or an edit or a
// removal, arrived for coming back to draw and read; the timer that lets the stream go after
// `LIVE_HIDDEN_CLOSE_MS` with no call listening; and the stream let go, to resume from its last event.
// Pausing the poll, drawing what arrived and the read that coming back makes are the page's; why a hidden
// page does each of these is told where the legacy script opens the section.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long a hidden page with no call keeps its live stream: two minutes. */
const LIVE_HIDDEN_CLOSE_MS = 120000;

/** The stream let go while hidden: its channel, the last event it saw, and the generation stopping it left. */
export interface HiddenResume {
  readonly channel: string;
  readonly from: string | null;
  readonly generation: number;
}

/** What the hidden stream asks of the page. */
export interface HiddenStreamDependencies {
  readonly clock: ClockPort;
  /** The stream followed (`ChannelFollow`): its channel, null with none; its last event; its generation. */
  readonly follow: {
    channel(): string | null;
    lastEventId(): string | null;
    generation(): number;
  };
  /** Whether the page is hidden now (`document.hidden`). */
  hidden(): boolean;
  /** Whether a call is listening, which keeps the stream (its socket open). */
  callListening(): boolean;
  /** Stop the stream (`stopChannelStream`). */
  stop(): void;
}

/** The hidden stream's commands and questions. */
export interface HiddenStream {
  /** The page went out of sight: let the stream go after `LIVE_HIDDEN_CLOSE_MS`, where one is followed. */
  hide(): void;
  /** The stream delivered a message for this channel while hidden. */
  noteMessage(): void;
  /** It delivered an edit or a removal then, which a read answers. */
  noteMutation(): void;
  /**
   * Back in sight: the timer stopped, and what arrived while hidden taken and forgotten — `mutation`, an
   * edit or a removal; `arrived`, a message, or `waiting()` (replays still waiting to be drawn).
   */
  takeArrivals(waiting: () => boolean): { readonly mutation: boolean; readonly arrived: boolean };
  /** The stream let go while hidden, taken and forgotten; null with none. */
  takeResume(): HiddenResume | null;
}

/** Nothing arrived, nothing let go. */
export function createHiddenStream(deps: HiddenStreamDependencies): HiddenStream {
  /** Whether the stream delivered a message for this channel while the page was hidden. */
  let liveArrivedHidden = false;
  /** Whether it delivered an edit or a removal then, which a read answers. */
  let mutationArrivedHidden = false;
  /** The timer that lets the stream go while the page stays hidden, or null. */
  let liveHiddenTimer: TimerHandle | null = null;
  /** The stream let go while hidden, to resume, or null. */
  let liveResumeAfterHidden: HiddenResume | null = null;

  /** Still hidden, and no call is listening: close the stream, keeping where it was up to. */
  function letStreamGoWhileHidden(): void {
    liveHiddenTimer = null;
    if (!deps.hidden() || deps.follow.channel() === null || deps.callListening()) return;
    const resume = { channel: String(deps.follow.channel()), from: deps.follow.lastEventId() };
    deps.stop();
    liveResumeAfterHidden = { ...resume, generation: deps.follow.generation() };
  }

  return {
    hide() {
      if (liveHiddenTimer === null && deps.follow.channel() !== null) {
        liveHiddenTimer = deps.clock.setTimeout(letStreamGoWhileHidden, LIVE_HIDDEN_CLOSE_MS);
      }
    },
    noteMessage() {
      liveArrivedHidden = true;
    },
    noteMutation() {
      mutationArrivedHidden = true;
    },
    takeArrivals(waiting) {
      if (liveHiddenTimer !== null) {
        deps.clock.clearTimeout(liveHiddenTimer);
        liveHiddenTimer = null;
      }
      const mutation = mutationArrivedHidden;
      const arrived = liveArrivedHidden || waiting();
      mutationArrivedHidden = false;
      liveArrivedHidden = false;
      return { mutation, arrived };
    },
    takeResume() {
      const resume = liveResumeAfterHidden;
      liveResumeAfterHidden = null;
      return resume;
    },
  };
}

// What the page and the tests use of it.
export { LIVE_HIDDEN_CLOSE_MS };

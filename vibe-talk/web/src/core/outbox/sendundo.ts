// The Undo a send dismissed from the outbox offers (`#193 compose-draft-loss`; `#624 voice-page-modularity`,
// moved from the legacy script into core/outbox): which dismissal the chip takes back while it is up — the
// entry's id, its wait for an automatic send if it had one, the stamp that orders it among the page's offers —
// and the timer that takes the chip away after `SEND_UNDO_MS`. Dismissing, putting back and drawing the chip
// are the page's.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long the Undo chip offers to take a Dismiss back. */
const SEND_UNDO_MS = 15000;

/** The dismissal the Undo chip takes back while it is up. */
export interface SendDismissal {
  /** The dismissed entry's id. */
  readonly id: string;
  /** Its wait for an automatic send, if it had one, for Undo to set waiting again; null otherwise. */
  readonly hold: unknown;
  /** Its place among the page's Undo offers. */
  readonly at: number;
}

/** What the Undo asks of the page. */
export interface SendUndoDependencies {
  readonly clock: ClockPort;
  /** The chip's time is up, or the offer ended: draw the chip again (`renderSendUndo`). */
  ended(): void;
}

/** The Undo's commands and questions. */
export interface SendUndo {
  /** The dismissal the chip takes back, or null with none up. */
  current(): SendDismissal | null;
  /** Offer Undo for `dismissal` for `SEND_UNDO_MS`, replacing any offer up. The page draws the chip after. */
  offer(dismissal: SendDismissal): void;
  /** End the offer, if one is up, and draw the chip again. */
  end(): void;
}

/** No offer up. */
export function createSendUndo(deps: SendUndoDependencies): SendUndo {
  /** The dismissal the Undo chip takes back while it is up, and the chip's timer. */
  let sendUndo: (SendDismissal & { timer: TimerHandle }) | null = null;

  function endSendUndo(): void {
    if (sendUndo) deps.clock.clearTimeout(sendUndo.timer);
    sendUndo = null;
    deps.ended();
  }

  return {
    current: () => sendUndo,
    offer(dismissal) {
      if (sendUndo) deps.clock.clearTimeout(sendUndo.timer);
      sendUndo = { ...dismissal, timer: deps.clock.setTimeout(endSendUndo, SEND_UNDO_MS) };
    },
    end: endSendUndo,
  };
}

// What the page and the tests use of it.
export { SEND_UNDO_MS };

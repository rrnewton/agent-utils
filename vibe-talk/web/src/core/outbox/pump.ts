// Starting what is queued (`#624 voice-page-modularity`, moved from the legacy script into core/outbox): one
// pass at a time over the outbox, and a pass asked for while one runs — starting a send can finish it at
// once, signed out or offline, and that asks for a pass of its own — makes the running one go round again
// rather than nesting inside it; and the running number in each new send's id. What a pass starts is the
// page's (`pumpOutgoing`).

/** The pump's command. */
export interface OutboxPump {
  /** Run `pass` now, again until no pass was asked for during it; asked during a pass, ask it to go round. */
  pump(pass: () => void): void;
}

/** Idle. */
export function createOutboxPump(): OutboxPump {
  let outgoingPumping = false;
  let outgoingPumpAgain = false;
  return {
    pump(pass) {
      if (outgoingPumping) {
        outgoingPumpAgain = true;
        return;
      }
      outgoingPumping = true;
      try {
        do {
          outgoingPumpAgain = false;
          pass();
        } while (outgoingPumpAgain);
      } finally {
        outgoingPumping = false;
      }
    },
  };
}

/** The running number in new sends' ids. */
export interface SendNumbers {
  /** The next number: 1, 2, ... */
  next(): number;
}

/** None given yet. */
export function createSendNumbers(): SendNumbers {
  let outgoingSequence = 0;
  return {
    next: () => {
      outgoingSequence += 1;
      return outgoingSequence;
    },
  };
}

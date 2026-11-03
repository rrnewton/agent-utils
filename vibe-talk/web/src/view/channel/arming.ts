// Asking twice before a bulk act (`#50 todo-view`; `#624 voice-page-modularity`, moved from the legacy
// script into view/channel): the backlog control armed by its first press, the window it stays armed for,
// and disarmed by its second, by the window passing, or by anything else that should put it down. What the
// control says, and what the second press does, are the page's.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** What the arming asks of the page. */
export interface ArmingDependencies {
  readonly clock: ClockPort;
  /** How long a first press keeps the control armed, in ms: the dock's Clear window, the same idiom. */
  ms(): number;
  /** Draw the control again (`renderTodoControls`). */
  drawn(): void;
}

/** The arming's commands and questions. */
export interface Arming {
  /** Whether the control is armed now. */
  armed(): boolean;
  /** Arm it, for its window afresh, and draw it. */
  arm(): void;
  /** Disarm it, and draw it. */
  disarm(): void;
}

/** Not armed. */
export function createArming(deps: ArmingDependencies): Arming {
  // Bulk and destructive, so it asks twice — the same armed idiom the Clear control on the dock
  // uses, and the same window, because a reader who has learnt one has learnt the other.
  let backlogArmedTimer: TimerHandle | null = null;

  function disarm(): void {
    if (backlogArmedTimer !== null) {
      deps.clock.clearTimeout(backlogArmedTimer);
      backlogArmedTimer = null;
    }
    deps.drawn();
  }

  return {
    armed: () => backlogArmedTimer !== null,
    arm() {
      if (backlogArmedTimer !== null) {
        deps.clock.clearTimeout(backlogArmedTimer);
      }
      backlogArmedTimer = deps.clock.setTimeout(disarm, deps.ms());
      deps.drawn();
    },
    disarm,
  };
}

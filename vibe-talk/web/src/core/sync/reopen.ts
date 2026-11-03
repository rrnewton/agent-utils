// The read a reopen owes the channel it put on screen (`#624 voice-page-modularity`, moved from the legacy
// script into core/sync): `after` is the count of timeline reads when it was owed, so a read the reader made
// since — by the switch, by changing channel — has read the channel and paid it. Not read at load: a read sent
// before `/client-config` answers uses a token nobody has proved, and one sent before the live stream
// attaches cannot answer for the stream's replay tail (`#43 replay-burst-double-read`). When it is paid, and
// what reading it means, are the page's (`reopenSavedView`, `payReopenRead`).

/** What a reopen owes: the count of timeline reads when it was owed. */
export interface OwedRead {
  readonly after: number;
}

/** The owed read's commands. */
export interface ReopenRead {
  /** A reopen put the channel on screen with `after` reads sent so far: its newest page is owed. */
  owe(after: number): void;
  /** The read owed, taken to be paid, or null where none is. */
  take(): OwedRead | null;
  /** `owed` could not be paid now (signed out while it waited): owed again, where nothing newer is. */
  keep(owed: OwedRead): void;
}

/** Nothing owed. */
export function createReopenRead(): ReopenRead {
  let reopenReadOwed: OwedRead | null = null;
  return {
    owe(after) {
      reopenReadOwed = { after };
    },
    take() {
      const owed = reopenReadOwed;
      reopenReadOwed = null;
      return owed;
    },
    keep(owed) {
      reopenReadOwed = reopenReadOwed || owed;
    },
  };
}

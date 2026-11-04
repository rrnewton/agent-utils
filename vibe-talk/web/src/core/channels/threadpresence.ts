// Whether the selected channel holds any thread at all (`#624 voice-page-modularity`, moved from the legacy
// script into core/channels): what its last history page said, or yes while a thread is open. A channel with
// none has nothing in All but its top-level messages. Nothing is known for a channel just opened or reopened.

/** The channel's thread presence: its question and commands. */
export interface ThreadPresence {
  /** Whether the last page said the channel holds threads, or one was open when it came. */
  any(): boolean;
  /** A history page came: `hasThreads` is its `has_threads`, `inThread` whether a thread is selected. */
  note(hasThreads: unknown, inThread: boolean): void;
  /** Another channel, or this one again: nothing is known. */
  forget(): void;
}

/** Nothing known yet. */
export function createThreadPresence(): ThreadPresence {
  let channelHasThreads = false;
  return {
    any: () => channelHasThreads,
    note(hasThreads, inThread) {
      channelHasThreads = hasThreads === true || inThread;
    },
    forget() {
      channelHasThreads = false;
    },
  };
}

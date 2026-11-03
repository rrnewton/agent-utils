// Whether the channel picker was opened by Ctrl+K (Cmd+K) and no channel has been chosen from it since
// (`#224 keyboard-shortcuts`, question 11; `#624 voice-page-modularity`, moved from the legacy script into
// view/shell). Only then does a channel chosen from it hand the keys back to the list: a picker reached by a
// click or by Tab keeps the focus through a change, because on Windows and Linux ↑ and ↓ on a closed picker
// step it one channel at a time, firing `change` at each step. Opening the picker and moving the focus are the
// page's (`pickChannel`, `channelPicked`).

/** The flag's commands and question. */
export interface ChannelPick {
  /** Ctrl+K opened the picker. */
  openedByKey(): void;
  /** Whether it did, and nothing has been chosen from it since. */
  byKey(): boolean;
  /** A channel was chosen, or the picker was left by Esc: the keys went back to the list. */
  forget(): void;
}

/** Not opened by a key. */
export function createChannelPick(): ChannelPick {
  let channelPickByKey = false;
  return {
    openedByKey() {
      channelPickByKey = true;
    },
    byKey: () => channelPickByKey,
    forget() {
      channelPickByKey = false;
    },
  };
}

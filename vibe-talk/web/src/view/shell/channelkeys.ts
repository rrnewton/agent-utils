// The channel keys (`#224 keyboard-shortcuts`, question 11; `#355 provider-grouped-channels`; `#624
// voice-page-modularity`, moved from the legacy script into view/shell for the plan's step 17): Alt+↓ and Alt+↑
// step through the channels in the picker's order, Ctrl+K (Cmd+K) opens the picker, and a channel chosen from the
// picker Ctrl+K opened hands the keys back to the list. The channels, choosing one and the list are the page's.

import { createChannelPick } from "./channelpick.ts";

/** A channel, as far as the keys read it. */
export interface SteppedChannel {
  readonly id: unknown;
}

/** The picker on the bar, as far as the keys use it. */
export interface ChannelPicker {
  readonly hidden: boolean;
  readonly disabled?: boolean;
  focus(): void;
  showPicker?(): void;
}

export interface ChannelKeysDependencies {
  /** Whether the list of the channel is the one on screen. */
  channelOnScreen(): boolean;
  /** The channel picker on the bar. */
  picker(): ChannelPicker;
  /** The channels in the order the picker shows them, across service groups. */
  channelsInPickerOrder(): readonly SteppedChannel[];
  /** The channel chosen, or a value that is not one. */
  selectedChannelId(): unknown;
  /** Choose the channel `id` on the picker. */
  chooseChannel(id: string): void;
  /** Read the channel now chosen. */
  changeChannel(): void;
  /** Settle the focus on the list. */
  settleFocus(): void;
}

/** The keys, and the flag Escape on the picker clears. */
export interface ChannelKeys {
  /** Alt+↓ and Alt+↑ (Option on a Mac): the next or previous channel, in the order of the picker. */
  stepChannel(event: { readonly key?: string }): boolean;
  /** Ctrl+K (Cmd+K): the channel picker, open (question 11). See `channelPicked` for the way back. */
  pickChannel(): boolean;
  /**
   * A channel chosen from the picker Ctrl+K opened: the keys go back to the list, on the new channel.
   *
   * Without it the focus stayed on the picker, where every single key is the picker's own: j did
   * nothing, Esc had nothing to do, and the ↑ or ↓ meant for the next message chose another channel
   * (or, on a Mac, opened the picker again). Only after Ctrl+K: a picker reached by a click or by Tab
   * keeps the focus through a change, because on Windows and Linux ↑ and ↓ on a closed picker step it
   * one channel at a time, firing `change` at each step, and taking the focus at the first step would
   * stop a reader walking through the channels that way. Esc on a picker of the bar hands the focus to
   * the list however it was reached (`onEscape`).
   */
  channelPicked(): void;
  /** The picker was left by Esc: a channel chosen from it later is not from Ctrl+K. */
  forgetPick(): void;
}

export function createChannelKeys(deps: ChannelKeysDependencies): ChannelKeys {
  /**
   * Whether the channel picker was opened by Ctrl+K (Cmd+K) and no channel has been chosen from it
   * since (`createChannelPick`). See `channelPicked`.
   */
  const channelPickByKey = createChannelPick();

  return {
    stepChannel(event) {
      if (!deps.channelOnScreen()) return false;
      const picker = deps.picker();
      if (picker.hidden || picker.disabled) return false;
      // In the order shown, across service groups. `#355 provider-grouped-channels`.
      const order = deps.channelsInPickerOrder();
      const at = order.findIndex((channel) => String(channel.id) === String(deps.selectedChannelId()));
      const next = at < 0 ? null : order[at + (event.key === "ArrowDown" ? 1 : -1)];
      // At either end the list stops, rather than wrapping round to a channel at the other end.
      if (!next) return true;
      deps.chooseChannel(String(next.id));
      deps.changeChannel();
      deps.settleFocus();
      return true;
    },
    pickChannel() {
      if (!deps.channelOnScreen()) return false;
      const picker = deps.picker();
      if (picker.hidden || picker.disabled) return false;
      channelPickByKey.openedByKey();
      picker.focus();
      if (typeof picker.showPicker === "function") {
        try {
          picker.showPicker();
        } catch (_error) {
          // A browser that will not open it from a key still has it focused: Space or Alt+↓ opens it.
        }
      }
      return true;
    },
    channelPicked() {
      if (!channelPickByKey.byKey() || (document.activeElement as unknown) !== deps.picker()) return;
      channelPickByKey.forget();
      deps.settleFocus();
    },
    forgetPick: () => channelPickByKey.forget(),
  };
}

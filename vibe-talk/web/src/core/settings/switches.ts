// A choice kept on this device as one switch, on or off (`#624 voice-page-modularity`, moved from the legacy
// script into core/settings): whether it is on now, what the device keeps, and keeping it. Mark my own
// messages as read (`#549 own-read-stored`), Hide empty messages (`#386 hide-empty-messages`), Combine
// messages, Main channel among the cards (`#419 threads-view-main-messages`) and the fold of the card at the
// top of a thread (`#388 thread-header-collapse`) are each one. What a change draws, and when it is kept, are
// the page's: it calls `set`, `keep` and `apply` in the order each switch has always used.

import type { DeviceSetting, DeviceSettings } from "./device.ts";

/** What a switch asks of the page. */
export interface KeptSwitchDependencies {
  /** The device setting the switch is kept as. */
  readonly setting: DeviceSetting<boolean>;
  /** The device's settings, asked each time they are read or written. */
  deviceSettings(): DeviceSettings;
  /**
   * Whether it is on at first. Absent: as the device keeps it, read once as the switch is made, so a page
   * that makes it where it used to read the setting reads it at the same moment.
   */
  readonly initial?: boolean;
  /** Draw the switch's value; called by `apply`, after the value is set and before it is kept. */
  render?(on: boolean): void;
}

/** A switch kept on the device: its commands. */
export interface KeptSwitch {
  /** Whether it is on now. */
  on(): boolean;
  /** On or off now, as `on` says; nothing is drawn or kept. */
  set(on: boolean): void;
  /** Keep `on` on the device. A browser that refuses storage keeps nothing, and the switch holds for this session. */
  keep(on: boolean): void;
  /** On or off as `Boolean(on)` says: set, drawn (`render`), then kept. */
  apply(on: unknown): void;
  /** What the device keeps. Where storage is refused this is the setting's `refused` value, or the refusal thrown. */
  stored(): boolean;
}

/** On or off as `initial` says, or as the device keeps it; nothing is drawn or kept while it is made. */
export function createKeptSwitch(deps: KeptSwitchDependencies): KeptSwitch {
  let value = deps.initial === undefined ? deps.deviceSettings().get(deps.setting) : deps.initial;

  function keep(on: boolean): void {
    try {
      deps.deviceSettings().keep(deps.setting, on);
    } catch (_error) {
      // A browser that refuses storage still honours the choice for this session.
    }
  }

  return {
    on: () => value,
    set(on) {
      value = on;
    },
    keep,
    apply(on) {
      value = Boolean(on);
      if (deps.render) deps.render(value);
      keep(value);
    },
    stored: () => deps.deviceSettings().get(deps.setting),
  };
}

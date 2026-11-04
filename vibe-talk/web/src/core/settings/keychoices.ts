// Two of the reader's choices about keys (`#624 voice-page-modularity`, moved from the legacy script into
// core/settings): whether a bare Enter sends in the message boxes (`#225 enter-to-send`, Settings' Enter to
// send), and whether the single-key shortcuts act (`#224 keyboard-shortcuts`). Each is ON unless the
// reader turned it off, and kept on the device. What a key does, and saying the keys, are the page's.

import { DEVICE_SETTINGS } from "./device.ts";
import type { DeviceSettings } from "./device.ts";

/** What a key choice asks of the page. None is called while the owner is made. */
export interface KeyChoiceDependencies {
  /** The device's settings (`deviceSettings`). */
  deviceSettings(): DeviceSettings;
  /** Say the choice where it shows, as it now is. Called after it is kept. */
  render(): void;
}

/** Enter to send in force, and the device's copy of it. */
export interface EnterSends {
  /** Whether a bare Enter sends now. */
  current(): boolean;
  /** What was stored. ABSENT MEANS ON: the default is the behaviour, not merely the initial value. */
  stored(): boolean;
  /** Turn it on or off, keep it, and say it: a browser that refuses storage still honours it for this session. */
  apply(on: unknown): void;
}

/** On, until the page applies the device's choice. */
export function createEnterSends(deps: KeyChoiceDependencies): EnterSends {
  let enterSends = true;
  return {
    current: () => enterSends,
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.enterSends),
    apply(on) {
      enterSends = Boolean(on);
      try {
        deps.deviceSettings().keep(DEVICE_SETTINGS.enterSends, enterSends);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this session.
      }
      deps.render();
    },
  };
}

/** The single-key shortcuts' switch in force, and the device's copy of it. */
export interface SingleKeys {
  /** Whether the single-key shortcuts are on now. */
  current(): boolean;
  /** What was stored. ABSENT MEANS ON, as for Enter to send: the default is the behaviour. */
  stored(): boolean;
  /** Take what was stored as the switch in force, keeping and saying nothing: at load, before the keys are said. */
  restore(): void;
  /** Turn the shortcuts on or off, keep the choice, and say the keys as they now are. */
  apply(on: unknown): void;
}

/** On, until the page restores the device's choice. */
export function createSingleKeys(deps: KeyChoiceDependencies): SingleKeys {
  let singleKeys = true;
  // A browser refusing storage reads as on (`DEVICE_SETTINGS.singleKeys.refused`).
  const stored = () => deps.deviceSettings().get(DEVICE_SETTINGS.singleKeys);
  return {
    current: () => singleKeys,
    stored,
    restore() {
      singleKeys = stored();
    },
    apply(on) {
      singleKeys = Boolean(on);
      try {
        deps.deviceSettings().keep(DEVICE_SETTINGS.singleKeys, singleKeys);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this session.
      }
      deps.render();
    },
  };
}

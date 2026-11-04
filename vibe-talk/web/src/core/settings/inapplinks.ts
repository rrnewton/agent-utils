// Whether a link to a message opens it here (`#543 in-app-message-links`, Settings' Message links; `#624
// voice-page-modularity`, moved from the legacy script into core/settings): ON unless the reader has
// turned it off, kept on the device and synced. Deciding which links the switch takes, drawing them and
// jumping are the page's.

import { DEVICE_SETTINGS } from "./device.ts";
import type { DeviceSettings } from "./device.ts";

/** What the switch asks of the page. None is called while the owner is made. */
export interface InAppLinksDependencies {
  /** The device's settings (`deviceSettings`). */
  deviceSettings(): DeviceSettings;
  /** Draw the switch as `on`: Settings' Message links box. */
  render(on: boolean): void;
}

/** The switch in force, and the device's copy of it. */
export interface InAppLinks {
  /** Whether a link to a message opens it here now. */
  current(): boolean;
  /** What was stored. ABSENT MEANS ON: the default is the behaviour, not merely the initial value. */
  stored(): boolean;
  /** Turn it on or off, draw it, then keep it: a browser that refuses storage still honours it for this session. */
  apply(on: unknown): void;
}

/** On, until the page applies the device's choice. */
export function createInAppLinks(deps: InAppLinksDependencies): InAppLinks {
  let inAppLinks = true;
  return {
    current: () => inAppLinks,
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.inAppLinks),
    apply(on) {
      inAppLinks = Boolean(on);
      deps.render(inAppLinks);
      try {
        deps.deviceSettings().keep(DEVICE_SETTINGS.inAppLinks, inAppLinks);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this session.
      }
    },
  };
}

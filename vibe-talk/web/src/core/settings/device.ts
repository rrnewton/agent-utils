// The settings this device keeps, one named entry each: its key, what its kept text means, what
// is kept for a value, and what a refused read means (`#624 voice-page-modularity`, moved out of
// the legacy script a group at a time). A setting's reader and writer go through here, so what a
// setting IS — its key, its encoding, its default — is said once, and the page only draws it.

import type { StoragePort } from "../../ports/index.ts";

/** One setting kept on the device. */
export interface DeviceSetting<T> {
  readonly key: string;
  /** The value `raw` means: what is kept, or null for nothing kept. */
  read(raw: string | null): T;
  /** What to keep for `value`, or null to forget the key. */
  write(value: T): string | null;
  /**
   * What a read means when the device refuses storage. Absent: the refusal is thrown to the
   * caller, as some settings' readers always let it be.
   */
  readonly refused?: { readonly value: T };
}

/** On unless "0" is kept: ABSENT MEANS ON, the default being the behaviour, not merely the initial value. */
function onUnlessOff(key: string, refused?: boolean): DeviceSetting<boolean> {
  return {
    key,
    read: (raw) => raw !== "0",
    write: (on) => (on ? "1" : "0"),
    ...(refused === undefined ? {} : { refused: { value: refused } }),
  };
}

/** Off unless "1" is kept. */
function offUnlessOn(key: string, refused?: boolean): DeviceSetting<boolean> {
  return {
    key,
    read: (raw) => raw === "1",
    write: (on) => (on ? "1" : "0"),
    ...(refused === undefined ? {} : { refused: { value: refused } }),
  };
}

/** The device's settings, by name. */
export const DEVICE_SETTINGS = Object.freeze({
  /** `#543 in-app-message-links`. Whether a link to a message opens it here. */
  inAppLinks: onUnlessOff("vibe-talk.voice.in-app-links"),
  /** `#564 message-summary-toggle`. Summarize collapsed messages. */
  summarizeCollapsed: offUnlessOn("vibe-talk.voice.summarize-collapsed", false),
  /** `#620 link-url-chip`. Whether the Links view writes each link as its address: kept as "on", forgotten when off. */
  linksRaw: {
    key: "vibe-talk.voice.link-raw-urls",
    read: (raw: string | null) => raw === "on",
    write: (on: boolean) => (on ? "on" : null),
    refused: { value: false },
  } satisfies DeviceSetting<boolean>,
  /** `#340 composer-grow-box`. Whether the message box grows to its text. */
  composerGrow: offUnlessOn("vibe-talk.voice.composer-grow", false),
  /** Whether Enter in the reply box sends. */
  enterSends: onUnlessOff("vibe-talk.voice.enter-sends"),
  /** `#224 keyboard-shortcuts`. Whether single keys are shortcuts. */
  singleKeys: onUnlessOff("vibe-talk.voice.single-keys", true),
  /** `#242 rich-headers`. Whether a message's rich header is drawn as one. */
  richHeaders: onUnlessOff("vibe-talk.voice.rich-headers", true),
  /** `#549 own-read-stored`. Whether the reader's own messages are marked read. */
  markOwnRead: onUnlessOff("vibe-talk.voice.mark-own-read"),
  /** Whether messages with nothing to read are hidden. */
  hideEmptyMessages: onUnlessOff("vibe-talk.voice.hide-empty"),
  /** Whether messages are combined. */
  combineMessages: onUnlessOff("vibe-talk.voice.combine"),
});

/** The device's settings, kept in its storage. */
export interface DeviceSettings {
  /** `setting`'s value. A refused read is the setting's `refused` value, or thrown where it has none. */
  get<T>(setting: DeviceSetting<T>): T;
  /** Keep `value` for `setting`. Throws, or silently keeps nothing, where storage is refused. */
  keep<T>(setting: DeviceSetting<T>, value: T): void;
}

/** The device's settings in `storage`. */
export function createDeviceSettings(storage: StoragePort): DeviceSettings {
  return {
    get<T>(setting: DeviceSetting<T>): T {
      if (!setting.refused) return setting.read(storage.getItem(setting.key));
      try {
        return setting.read(storage.getItem(setting.key));
      } catch (_error) {
        return setting.refused.value;
      }
    },
    keep<T>(setting: DeviceSetting<T>, value: T): void {
      const raw = setting.write(value);
      if (raw === null) storage.removeItem(setting.key);
      else storage.setItem(setting.key, raw);
    },
  };
}

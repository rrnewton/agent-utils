// The settings this device keeps, one named entry each: its key, what its kept text means, what
// is kept for a value, and what a refused read means (`#624 voice-page-modularity`, moved out of
// the legacy script a group at a time). A setting's reader and writer go through here, so what a
// setting IS — its key, its encoding, its default — is said once, and the page only draws it.

import type { StoragePort } from "../../ports/index.ts";
import {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_MSG_SCALE,
  DEFAULT_READING_CH,
  clampCardMaxLines,
  clampCollapsedLines,
  clampMsgScale,
  clampReadSpeed,
  clampReadingWidth,
} from "./bounds.ts";

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

/** On only when "on" is kept; kept as "on" or "off". A refused read is off. */
function onOrOff(key: string): DeviceSetting<boolean> {
  return { key, read: (raw) => raw === "on", write: (on) => (on ? "on" : "off"), refused: { value: false } };
}

/** A voice that reads messages aloud: the voice agent's, the device's, or Wit.ai's. */
export type ReadAudioSource = "agent" | "device" | "wit";

/** An offset on screen, in CSS pixels. */
export interface SteerOffset {
  readonly x: number;
  readonly y: number;
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
  /**
   * `#580 open-in-chat-jump`. The view a sent card's Open in channel lands in: "flat", All, unless
   * the reader chose "main". ABSENT MEANS ALL, the owner's choice.
   */
  openInView: {
    key: "vibe-talk.voice.open-in-view",
    read: (raw: string | null): "flat" | "main" => (raw === "main" ? "main" : "flat"),
    write: (view: "flat" | "main") => (view === "main" ? "main" : "flat"),
  } satisfies DeviceSetting<"flat" | "main">,
  /** Which voice reads messages aloud, as chosen: null for no choice (or one this page does not know). */
  readAudioSource: {
    key: "vibe-talk.voice.read-audio-source",
    read: (raw: string | null): ReadAudioSource | null =>
      (raw === "agent" || raw === "device" || raw === "wit" ? raw : null),
    write: (source: ReadAudioSource | null) => source,
    refused: { value: null },
  } satisfies DeviceSetting<ReadAudioSource | null>,
  /** `#552 wit-tts-provider`. The Wit.ai voice chosen: "" for the configured one, kept as nothing. */
  witVoice: {
    key: "vibe-talk.voice.wit-voice",
    read: (raw: string | null) => raw || "",
    write: (name: string) => name || null,
    refused: { value: "" },
  } satisfies DeviceSetting<string>,
  /** Where the floating read-aloud controller was dragged to, as an offset from where it docks: null
   * for docked. A damaged record is docked. */
  readSteerOffset: {
    key: "vibe-talk.voice.read-steer-offset",
    read: (raw: string | null): SteerOffset | null => {
      const held = JSON.parse(raw || "null") as { x?: unknown; y?: unknown } | null;
      return held && Number.isFinite(held.x) && Number.isFinite(held.y)
        ? { x: held.x as number, y: held.y as number }
        : null;
    },
    write: (offset: SteerOffset | null) => (offset === null ? null : JSON.stringify(offset)),
    refused: { value: null },
  } satisfies DeviceSetting<SteerOffset | null>,
  /** `#481 slack-app-deep-link`. Where Open in Slack goes from here: the app, or a browser tab unless the app was chosen. */
  slackOpen: {
    key: "vibe-talk.voice.slack-open",
    read: (raw: string | null): "app" | "browser" => (raw === "app" ? "app" : "browser"),
    write: (where: "app" | "browser") => where,
    refused: { value: "browser" },
  } satisfies DeviceSetting<"app" | "browser">,
  /** The device's choice of Main channel among the thread list's cards: kept "on" or "off". */
  threadsMain: onOrOff("vibe-talk.voice.threads-main"),
  /** `#388 thread-header-collapse`. Whether the card at the top of the thread list is folded: kept "on" or "off". */
  threadCardFolded: onOrOff("vibe-talk.voice.thread-card-folded"),
  /** The reader's type size for message text, a percentage, clamped (`clampMsgScale`). */
  msgScale: {
    key: "vibe-talk.voice.msg-scale",
    read: (raw: string | null) => clampMsgScale(raw === null ? DEFAULT_MSG_SCALE : raw),
    write: (pct: number) => String(pct),
  } satisfies DeviceSetting<number>,
  /** The reading column's width in characters, clamped (`clampReadingWidth`). */
  readingWidth: {
    key: "vibe-talk.voice.width",
    read: (raw: string | null) => clampReadingWidth(raw === null ? DEFAULT_READING_CH : raw),
    write: (ch: number) => String(ch),
  } satisfies DeviceSetting<number>,
  /** `#425 collapsed-lines`. The lines a fold shows, clamped. Synced. */
  collapsedLines: {
    key: "vibe-talk.voice.collapsed-lines",
    read: (raw: string | null) => clampCollapsedLines(raw === null ? DEFAULT_COLLAPSED_LINES : raw),
    write: (n: number) => String(n),
  } satisfies DeviceSetting<number>,
  /** `#522 thread-card-max-lines`. K, clamped; a refused read is the default. Synced. */
  cardMaxLines: {
    key: "vibe-talk.voice.thread-card-max-lines",
    read: (raw: string | null) => clampCardMaxLines(raw === null ? DEFAULT_CARD_MAX_LINES : raw),
    write: (n: number) => String(n),
    refused: { value: DEFAULT_CARD_MAX_LINES },
  } satisfies DeviceSetting<number>,
  /** The reading pace, a percentage, or null (nothing kept) for however the agent speaks. Synced. */
  readSpeed: {
    key: "vibe-talk.voice.read-speed",
    read: (raw: string | null) => (raw === null ? null : clampReadSpeed(raw)),
    write: (pct: number | null) => (pct === null ? null : String(pct)),
  } satisfies DeviceSetting<number | null>,
  /** `#610 mobile-keyboard-compose`. Whether the keyboard layout debug readout is drawn: kept "1", forgotten when off. */
  keyboardDebug: {
    key: "vibe-talk.voice.keyboard-debug",
    read: (raw: string | null) => raw === "1",
    write: (on: boolean) => (on ? "1" : null),
    refused: { value: false },
  } satisfies DeviceSetting<boolean>,
});

/** The device's settings, kept in its storage. */
export interface DeviceSettings {
  /** `setting`'s value. A refused read is the setting's `refused` value, or thrown where it has none. */
  get<T>(setting: DeviceSetting<T>): T;
  /** Keep `value` for `setting`. Throws, or silently keeps nothing, where storage is refused. */
  keep<T>(setting: DeviceSetting<T>, value: T): void;
  /**
   * Whether the device now holds exactly what `value` keeps for `setting`: the read-back after a
   * keep, since a refusing browser may drop a write silently. Throws where storage is refused.
   */
  keptAs<T>(setting: DeviceSetting<T>, value: T): boolean;
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
    keptAs<T>(setting: DeviceSetting<T>, value: T): boolean {
      return storage.getItem(setting.key) === setting.write(value);
    },
  };
}

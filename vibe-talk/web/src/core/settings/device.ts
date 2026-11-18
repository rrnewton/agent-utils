// The settings this device keeps, one named entry each: its key, what its kept text means, what
// is kept for a value, and what a refused read means (`#624 voice-page-modularity`, moved out of
// the legacy script a group at a time). A setting's reader and writer go through here, so what a
// setting IS — its key, its encoding, its default — is said once, and the page only draws it.

import type { StoragePort } from "../../ports/index.ts";
import {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_MARK_READ_FIRST_PERCENT,
  DEFAULT_MARK_READ_LONG_WORDS,
  DEFAULT_QUOTE_READ_PERCENT,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_DISCUSS_DOCKED_HEIGHT,
  DEFAULT_DISCUSS_WIDTH,
  DEFAULT_MSG_SCALE,
  DEFAULT_READ_KEEP_ALIVE_MS,
  DEFAULT_READ_PREROLL_MS,
  DEFAULT_READING_CH,
  DEFAULT_TWO_COLUMN_SPLIT,
  DEFAULT_DATA_REACH_BACK,
  DEFAULT_RENDER_PREFETCH_PX,
  DEFAULT_RENDER_ROW_CAP,
  clampCardMaxLines,
  clampDataReachBack,
  clampMarkReadFirstPercent,
  clampMarkReadLongWords,
  clampQuoteReadPercent,
  clampCollapsedLines,
  clampDiscussDockedHeight,
  clampDiscussWidth,
  clampMsgScale,
  clampReadKeepAliveMs,
  clampReadPrerollMs,
  clampRenderPrefetchPx,
  clampRenderRowCap,
  clampReadSpeed,
  clampReadingWidth,
  clampTwoColumnSplit,
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

/** What `e` does after marking a message Done (`#1321 after-done-setting`): stay on it, go to the next message, or go to the next unread one. */
export type AfterDone = "stay" | "next" | "unread";

/** The Appearance choice (`#1845 vibe-talk-appearance-setting`): follow the system, or force light or dark. */
export type Appearance = "light" | "dark" | "system";

/** An offset on screen, in CSS pixels. */
export interface SteerOffset {
  readonly x: number;
  readonly y: number;
}

/** `#1821 vibe-talk-discuss-state-persists`. Where Discuss stands on this device: open or
 * closed, and the conversation it shows (a thread, or the channel in `view`). */
export interface DiscussPanelState {
  readonly open: boolean;
  readonly channelId: string;
  readonly threadId: string | null;
  readonly view: "flat" | "main";
  // `#1841`: the bound scope's own name, kept so a restored thread scope is
  // titled by the thread and never by the channel.
  readonly label?: string;
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
  /** `#880 voice-page-menu-placement`. Whether the channel's row of big reader buttons is hidden: a screen-layout
   * choice, kept on this device as `composerGrow` is. A refused read shows it. */
  readerHidden: offUnlessOn("vibe-talk.voice.reader-hidden", false),
  /** `#886 voice-page-autoplay`. Whether a read heard to its end goes on to the next unread message below it: kept on
   * this device, as the reader's row is. A refused read is off. */
  autoplay: offUnlessOn("vibe-talk.voice.autoplay", false),
  /** Whether Enter in the reply box sends. */
  enterSends: onUnlessOff("vibe-talk.voice.enter-sends"),
  /** `#1840 vibe-talk-draft-sync-across-devices`. Whether drafts auto-sync. */
  draftAutosync: onUnlessOff("vibe-talk.draft-autosync"),
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
  /**
   * `#1321 after-done-setting`. What `e` (and Mod+E) does after marking Done: "stay" on the
   * message unless the reader chose "next" (the next message below) or "unread" (the next
   * unread below). ABSENT MEANS STAY, the owner's choice. Synced.
   */
  afterDone: {
    key: "vibe-talk.voice.after-done",
    read: (raw: string | null): AfterDone => (raw === "next" || raw === "unread" ? raw : "stay"),
    write: (choice: AfterDone) => choice,
    refused: { value: "stay" },
  } satisfies DeviceSetting<AfterDone>,
  /**
   * `#1845 vibe-talk-appearance-setting`. The Appearance choice: "system" unless the reader
   * chose "light" or "dark". ABSENT MEANS SYSTEM, today's behaviour. Synced, so the choice
   * follows the owner across devices. A refused read is System.
   */
  appearance: {
    key: "vibe-talk.voice.appearance",
    read: (raw: string | null): Appearance => (raw === "light" || raw === "dark" ? raw : "system"),
    write: (choice: Appearance) => choice,
    refused: { value: "system" },
  } satisfies DeviceSetting<Appearance>,
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
  /** `#1186 two-column-splitter`. The two columns' split, a percentage, clamped (`clampTwoColumnSplit`). */
  twoColumnSplit: {
    key: "vibe-talk.voice.two-column-split",
    read: (raw: string | null) => clampTwoColumnSplit(raw === null ? DEFAULT_TWO_COLUMN_SPLIT : raw),
    write: (pct: number) => String(pct),
  } satisfies DeviceSetting<number>,
  /** `#1344 discuss-panel-resize`. The Discuss overlay's width on a desk, in CSS pixels, clamped
   * (`clampDiscussWidth`). Per device, as the split is: a width chosen on this screen says nothing
   * about another. A refused read is the wider default. */
  discussWidth: {
    key: "vibe-talk.voice.discuss-width",
    read: (raw: string | null) => clampDiscussWidth(raw === null ? DEFAULT_DISCUSS_WIDTH : raw),
    write: (px: number) => String(px),
    refused: { value: DEFAULT_DISCUSS_WIDTH },
  } satisfies DeviceSetting<number>,
  /** `#1552 discuss-docked-height-drag`. The docked pane's height, a viewport percentage,
   * clamped (`clampDiscussDockedHeight`). Per device, as the width is; a refused read is half. */
  discussDockedHeight: {
    key: "vibe-talk.voice.discuss-docked-height",
    read: (raw: string | null) => clampDiscussDockedHeight(raw === null ? DEFAULT_DISCUSS_DOCKED_HEIGHT : raw),
    write: (pct: number) => String(pct),
    refused: { value: DEFAULT_DISCUSS_DOCKED_HEIGHT },
  } satisfies DeviceSetting<number>,
  /** `#1344 discuss-panel-resize` (owner reply 417). Whether the phone's Discuss sheet stands at
   * full height: half until the reader maximizes it, and the choice sticks across close/reopen.
   * Per device; a refused read is half. */
  discussSheetFull: {
    key: "vibe-talk.voice.discuss-sheet-full",
    read: (raw: string | null) => raw === "1",
    write: (full: boolean) => (full ? "1" : null),
    refused: { value: false },
  } satisfies DeviceSetting<boolean>,
  /** `#1354 discuss-layout-modes`. How the Discuss panel stands on a desk: overlay, docked
   * beside the list, or maximized over the main area. Per device; anything unknown, and a
   * refused read, is the overlay it has always been. The stylesheet decides what a mode
   * means on a phone, where only the overlay sheet and its heights stand. */
  discussMode: {
    key: "vibe-talk.voice.discuss-mode",
    read: (raw: string | null): "overlay" | "docked" | "maximized" =>
      raw === "docked" || raw === "maximized" ? raw : "overlay",
    write: (mode: "overlay" | "docked" | "maximized") => mode,
    refused: { value: "overlay" },
  } satisfies DeviceSetting<"overlay" | "docked" | "maximized">,
  /** `#1497 discuss-fold-state-persist`. Which sections of the saved Discuss summary
   * stand open, for that summary only: one slot holding its record (scope plus the
   * summary's generation) and the open sections' keys, overwritten by every keep,
   * so a new summary forgets the old one's folds. Per device; a damaged entry, and
   * a refused read, is no entry. */
  discussFolds: {
    key: "vibe-talk.voice.discuss-folds",
    read: (raw: string | null): { record: string; open: string[] } | null => {
      try {
        const held = JSON.parse(raw || "null") as { record?: unknown; open?: unknown } | null;
        if (!held || typeof held.record !== "string" || !Array.isArray(held.open)) return null;
        return { record: held.record, open: held.open.filter((key): key is string => typeof key === "string") };
      } catch {
        return null;
      }
    },
    write: (state: { record: string; open: string[] } | null) => (state === null ? null : JSON.stringify(state)),
    refused: { value: null },
  } satisfies DeviceSetting<{ record: string; open: string[] } | null>,
  /** `#1842 vibe-talk-discuss-default-bound-since-last-summary`. The covered
   * ranges of each Discuss scope on this device (scope key to its ranges), so
   * a scope's default Summarize bound starts where its last summary ended,
   * across a refresh. A damaged entry reads as no coverage. */
  discussCoverage: {
    key: "vibe-talk.voice.discuss-coverage",
    read: (raw: string | null): Record<string, { ranges: { start: number; end: number }[] }> => {
      try {
        const held = JSON.parse(raw || "null") as Record<string, unknown> | null;
        if (!held || typeof held !== "object") return {};
        const out: Record<string, { ranges: { start: number; end: number }[] }> = {};
        for (const [key, value] of Object.entries(held)) {
          const ranges = (value as { ranges?: unknown })?.ranges;
          if (!Array.isArray(ranges)) continue;
          out[key] = { ranges: ranges.filter((r): r is { start: number; end: number } =>
            !!r && typeof (r as { start?: unknown }).start === "number" && typeof (r as { end?: unknown }).end === "number") };
        }
        return out;
      } catch {
        return {};
      }
    },
    write: (state: Record<string, { ranges: { start: number; end: number }[] }> | null) => (state === null ? null : JSON.stringify(state)),
    refused: { value: {} },
  } satisfies DeviceSetting<Record<string, { ranges: { start: number; end: number }[] }>>,
  /** `#1821 vibe-talk-discuss-state-persists`. Whether Discuss stands open and the
   * conversation it shows, so a refresh restores it (and a close stays closed). Per device;
   * a damaged entry, and a refused read, is no entry (closed). The docked size itself stays
   * in `discussWidth`, `discussDockedHeight`, `discussSheetFull` and `discussMode`. */
  discussPanel: {
    key: "vibe-talk.voice.discuss-panel",
    read: (raw: string | null): DiscussPanelState | null => {
      try {
        const held = JSON.parse(raw || "null") as { open?: unknown; channelId?: unknown; threadId?: unknown; view?: unknown; label?: unknown } | null;
        if (!held || typeof held.open !== "boolean" || typeof held.channelId !== "string"
          || !(held.threadId === null || typeof held.threadId === "string")
          || (held.view !== "flat" && held.view !== "main")) return null;
        return { open: held.open, channelId: held.channelId, threadId: held.threadId as string | null, view: held.view, ...(typeof held.label === "string" ? { label: held.label } : {}) };
      } catch {
        return null;
      }
    },
    write: (state: DiscussPanelState | null) => (state === null ? null : JSON.stringify(state)),
    refused: { value: null },
  } satisfies DeviceSetting<DiscussPanelState | null>,
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
  /** Mark read on reply itself: No means a sent reply never marks what it answers. On unless off. Synced. */
  markReadOnReply: onUnlessOff("vibe-talk.voice.mark-read-on-reply"),
  /** Hold a reply back when its quote covers under the share. Off unless on: the size rule is
   * the older heuristic, so a kept percent alone never turns it on. Synced. */
  markReadQuoteGuard: offUnlessOn("vibe-talk.voice.mark-read-quote-guard", false),
  /** Hold a reply back sent in Fold read. On unless off. Synced. */
  markReadFoldGuard: onUnlessOff("vibe-talk.voice.mark-read-fold-guard"),
  /** Whether an unquoted reply marks what it answers. On unless off. Synced. */
  markReadNonquote: onUnlessOff("vibe-talk.voice.mark-read-nonquote"),
  /** Hold a quoted reply back when its quote ends before the final paragraph. On unless off. Synced. */
  markReadFinalGuard: onUnlessOff("vibe-talk.voice.mark-read-final-guard"),
  /** Hold a quoted reply back when its quote sits in the first share of a long message. On unless off. Synced. */
  markReadFirstGuard: onUnlessOff("vibe-talk.voice.mark-read-first-guard"),
  /** The first share, in percent, for the long-message guard. Synced. */
  markReadFirstPercent: {
    key: "vibe-talk.voice.mark-read-first-percent",
    read: (raw: string | null) => clampMarkReadFirstPercent(raw === null ? DEFAULT_MARK_READ_FIRST_PERCENT : raw),
    write: (pct: number) => String(pct),
    refused: { value: DEFAULT_MARK_READ_FIRST_PERCENT },
  } satisfies DeviceSetting<number>,
  /** How many words make a message long, for the first-share guard. Synced. */
  markReadLongWords: {
    key: "vibe-talk.voice.mark-read-long-words",
    read: (raw: string | null) => clampMarkReadLongWords(raw === null ? DEFAULT_MARK_READ_LONG_WORDS : raw),
    write: (n: number) => String(n),
    refused: { value: DEFAULT_MARK_READ_LONG_WORDS },
  } satisfies DeviceSetting<number>,
  /** `#728 vt-quote-read-threshold`. How much a reply's quote must cover to mark read, clamped. Synced. */
  quoteReadPercent: {
    key: "vibe-talk.voice.quote-read-percent",
    read: (raw: string | null) => clampQuoteReadPercent(raw === null ? DEFAULT_QUOTE_READ_PERCENT : raw),
    write: (pct: number) => String(pct),
    refused: { value: DEFAULT_QUOTE_READ_PERCENT },
  } satisfies DeviceSetting<number>,
  /** `#1800 vibe-talk-read-aloud-chunk-start-clipping`. The silence before a cold chunk, in
   * milliseconds, clamped (`clampReadPrerollMs`); a refused read is the 300 ms default. */
  readPrerollMs: {
    key: "vibe-talk.voice.read-preroll-ms",
    read: (raw: string | null) => clampReadPrerollMs(raw === null ? DEFAULT_READ_PREROLL_MS : raw),
    write: (ms: number) => String(clampReadPrerollMs(ms)),
    refused: { value: DEFAULT_READ_PREROLL_MS },
  } satisfies DeviceSetting<number>,
  /** `#1800 vibe-talk-read-aloud-chunk-start-clipping`. How long the output stays alive after
   * a read, in milliseconds, clamped (`clampReadKeepAliveMs`); a refused read is 25 s. */
  readKeepAliveMs: {
    key: "vibe-talk.voice.read-keep-alive-ms",
    read: (raw: string | null) => clampReadKeepAliveMs(raw === null ? DEFAULT_READ_KEEP_ALIVE_MS : raw),
    write: (ms: number) => String(clampReadKeepAliveMs(ms)),
    refused: { value: DEFAULT_READ_KEEP_ALIVE_MS },
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
  /** `#1144 scrollback-typing-jump`. Whether the lists' scrolls are traced, for the Send beside it
   * to post to the server's log: kept "1", forgotten when off. */
  scrollTrace: {
    key: "vibe-talk.voice.scroll-trace",
    read: (raw: string | null) => raw === "1",
    write: (on: boolean) => (on ? "1" : null),
    refused: { value: false },
  } satisfies DeviceSetting<boolean>,
  /**
   * `#892 voice-page-optimistic-done`. Whether each key's time to its paint is said on the console
   * (web/src/view/shell/keytiming.ts): kept "1", forgotten when off. Set from the console; no control.
   */
  keyTiming: {
    key: "vibe-talk.voice.key-timing",
    read: (raw: string | null) => raw === "1",
    write: (on: boolean) => (on ? "1" : null),
    refused: { value: false },
  } satisfies DeviceSetting<boolean>,
  /**
   * `#1846 vibe-talk-windowed-rendering`. Whether the channel/thread lists mount only the
   * viewport plus a prefetch margin. OFF unless on; default off at S0 (see settings/windowed.ts).
   * The Settings help for this switch states the browser find-in-page limitation (Q3).
   */
  windowedLists: offUnlessOn("vibe-talk.voice.windowed-lists", false),
  /** `#1846`. Messages held as DATA for local search/filtering; 0 = per-device default. */
  dataReachBack: {
    key: "vibe-talk.voice.data-reach-back",
    read: (raw: string | null) => clampDataReachBack(raw === null ? DEFAULT_DATA_REACH_BACK : raw),
    write: (n: number) => String(clampDataReachBack(n)),
    refused: { value: DEFAULT_DATA_REACH_BACK },
  } satisfies DeviceSetting<number>,
  /** `#1846`. Render prefetch margin in px; 0 = per-device default (1200 phone / 1600 desktop). */
  renderPrefetchPx: {
    key: "vibe-talk.voice.render-prefetch-px",
    read: (raw: string | null) => clampRenderPrefetchPx(raw === null ? DEFAULT_RENDER_PREFETCH_PX : raw),
    write: (n: number) => String(clampRenderPrefetchPx(n)),
    refused: { value: DEFAULT_RENDER_PREFETCH_PX },
  } satisfies DeviceSetting<number>,
  /** `#1846`. Mounted-row cap per list; 0 = per-device default (60 phone / 80 desktop). */
  renderRowCap: {
    key: "vibe-talk.voice.render-row-cap",
    read: (raw: string | null) => clampRenderRowCap(raw === null ? DEFAULT_RENDER_ROW_CAP : raw),
    write: (n: number) => String(clampRenderRowCap(n)),
    refused: { value: DEFAULT_RENDER_ROW_CAP },
  } satisfies DeviceSetting<number>,
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

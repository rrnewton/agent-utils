// The two independent knobs of `#1846 vibe-talk-windowed-rendering` (owner thread 186), kept
// apart on purpose: (1) DATA reach-back — how many messages are fetched and held as data in the
// cache for fast local search/filtering; deep on desktop, shallower on a phone. (2) RENDER
// window — how many DOM rows are mounted (design sec.2); small everywhere. Each has per-device
// defaults below, overridable in Settings (core/settings/device.ts: windowedLists, dataReachBack,
// renderPrefetchPx, renderRowCap; 0 there means "the default here"). If server-side search gets
// fast, the desktop data default can shrink without touching the render window at all.

import { DEVICE_SETTINGS } from "./device.ts";
import type { DeviceSettings } from "./device.ts";

/** Approved starting values (coordb Q1, 2026-11-18): prefetch beside the viewport, in px. */
export const RENDER_PREFETCH_PHONE_PX = 1200;
export const RENDER_PREFETCH_DESKTOP_PX = 1600;
/** Approved hard caps (coordb Q2): mounted rows per list, spacers excluded. */
export const RENDER_CAP_PHONE = 60;
export const RENDER_CAP_DESKTOP = 80;
/** One screenful of scrolling mounts at most this many new rows (budget B3/M). */
export const SCROLL_MOUNT_BUDGET = 20;

/** Data reach-back defaults (owner thread 186): thousands on a desk, hundreds on a phone. */
export const DATA_REACH_PHONE = 300;
export const DATA_REACH_DESKTOP = 5000;

/**
 * Planning figure for the data cache's memory cost (see incremental.ts estimateMessageBytes,
 * which measures rather than assumes): a text message's wire form runs ~1.5 KB, so the desktop
 * default of 5000 costs ~7.5 MB and the phone default of 300 costs ~0.45 MB. Media/summary-heavy
 * rows cost more; the byte caps below are the real bound and the count is the simple one.
 */
export const DATA_BYTES_PER_MESSAGE = 1536;
export const DATA_MAX_BYTES_PHONE = 4 * 1024 * 1024;
export const DATA_MAX_BYTES_DESKTOP = 16 * 1024 * 1024;

/**
 * Settings help text for the windowed-lists switch (coordb Q3): the native find limitation is
 * stated where the owner turns the feature on, not discovered later.
 */
export const WINDOWED_LISTS_HELP =
  "Show only the messages near the screen (faster, lighter). " +
  "Browser find (Ctrl/Cmd+F) reaches only shown messages; use the in-page search to find older ones.";

/** The windowed configuration in force on one device. */
export interface WindowedConfig {
  /** Whether windowing is on (S0: default off; the setting is the kill switch). */
  readonly windowed: boolean;
  /** Prefetch margin beside the viewport, px, each side. */
  readonly prefetchPx: number;
  /** Hard cap on mounted rows per list. */
  readonly rowCap: number;
  /** Messages held as data for local search/filtering. */
  readonly dataReach: number;
  /** Byte bound for that data (oldest dropped first past it). */
  readonly dataMaxBytes: number;
}

/** The configuration `settings` gives on a phone (`phone`) or a desk, defaults per form factor. */
export function windowedConfig(settings: DeviceSettings, phone: boolean): WindowedConfig {
  const reach = settings.get(DEVICE_SETTINGS.dataReachBack);
  const prefetch = settings.get(DEVICE_SETTINGS.renderPrefetchPx);
  const cap = settings.get(DEVICE_SETTINGS.renderRowCap);
  return {
    // S1 is ready (Settings switch + explicit-Off key) but the default stays Off until the
    // browser suite gains windowed variants; flipping it here reddens every phone test that
    // expects full mounting. One-line flip: `|| (phone && !settings.get(...windowedListsOff))`.
    windowed: settings.get(DEVICE_SETTINGS.windowedLists),
    prefetchPx: prefetch > 0 ? prefetch : phone ? RENDER_PREFETCH_PHONE_PX : RENDER_PREFETCH_DESKTOP_PX,
    rowCap: cap > 0 ? cap : phone ? RENDER_CAP_PHONE : RENDER_CAP_DESKTOP,
    dataReach: reach > 0 ? reach : phone ? DATA_REACH_PHONE : DATA_REACH_DESKTOP,
    dataMaxBytes: phone ? DATA_MAX_BYTES_PHONE : DATA_MAX_BYTES_DESKTOP,
  };
}

/**
 * How many oldest cached messages must go to satisfy `config` (count and bytes together),
 * given `count` held messages costing `bytes`. 0 = keep all. Pure: the caller drops them.
 */
export function reachBackExcess(count: number, bytes: number, config: WindowedConfig): number {
  let excess = Math.max(0, count - config.dataReach);
  if (bytes > config.dataMaxBytes && count > excess) {
    // Estimate the byte-driven excess from the average; callers re-check after dropping.
    const average = count > 0 ? bytes / count : DATA_BYTES_PER_MESSAGE;
    const byBytes = Math.ceil((bytes - config.dataMaxBytes) / Math.max(1, average));
    excess = Math.max(excess, Math.min(count, byBytes));
  }
  return excess;
}

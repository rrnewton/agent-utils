// How many messages the page holds as DATA (the reach-back knob of `#1846`, owner thread 186):
// deep on a desk, shallower on a phone, overridable in Settings (core/settings/device.ts:
// dataReachBack; 0 there means "the default here").
//
// ONE LOADING STRATEGY (`#2426 vibe-talk-sparse-loading-program`, owner 2026-11-27: "we're
// looking for the new smart/incremental loading to become DEFAULT not maintaining multiple
// strategies"). The separate render window of `#1846` (mount only the viewport plus a margin,
// behind the windowed-lists setting) is retired: every held row is mounted, and what bounds the
// list is how much is held. The residency layer (`#2427`, `#2426`) enforces the held budget;
// the windowed-lists keys a device may still hold are read nowhere.

import { DEVICE_SETTINGS } from "./device.ts";
import type { DeviceSettings } from "./device.ts";

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

/** The data budget in force on one device. */
export interface WindowedConfig {
  /** Messages held as data for local search/filtering. */
  readonly dataReach: number;
  /** Byte bound for that data (oldest dropped first past it). */
  readonly dataMaxBytes: number;
}

/** The data budget `settings` gives on a phone (`phone`) or a desk, defaults per form factor. */
export function windowedConfig(settings: DeviceSettings, phone: boolean): WindowedConfig {
  const reach = settings.get(DEVICE_SETTINGS.dataReachBack);
  return {
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

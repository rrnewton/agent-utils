// How much of each channel view stays resident on this device (`#2427 vibe-talk-windowed-default-on`,
// the interim held-rows cap; the residency layer of `#2426`, see core/channels/residency.ts).
//
// Measured on 2026-11-27 with windowing off, at desk 1280x1322 and phone 412x915 (the owner's page
// held about 650 rows, 107,000 px): every freeze the page has grows about linearly with the rows it
// holds — the load draw, each arrival's redraw, a selection change, a keystroke's handler — so the
// newest window is small. The owner asked for loading that is "incremental and small".

import { DEVICE_SETTINGS } from "./device.ts";
import type { DeviceSettings } from "./device.ts";

/** Rows of the newest window a channel view keeps, on a phone and on a desk. */
export const RESIDENT_ROWS_PHONE = 150;
export const RESIDENT_ROWS_DESKTOP = 200;
/** Byte bounds for what is held (unused in v0: the row count is the bound). */
export const RESIDENT_MAX_BYTES_PHONE = 4 * 1024 * 1024;
export const RESIDENT_MAX_BYTES_DESKTOP = 16 * 1024 * 1024;

/** What residency keeps of one view. */
export interface ResidencyBudget {
  /** Rows of the view's newest window, always kept. */
  readonly newestRows: number;
  /** Rows of one island far back from the newest (`#2426` islands, read there; the same per device). */
  readonly islandRows: number;
  /** Bytes the held rows may cost. */
  readonly maxBytes: number;
}

/** The budget `settings` gives on a phone (`phone`) or a desk: the device's override, else the default. */
export function residencyBudget(settings: DeviceSettings, phone: boolean): ResidencyBudget {
  const rows = settings.get(DEVICE_SETTINGS.residentRows);
  const newestRows = rows > 0 ? rows : phone ? RESIDENT_ROWS_PHONE : RESIDENT_ROWS_DESKTOP;
  return {
    newestRows,
    islandRows: newestRows,
    maxBytes: phone ? RESIDENT_MAX_BYTES_PHONE : RESIDENT_MAX_BYTES_DESKTOP,
  };
}

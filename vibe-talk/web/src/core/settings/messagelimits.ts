// The message limits in force on this page (`#624 voice-page-modularity`, moved from the legacy script
// into core/settings): the lines a fold shows (`#425 collapsed-lines`), the lines a thread card shows of
// its first message, K (`#522 thread-card-max-lines`), and the share of a message a reply's quote must
// cover to mark it read (`#728 vt-quote-read-threshold`). Each is the device's setting, read and applied
// clamped (./bounds.ts). Drawing a limit, and what the page draws again by it, are the page's, handed in.

import {
  DEFAULT_CARD_MAX_LINES,
  DEFAULT_COLLAPSED_LINES,
  DEFAULT_MARK_READ_FIRST_PERCENT,
  DEFAULT_MARK_READ_LONG_WORDS,
  DEFAULT_QUOTE_READ_PERCENT,
  clampCardMaxLines,
  clampCollapsedLines,
  clampMarkReadFirstPercent,
  clampMarkReadLongWords,
  clampQuoteReadPercent,
} from "./bounds.ts";
import { DEVICE_SETTINGS } from "./device.ts";
import type { DeviceSettings } from "./device.ts";

/** What a message limit asks of the page. None is called while the owner is made. */
export interface MessageLimitDependencies {
  /** The device's settings (`deviceSettings`). */
  deviceSettings(): DeviceSettings;
  /** Draw `n` as the limit in force: Settings' slider, and anything the stylesheet reads of it. */
  render(n: number): void;
}

/** The collapsed-lines count in force, and the device's copy of it. */
export interface CollapsedLines {
  /** The count in force. */
  current(): number;
  /** What was stored, clamped. A missing or corrupt entry is the default, never an error. */
  stored(): number;
  /**
   * Put a lines count on the page, and return the one actually applied, which is the clamped one. One
   * property on the root reaches every list, as the reading width's does; the value is ALSO the root's
   * `data-collapsed-lines`, because at 0 the stylesheet hides the folded body outright — a clamp of 0
   * reads as no clamp in a browser and would show every line (web/voice.css). Keeps nothing.
   */
  apply(value: unknown): number;
  /** Store `n`, and READ IT BACK: private browsing accepts `setItem` and keeps nothing. */
  persist(n: number): boolean;
}

/** The default count in force, until the page applies the device's. */
export function createCollapsedLines(deps: MessageLimitDependencies): CollapsedLines {
  let collapsedLines = DEFAULT_COLLAPSED_LINES;
  return {
    current: () => collapsedLines,
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.collapsedLines),
    apply(value) {
      collapsedLines = clampCollapsedLines(value);
      deps.render(collapsedLines);
      return collapsedLines;
    },
    persist(n) {
      try {
        deps.deviceSettings().keep(DEVICE_SETTINGS.collapsedLines, n);
      } catch (_error) {
        return false;
      }
      return deps.deviceSettings().keptAs(DEVICE_SETTINGS.collapsedLines, n);
    },
  };
}

/** What K asks of the page: a limit's, and the cards drawn again when it changes. */
export interface CardMaxLinesDependencies extends MessageLimitDependencies {
  /** Draw every card list again by the K now in force. Called after it is in force and drawn. */
  redraw(): void;
}

/** K in force, and the device's copy of it. */
export interface CardMaxLines {
  /** K in force. */
  current(): number;
  /** What was stored, clamped. A missing or corrupt entry is the default, never an error; so is a refused read. */
  stored(): number;
  /**
   * Put K on the page, and return the one applied: the slider says it, and where `redraw` (by default)
   * and K changed, every card list is drawn again by it. At load there is nothing to draw again yet.
   * Keeps nothing.
   */
  apply(value: unknown, redraw?: boolean): number;
  /** Store `n`, and read it back: private browsing accepts `setItem` and keeps nothing. */
  persist(n: number): boolean;
}

/** The default K in force, until the page applies the device's. */
export function createCardMaxLines(deps: CardMaxLinesDependencies): CardMaxLines {
  let cardMaxLines = DEFAULT_CARD_MAX_LINES;
  return {
    current: () => cardMaxLines,
    // A browser refusing storage reads as the default (`DEVICE_SETTINGS.cardMaxLines.refused`).
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.cardMaxLines),
    apply(value, redraw = true) {
      const n = clampCardMaxLines(value);
      const changed = redraw && n !== cardMaxLines;
      cardMaxLines = n;
      deps.render(n);
      if (changed) deps.redraw();
      return n;
    },
    persist(n) {
      try {
        deps.deviceSettings().keep(DEVICE_SETTINGS.cardMaxLines, n);
      } catch (_error) {
        return false;
      }
      return deps.deviceSettings().keptAs(DEVICE_SETTINGS.cardMaxLines, n);
    },
  };
}

/**
 * The share in force, a percentage, and the device's copy of it. Keeping a changed share, and saying
 * whether it was kept, are Settings' (web/src/view/settings `mountMessagePreferences`).
 */
export interface QuoteReadPercent {
  /** The share in force. */
  current(): number;
  /** What was stored, clamped. A missing, corrupt or refused entry is the default. */
  stored(): number;
  /** Put the share on the page, and return the one applied: the slider says it. Keeps nothing. */
  apply(value: unknown): number;
}

/** The default share in force, until the page applies the device's. */
export function createQuoteReadPercent(deps: MessageLimitDependencies): QuoteReadPercent {
  let quoteReadPercent = DEFAULT_QUOTE_READ_PERCENT;
  return {
    current: () => quoteReadPercent,
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.quoteReadPercent),
    apply(value) {
      const n = clampQuoteReadPercent(value);
      quoteReadPercent = n;
      deps.render(n);
      return n;
    },
  };
}

/** A Mark read on reply number in force, and the device's copy of it, as `QuoteReadPercent` is. */
export interface MarkReadNumber {
  current(): number;
  stored(): number;
  apply(value: unknown): number;
}

/** The first-share percentage in force (50 until the reader says otherwise). */
export function createMarkReadFirstPercent(deps: MessageLimitDependencies): MarkReadNumber {
  let value = DEFAULT_MARK_READ_FIRST_PERCENT;
  return {
    current: () => value,
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.markReadFirstPercent),
    apply(next) {
      const n = clampMarkReadFirstPercent(next);
      value = n;
      deps.render(n);
      return n;
    },
  };
}

/** The long-message word count in force (0, every message, until the reader says otherwise). */
export function createMarkReadLongWords(deps: MessageLimitDependencies): MarkReadNumber {
  let value = DEFAULT_MARK_READ_LONG_WORDS;
  return {
    current: () => value,
    stored: () => deps.deviceSettings().get(DEVICE_SETTINGS.markReadLongWords),
    apply(next) {
      const n = clampMarkReadLongWords(next);
      value = n;
      deps.render(n);
      return n;
    },
  };
}

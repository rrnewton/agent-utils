// How long a key takes to show (`#892 voice-page-optimistic-done`): from the keydown to the end of the page's
// own work on it, and to the frame painted after it. Off unless asked for, and then said on the console and in
// the browser's performance timeline, a key at a time. The owner asked why Shift+E lagged; this is how the
// answer is measured, before and after a change, on the device where it lags.
//
// Asked for by `?key-timing` in the page's address, or kept on the device with
// `localStorage.setItem("vibe-talk.voice.key-timing", "1")` in the console (and taken away with `removeItem`).

/** How many keys' timings are kept for `recent`, newest last. */
const KEY_TIMINGS_KEPT = 100;

/** The keys that are only ever half of a chord. */
const MODIFIERS = ["Shift", "Control", "Alt", "Meta", "AltGraph", "CapsLock"];

/** A key the timing reads: what it is called and when the browser says it happened. */
export interface TimedKey {
  readonly key?: string;
  readonly shiftKey?: boolean;
  readonly ctrlKey?: boolean;
  readonly metaKey?: boolean;
  readonly altKey?: boolean;
  /** When the browser took the key, on `now`'s clock (an event's `timeStamp`). */
  readonly timeStamp?: number;
}

/** One key's timing, in milliseconds from the keydown. */
export interface KeyTimingEntry {
  /** The chord, "Shift+E". */
  readonly chord: string;
  /** To the end of the page's own handling of the key. */
  readonly handler: number;
  /** To the first moment after the next frame was painted; null until then. */
  paint: number | null;
}

export interface KeyTimingDependencies {
  /** Whether timing is asked for now. Read at each key, so turning it on needs no reload. */
  enabled(): boolean;
  /** The clock of the keys' `timeStamp`, in ms (`performance.now`). */
  now(): number;
  /** Run `act` once the next frame has been painted: a timer started from an animation frame. */
  afterPaint(act: () => void): void;
  /** Say one key's timing: a console line and a mark on the browser's timeline. */
  report(entry: KeyTimingEntry, start: number): void;
}

/** The timing's commands. */
export interface KeyTiming {
  /** Run `handle`, the page's handling of `event`, timing it when asked for. */
  around(event: TimedKey | null | undefined, handle: () => void): void;
  /** The last keys timed, oldest first. */
  recent(): readonly KeyTimingEntry[];
}

/** The chord `event` is, as the key hints write one: "Shift+E", "Ctrl+K", "e". */
export function chordOf(event: TimedKey): string {
  const key = String(event.key || "?");
  // Shift is said where the key's name does not say it already: "Shift+E" and "Shift+ArrowDown", but "?".
  const shift = event.shiftKey && (key.length > 1 || /^[A-Z]$/.test(key));
  const held = [event.ctrlKey && "Ctrl", event.metaKey && "Cmd", event.altKey && "Alt", shift && "Shift"];
  return [...held.filter(Boolean), key].join("+");
}

/** One line for the console: "Shift+E: handler 3.1 ms, paint 12.4 ms". */
export function keyTimingLine(entry: KeyTimingEntry): string {
  const paint = entry.paint === null ? "not painted" : `paint ${entry.paint.toFixed(1)} ms`;
  return `${entry.chord}: handler ${entry.handler.toFixed(1)} ms, ${paint}`;
}

export function createKeyTiming(deps: KeyTimingDependencies): KeyTiming {
  const kept: KeyTimingEntry[] = [];
  return {
    around(event, handle) {
      // A modifier pressed on its own is the first half of a chord, not a key that does anything.
      if (!event || MODIFIERS.includes(String(event.key)) || !deps.enabled()) {
        handle();
        return;
      }
      // From when the browser took the key where it says, so time queued before the handler counts.
      const start = typeof event.timeStamp === "number" && event.timeStamp > 0 ? event.timeStamp : deps.now();
      try {
        handle();
      } finally {
        const entry: KeyTimingEntry = { chord: chordOf(event), handler: deps.now() - start, paint: null };
        kept.push(entry);
        if (kept.length > KEY_TIMINGS_KEPT) kept.shift();
        deps.afterPaint(() => {
          entry.paint = deps.now() - start;
          deps.report(entry, start);
        });
      }
    },
    recent: () => kept.slice(),
  };
}

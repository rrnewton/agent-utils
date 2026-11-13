// The read modes: Show read, Fold read, Hide read (`#221 read-modes`, `#432 read-mode-unify`, `#457
// settings-server-side`; `#624 voice-page-modularity`, moved from the legacy script into present/rows).
// The one owner of the mode the list is in, and of how a mode is named, said and kept. Why the modes
// are three, in this order, with Show read the default, is told where the legacy script describes
// them; what each mode does to the list is the page's.

import type { StoragePort } from "../../ports/index.ts";

/** The read modes, in the order a tap on the button moves through them. */
const READ_MODES: readonly string[] = Object.freeze(["show", "fold", "hide"]);

/** What the button calls each mode: the mode the list is in, not the act a tap performs. */
const READ_MODE_LABELS: Readonly<Record<string, string>> = Object.freeze({ show: "Show read", fold: "Fold read", hide: "Hide read" });

/**
 * The first word of each name, in a span of its own so that a desk's dock row can stand the name in
 * two short lines: `#218 desktop-dock` fitted that row to "Hide read" with nothing to spare, and
 * "Collapse read" (now Fold read) on one line beside its icon would push the dock onto a second row. Never
 * drawn alone — a lone "Hide" or "Fold" reads as the act a tap performs. See web/voice.css.
 */
const READ_MODE_VERBS: Readonly<Record<string, string>> = Object.freeze({ show: "Show", fold: "Fold", hide: "Hide" });

/** What h says on the status line of each mode it moves to (view/shell `readModeFromKeyboard`). `#432 read-mode-unify`. */
const READ_MODE_SAID: Readonly<Record<string, string>> = Object.freeze({
  show: "Show read: everything shown.",
  fold: "Fold read: read runs folded.",
  hide: "Hide read: read ones hidden.",
});

/** The sentence under the pointer for each mode: what it does, and where a tap goes next. */
const READ_MODE_TITLES: Readonly<Record<string, string>> = Object.freeze({
  show: "Read messages are shown, greyed. Tap to fold each run of them into one line.",
  fold: "Each run of read messages, or of read threads, is one line; tap a line to open it. Tap to hide read messages.",
  hide: "Read messages are hidden. Tap to show them again.",
});

/**
 * The read mode's own kept copy, for the server (`#457 settings-server-side`). The mode itself
 * lives in the UI-state record with the view it belongs to; this mirror is written wherever the
 * mode changes, so the settings sync can carry the mode — a messaging choice — without carrying
 * the view record (this window's place in a channel, which is navigation, not a setting).
 */
const READ_MODE_KEY = "vibe-talk.voice.read-mode";

/** The mode a kept UI-state record says. `#432 read-mode-unify`: Fold read was "Collapse read", and a record kept then still means it. */
function savedReadMode(saved: { readonly read?: unknown; readonly todo?: unknown }): string {
  if (saved.read === "collapse") return "fold";
  return READ_MODES.includes(saved.read as string) ? (saved.read as string) : saved.todo ? "hide" : "show";
}

/** The mode after `mode`, round the cycle. */
const nextReadMode = (mode: string): string => READ_MODES[(READ_MODES.indexOf(mode) + 1) % READ_MODES.length] as string;

/** What the read mode asks of the page: the device's storage, for the mode's kept copy. */
export interface ReadModeDependencies {
  storage(): StoragePort;
}

/** The read mode's owner. */
export interface ReadMode {
  /** The mode the list is in: one of `READ_MODES`. */
  mode(): string;
  /** Whether the list hides what is read (Hide read). */
  hiding(): boolean;
  /** Put the list in `mode` (the page has checked it is one of `READ_MODES`). */
  set(mode: string): void;
  /** Keep the mode's own copy (`READ_MODE_KEY`) on the device, if the device will. */
  keep(): void;
}

/** Show read, until a kept record or the reader says otherwise. */
export function createReadMode(deps: ReadModeDependencies): ReadMode {
  let readMode = "show";
  let todoMode = false;
  return {
    mode: () => readMode,
    hiding: () => todoMode,
    set(mode) {
      readMode = mode;
      todoMode = mode === "hide";
    },
    keep() {
      try {
        deps.storage().setItem(READ_MODE_KEY, readMode);
      } catch (_error) {
        // The UI-state record is the restore; this copy is only the sync's.
      }
    },
  };
}

// What the page and the tests use of it.
export { READ_MODES, READ_MODE_KEY, READ_MODE_LABELS, READ_MODE_SAID, READ_MODE_TITLES, READ_MODE_VERBS, nextReadMode, savedReadMode };

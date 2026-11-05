// Saying the keys (`#224 keyboard-shortcuts`, `#295 key-hints`, `#281 view-keys`; `#624 voice-page-modularity`,
// moved from the legacy script into view/shell for the plan's step 17): every place a reader looks for the keys,
// said from the keymap in the words on their own keyboard. The keymap itself, and when the keys are said again,
// are the page's.

import { typesCharacter } from "./chords.ts";
import type { KeyChord } from "./chords.ts";
import type { RowShortcuts } from "./rowshortcuts.ts";

/** A binding of the keymap, as far as saying it reads it. */
export interface SaidBinding {
  /** Which part of the list of keys it is in. */
  readonly group?: string;
  readonly chords: readonly KeyChord[];
  /** What it does, as the list of keys says it: asked each time the list is drawn where it names a chord. */
  readonly does: string | (() => string);
  /** Ids of the buttons of the page this key presses, for `aria-keyshortcuts`. */
  readonly controls?: readonly string[];
  /** Classes of the buttons of a row this key presses, the same. */
  readonly rowControls?: readonly string[];
}

/** The names this list gives the named keys. */
export const KEY_WORDS: Readonly<Record<string, string>> = {
  Escape: "Esc", " ": "Space", PageUp: "PgUp", PageDown: "PgDn", ArrowDown: "↓", ArrowUp: "↑",
  ArrowLeft: "←", ArrowRight: "→",
};

/**
 * One chord, in the words on the keyboard of the reader: "Ctrl+S" or "Cmd+S", "Option+↓", "Shift+E", "j".
 * `mod` is the name of the reader's Ctrl or Cmd key; a keyboard that says Cmd says Option for Alt.
 */
export function chordWords(chord: KeyChord, mod: string): string {
  const parts: string[] = [];
  if (chord.mod) parts.push(mod);
  if (chord.alt) parts.push(mod === "Cmd" ? "Option" : "Alt");
  if (chord.shift) parts.push("Shift");
  const letter = /^[a-z]$/.test(chord.key);
  parts.push(KEY_WORDS[chord.key] || (letter && (chord.shift || chord.mod) ? chord.key.toUpperCase() : chord.key));
  return parts.join("+");
}

/** One chord as `aria-keyshortcuts` names it: "Control+S Meta+S" for a chord, as either key works. */
export function chordShortcuts(chord: KeyChord): string[] {
  const tail: string[] = [];
  if (chord.alt) tail.push("Alt");
  if (chord.shift) tail.push("Shift");
  tail.push(chord.key === " " ? "Space" : /^[a-z]$/.test(chord.key) ? chord.key.toUpperCase() : chord.key);
  const said = tail.join("+");
  return chord.mod ? [`Control+${said}`, `Meta+${said}`] : [said];
}

/**
 * What the list of keys says under the views: where their chords work, what became of the bare digits,
 * and where a browser may keep the chords for itself. `#281 view-keys`. Chromium hands a page Ctrl+1 to
 * Ctrl+8 (Cmd on a Mac) before it switches tabs with them — they are not among the shortcuts it
 * reserves — and in the installed app there are no tabs to switch; another browser may keep them.
 */
export function viewKeysNote(mod: string): string {
  return `${mod}+0 goes to the voice view, and ${mod}+1 to ${mod}+4 work from anywhere, in a message box or the search too, and type nothing there. ` +
    "1, 2 and 3 on their own no longer change the view. In a browser tab, Chrome and Edge let this page have " +
    `${mod}+1 to ${mod}+4 before their own tab switching; a browser that keeps them for its tabs switches tabs ` +
    "instead, and the view picker in the bar chooses the same four views.";
}

export interface KeyHintsDependencies {
  /** The keymap, in the order the list of keys says it. */
  keymap(): readonly SaidBinding[];
  /** Whether the single-key shortcuts are on. */
  singleKeysOn(): boolean;
  /** The name of the reader's Ctrl or Cmd key. */
  modKeyName(): string;
  /** An element of the page by id. */
  byId(id: string): HTMLElement;
  /** Where the row buttons' shortcuts are kept for the rows drawn later. */
  readonly rowShortcuts: RowShortcuts;
  /** The child of `row` with `className`, or null. */
  childByClass(row: HTMLElement, className: string): HTMLElement | null;
  /** The groups of the list of keys, in order. */
  readonly groups: readonly string[];
  /** The group of the view keys, said with `viewKeysNote` under it. */
  readonly viewGroup: string;
}

/** The keys, said. */
export interface KeyHints {
  /** The chords of a binding, said: "↓ or j", "1, 2, 3". `live` leaves out what the switch has turned off. */
  bindingWords(binding: SaidBinding, live?: boolean): string;
  /**
   * Say the keys where a reader looks for them, from the keymap and in the words on their own keyboard:
   * the list of every key on the Keyboard entry of the Help screen; the switch in Settings and the line
   * under it; the `aria-keyshortcuts` of each button, which is what a screen reader reads, on the page
   * and on every row; and the tooltips of the glass, the gear and the Back buttons.
   */
  render(): void;
}

export function createKeyHints(deps: KeyHintsDependencies): KeyHints {
  const live = (chord: KeyChord): boolean => deps.singleKeysOn() || !typesCharacter(chord);

  function bindingWords(binding: SaidBinding, onlyLive = false): string {
    const mod = deps.modKeyName();
    const words = binding.chords.filter((chord) => !onlyLive || live(chord)).map((chord) => chordWords(chord, mod));
    return words.length === 2 ? words.join(" or ") : words.join(", ");
  }

  /** For each control a key presses, the keys that press it now: id (or the class of a row button) to the attribute value. */
  function controlShortcuts(field: "controls" | "rowControls"): Map<string, string[]> {
    const out = new Map<string, string[]>();
    for (const binding of deps.keymap()) {
      for (const name of binding[field] || []) {
        const said = binding.chords.filter(live).flatMap(chordShortcuts);
        out.set(name, [...(out.get(name) || []), ...said]);
      }
    }
    return out;
  }

  function render(): void {
    const el = deps.byId;
    const mod = deps.modKeyName();
    const single = deps.singleKeysOn();
    (el("single-keys") as HTMLInputElement).checked = single;
    el("single-keys-hint").textContent = single
      ? "Keys such as j, k, e and / act on the messages. Press ? for the list."
      : `Off: Esc, the arrows, Enter and the ${mod} chords still work. ${mod}+? lists them.`;
    for (const [id, said] of controlShortcuts("controls")) {
      if (said.length) el(id).setAttribute("aria-keyshortcuts", said.join(" "));
      else el(id).removeAttribute("aria-keyshortcuts");
    }
    deps.rowShortcuts.set(controlShortcuts("rowControls"));
    for (const list of ["discord-log", "pinned-log"]) {
      for (const row of [...el(list).children] as HTMLElement[]) {
        for (const className of deps.rowShortcuts.classes()) {
          const button = deps.childByClass(row, className);
          if (button) deps.rowShortcuts.say(button, className);
        }
      }
    }
    const binding = (id: string): SaidBinding =>
      deps.keymap().find((one) => (one.controls || []).includes(id)) as SaidBinding;
    el("search-toggle").setAttribute("title", `Search messages (${bindingWords(binding("search-toggle"), true)})`);
    el("open-settings").setAttribute("title", `Settings (${bindingWords(binding("open-settings"), true)})`);
    for (const id of ["close-settings", "close-help"]) {
      el(id).setAttribute("title", "Back (Esc)");
    }
    const groups: HTMLElement[] = [];
    if (!single) {
      const off = document.createElement("p");
      off.className = "hint";
      off.textContent = `Single-key shortcuts are off in Settings: only Esc, the arrows, Enter, PgUp, PgDn, Home, End ` +
        `and the ${mod} and ${mod === "Cmd" ? "Option" : "Alt"} chords act.`;
      groups.push(off);
    }
    for (const group of deps.groups) {
      const heading = document.createElement("h3");
      heading.className = "key-group";
      heading.textContent = group;
      const list = document.createElement("ul");
      list.className = "key-list";
      for (const one of deps.keymap().filter((entry) => entry.group === group)) {
        const row = document.createElement("li");
        const keys = document.createElement("kbd");
        keys.textContent = bindingWords(one);
        const meaning = document.createElement("span");
        meaning.textContent = typeof one.does === "function" ? one.does() : one.does;
        row.append(keys, meaning);
        list.append(row);
      }
      groups.push(heading, list);
      if (group === deps.viewGroup) {
        const note = document.createElement("p");
        note.className = "hint";
        note.textContent = viewKeysNote(mod);
        groups.push(note);
      }
    }
    el("keyboard-keys").replaceChildren(...groups);
  }

  return { bindingWords, render };
}

// The keys that press each row button (`#295 key-hints`; `#624 voice-page-modularity`, moved from the legacy
// script into view/shell): the `aria-keyshortcuts` of every row button, by its class, as the key hints last
// said them, so a row drawn after says the same as the rows already there. Working them out from the keymap is
// keyhints.ts; when, the page's.

/** A row button, as far as its shortcuts are written on it. */
export interface ShortcutButton {
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
}

/** The row buttons' shortcuts. */
export interface RowShortcuts {
  /** The shortcuts the key hints say now: for each row button's class, the keys that press it. */
  set(said: Map<string, string[]>): void;
  /** The classes of the row buttons a key presses. */
  classes(): IterableIterator<string>;
  /** Give `button`, of class `className`, its `aria-keyshortcuts`, or take them away where no key presses it. */
  say(button: ShortcutButton, className: string): void;
}

/** No row button pressed by a key yet. */
export function createRowShortcuts(): RowShortcuts {
  let rowShortcuts = new Map<string, string[]>();
  return {
    set(said) {
      rowShortcuts = said;
    },
    classes: () => rowShortcuts.keys(),
    say(button, className) {
      const said = (rowShortcuts.get(className) || []).join(" ");
      if (said) button.setAttribute("aria-keyshortcuts", said);
      else button.removeAttribute("aria-keyshortcuts");
    },
  };
}

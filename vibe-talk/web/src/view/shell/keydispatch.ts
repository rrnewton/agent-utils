// The page's keyboard dispatch (`#224 keyboard-shortcuts`, `#344 search-shortcut`, `#403 composer-key-global`;
// `#624 voice-page-modularity`, moved from the legacy script into view/shell for the plan's step 17): the two
// keydown listeners that walk the keymap, which binding a key runs, whether a chord may fire where the focus is,
// the Ctrl keys a text field keeps on a Mac, and the column it runs in. The keymap itself, what each binding
// does, and registering the listeners are the page's.

import { MAC_FIELD_EDIT_LETTERS, appleKeyboard, chordMatches, swallow, typesCharacter } from "./chords.ts";
import type { KeyChord, KeyEvent } from "./chords.ts";

/** A keydown, as the dispatch reads it. */
export interface DispatchedKey extends KeyEvent {
  readonly isComposing?: boolean;
  readonly keyCode?: number;
  readonly repeat?: boolean;
  readonly defaultPrevented?: boolean;
  readonly target?: unknown;
}

/** A binding of the keymap, as far as the dispatch reads it. */
export interface KeyBinding<Target> {
  readonly chords: readonly KeyChord[];
  /** What the key does; true when it took the key. A binding without one is only listed. */
  readonly run?: (event: DispatchedKey, target: Target) => unknown;
  /** Heard first, at the window, whatever holds the focus (its chords with Ctrl or Cmd). */
  readonly everywhere?: boolean;
  /** Fires in a text field too. */
  readonly inField?: boolean;
  /** A held key repeats it. */
  readonly repeats?: boolean;
  /** The group of the list of keys it is in. */
  readonly group?: string;
}

export interface KeyDispatchDependencies<Target, Pane, Column> {
  /** The keymap, walked in order. */
  keymap(): readonly KeyBinding<Target>[];
  /** Whether the single-key shortcuts are on. */
  singleKeysOn(): boolean;
  /** Whether `target` is a field that types. */
  isTextField(target: Target): boolean;
  /** Whether `target` answers its own keys (a slider, a menu). */
  keepsOwnKeys(target: Target): boolean;
  /** The keys are in use: the one selection moves from wherever the mouse put it. */
  keysInUse(): void;
  /** The name of the reader's Ctrl or Cmd key: "Cmd" on an Apple keyboard. */
  modKeyName(): string;
  /** The element holding the focus, for a key with no target. */
  activeElement(): Target;
  /** Whether the main screen is up, where the lists stand in columns. */
  onMain(): boolean;
  /** The list's pane `target` is in, or null. */
  paneFor(target: Target): Pane | null;
  /** The column of `pane`. */
  paneColumn(pane: Pane): Column;
  /** The column the keys moved last. */
  mainColumn(): Column;
  /** Run `act` in `column`. */
  inColumn(column: Column, act: () => void): void;
  /** The groups whose keys are the reader acting in the list: moving through the messages, acting on one. */
  readonly listGroups: readonly string[];
  /** The reader acted in the list. */
  readerActed(): void;
  /** A key no binding took: the scroll keys'. */
  routeScrollKey(event: DispatchedKey, target: Target): void;
}

/** The two listeners and the rule they share. */
export interface KeyDispatch<Target> {
  /** The keydown listener at the document: every key but the few `onEverywhereKey` takes first. */
  onPageKey(event: DispatchedKey | null | undefined): void;
  /**
   * The chords that act whatever holds the focus: those of the bindings marked `everywhere`, today the
   * search's Ctrl+/ (Cmd+/), `#344 search-shortcut`, and the message box's Ctrl+S (Cmd+S), `#403
   * composer-key-global`.
   *
   * Registered at the WINDOW in the CAPTURE phase, which a keydown reaches first: before the document,
   * before any element's own listener and before `onPageKey`. So nothing the page adds later can keep the
   * chord from it, a listener that stops the key included. Only a window capture listener registered
   * before it could, and the page has none; a browser extension's is out of the page's reach. The chord's
   * default is refused at once, so it is never the browser's, even where it finds nothing to open; a held
   * chord's repeats are refused too, and do nothing more. The key is NOT stopped: `onPageKey` still hears
   * it, notes the keys in use, and leaves it alone as a key already taken (`defaultPrevented`), as does
   * every element's own listener, none of which answers a Ctrl or Cmd chord with "/" or S.
   *
   * Nothing fires while an input method composes. An AltGr chord (Ctrl and Alt together, as Windows sends
   * it) matches no binding, so a "/" typed with AltGr is still a "/".
   */
  onEverywhereKey(event: DispatchedKey | null | undefined): void;
  /** Whether `chord` of `binding` may fire with the focus on `target`. */
  chordAllowed(binding: KeyBinding<Target>, chord: KeyChord, target: Target): boolean;
  /**
   * Whether `event` is one of the keys that edit text in every field on a Mac, pressed in a text field on an
   * Apple keyboard: Ctrl and one of `MAC_FIELD_EDIT_LETTERS`, without Cmd. Such a key is the field's, never
   * a chord of the page, so in a message box on a Mac Ctrl+K deletes to the end of the line as it does
   * everywhere else there, and Cmd+K is the channel picker. Off the field, and on other keyboards, Ctrl
   * works as Cmd does. Ctrl+S is not one of them, and goes to the message box on a Mac as it does
   * everywhere else.
   */
  macFieldEditKey(event: DispatchedKey, target: Target): boolean;
}

export function createKeyDispatch<Target, Pane, Column>(deps: KeyDispatchDependencies<Target, Pane, Column>): KeyDispatch<Target> {
  function chordAllowed(binding: KeyBinding<Target>, chord: KeyChord, target: Target): boolean {
    if (typesCharacter(chord) && !deps.singleKeysOn()) return false;
    if (chord.mod || chord.key === "Escape" || binding.inField) return true;
    return !deps.isTextField(target) && !deps.keepsOwnKeys(target);
  }

  function macFieldEditKey(event: DispatchedKey, target: Target): boolean {
    if (!event.ctrlKey || event.metaKey || !deps.isTextField(target) || !appleKeyboard(deps.modKeyName())) return false;
    const key = String(event.key || "");
    const letter = /^[a-z]$/i.test(key) ? key.toLowerCase() : /^Key[A-Z]$/.test(String(event.code || ""))
      ? String(event.code).slice(3).toLowerCase() : "";
    return letter !== "" && MAC_FIELD_EDIT_LETTERS.includes(letter);
  }

  /** The key's target, else the focus. */
  const targetOf = (event: DispatchedKey): Target => (event.target || deps.activeElement()) as Target;

  /**
   * `#229 desktop-two-column`. In the column of the list the key is for: the one the focus is in, else
   * the one the keys moved last.
   */
  function inColumnOf(target: Target, act: () => void): void {
    const pane = deps.onMain() ? deps.paneFor(target) : null;
    deps.inColumn(pane ? deps.paneColumn(pane) : deps.mainColumn(), act);
  }

  /** `onPageKey`, in the column it chose: the first binding that takes the key, else the scroll keys. */
  function dispatchKey(event: DispatchedKey, target: Target): void {
    for (const binding of deps.keymap()) {
      if (!binding.run) continue;
      if (!binding.chords.some((chord) => chordMatches(chord, event) && chordAllowed(binding, chord, target))) continue;
      // A held key repeats only the keys that move: a held e is one Done, not a Done on every row it passes.
      if (event.repeat && !binding.repeats) continue;
      // A key that moves through the messages or acts on one is the reader acting in the list, measured
      // from where they are (`readerActed`). The keys for a filter, the search and Escape are not: they
      // ARE filter changes, and a filter turned on and off again by them is a round trip. `#243`.
      if (binding.group !== undefined && deps.listGroups.includes(binding.group)) deps.readerActed();
      if (binding.run(event, target)) {
        swallow(event);
        return;
      }
    }
    deps.routeScrollKey(event, target);
  }

  return {
    chordAllowed,
    macFieldEditKey,
    onPageKey(event) {
      if (!event || event.isComposing || event.keyCode === 229) return;
      // `#239 single-selection`. A key moves the one selection from wherever the mouse put it. Said
      // before a key its own element took (Escape out of the search, Enter in a message box) is left
      // alone: the keys are in use all the same, and what a still pointer lights stays put away.
      deps.keysInUse();
      if (event.defaultPrevented) return;
      const target = targetOf(event);
      if (macFieldEditKey(event, target)) return;
      inColumnOf(target, () => dispatchKey(event, target));
    },
    onEverywhereKey(event) {
      if (!event || event.isComposing || event.keyCode === 229) return;
      const binding = deps.keymap().find((one) => one.everywhere && one.run &&
        one.chords.some((chord) => chord.mod && chordMatches(chord, event)));
      if (!binding) return;
      swallow(event);
      if (event.repeat) return;
      deps.keysInUse();
      const target = targetOf(event);
      inColumnOf(target, () => binding.run?.(event, target));
    },
  };
}

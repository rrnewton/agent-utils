// What key a keydown is, as the keymap reads it (`#344 search-shortcut`, `#281 view-keys`, `#425
// collapsed-lines`, `#525 cmd-0-voice-view`; `#624 voice-page-modularity`, moved from the legacy script into
// view/shell for the plan's step 17): a chord matched by the character a key typed, or by the place of the key
// where the layout types something else there, Ctrl and Cmd alike. Which binding a matched chord runs, and
// whether it may fire where the focus is, is the page's keyboard dispatch.

/** A chord of the keymap, as far as matching reads it. */
export interface KeyChord {
  /** `KeyboardEvent.key`: a letter in lower case, a character, or a named key. */
  readonly key: string;
  readonly mod?: boolean;
  readonly alt?: boolean;
  readonly shift?: boolean;
}

/** A keydown, as far as these read it. */
export interface KeyEvent {
  readonly key?: string;
  readonly code?: string;
  readonly ctrlKey?: boolean;
  readonly metaKey?: boolean;
  readonly altKey?: boolean;
  readonly shiftKey?: boolean;
  preventDefault?(): void;
}

/** Keep a key from the browser's own default for it. */
export function swallow(event: KeyEvent | null | undefined): void {
  if (event && typeof event.preventDefault === "function") event.preventDefault();
}

/** Whether the key is `letter`, on any layout: by the character, or by the place of the key on a layout that types no Latin letter there. */
export function isLetterKey(event: KeyEvent, letter: string): boolean {
  const key = String(event.key || "");
  if (/^[a-z]$/i.test(key)) return key.toLowerCase() === letter;
  // A key that types ASCII punctuation or a digit means that character, never the letter its place
  // would type on QWERTY: on Dvorak ";" sits where QWERTY has Z, and read by its place it was z, Undo.
  // Only a key that types no ASCII at all (a layout for another script) is read by its place.
  if (/^[\x21-\x7e]$/.test(key)) return false;
  return event.code === `Key${letter.toUpperCase()}`;
}

/**
 * The character a key typed, as the keymap reads it: what the browser says, with one correction made
 * only while Cmd is held. Cmd and Shift on the key that types "/" and "?" (US, UK and the like) is "?",
 * the list of keys, even where the browser says "/": a Mac browser may report the key as if Shift were
 * not held while Cmd is, as it does for a letter. `#344 search-shortcut`. Whether one does for this key
 * has not been seen on a Mac here; reading it so costs nothing where it does not. Never without Cmd, so
 * Ctrl+/ on Linux or Windows is what the browser says, whatever the layout. On a layout where "/" takes
 * Shift (Shift+7 on German), the key is not the Slash key and "/" stays "/": Cmd+/ there is the search.
 * Cmd+: (Cmd+Shift+;) has no such correction, and no report has said it needs one.
 */
export function typedKey(event: KeyEvent): string {
  const key = String(event.key || "");
  return key === "/" && event.metaKey && event.shiftKey && event.code === "Slash" ? "?" : key;
}

/**
 * Whether Ctrl or Cmd is held: every chord takes either, on every platform (see `modKeyName`), but
 * for the one exception `macFieldEditKey` makes in a text field on a Mac.
 */
export const modHeld = (event: KeyEvent): boolean => Boolean(event.ctrlKey || event.metaKey);

/**
 * The letters whose Ctrl chord edits text in every field on a Mac: the Emacs-style keys macOS gives
 * all its text fields, and Chrome honours in a page's. Ctrl+A and Ctrl+E go to the ends of the line,
 * Ctrl+K deletes to its end and Ctrl+Y puts that back, Ctrl+F, B, N and P move the caret, Ctrl+D and
 * Ctrl+H delete, Ctrl+O opens a line, Ctrl+T swaps two letters, Ctrl+L centres and Ctrl+V pages.
 */
export const MAC_FIELD_EDIT_LETTERS = "abdefhklnoptvy";

/** Whether a keyboard whose Ctrl or Cmd key is named `mod` says Cmd and Option, as an Apple one does. */
export const appleKeyboard = (mod: string): boolean => mod === "Cmd";

/** Whether `chord` is the key `event` reports. Shift counts for letters and named keys only. */
export function chordMatches(chord: KeyChord, event: KeyEvent): boolean {
  if (Boolean(chord.mod) !== modHeld(event) || Boolean(chord.alt) !== Boolean(event.altKey)) return false;
  if (/^[a-z]$/.test(chord.key)) {
    return Boolean(chord.shift) === Boolean(event.shiftKey) && isLetterKey(event, chord.key);
  }
  const key = typedKey(event);
  // A shifted 0 is never the Ctrl+0 chord: on the Digit0 key it is the ")" of the collapsed-lines
  // chord, which `parenByPlace` reads for that chord below — a browser reporting the digit itself
  // while Shift is held must not hand it to Ctrl+0 (`#525 cmd-0-voice-view`). (Shifted digits 1 to 4
  // keep matching: on AZERTY Shift is what types the digit, and the view chords read it as such.)
  if (chord.key.length === 1 && chord.key !== " ") return (key === chord.key && !(chord.key === "0" && event.shiftKey)) || digitByPlace(chord, event) || parenByPlace(chord, event);
  return key === chord.key && Boolean(chord.shift) === Boolean(event.shiftKey);
}

/**
 * Whether `event` is the digit chord `chord` by the place of its key, on a layout whose digit row types
 * something else without Shift: on AZERTY Ctrl+1 reports "&", on the key a browser's own Ctrl+1 is read
 * from. `#281 view-keys`. Never for a key that types a digit, which is read as that digit; nor with
 * Shift held, which on QWERTY makes Ctrl+2 Ctrl+@ — another key, and not the view key.
 */
function digitByPlace(chord: KeyChord, event: KeyEvent): boolean {
  return Boolean(chord.mod) && /^[0-9]$/.test(chord.key) && !event.shiftKey &&
    !/^[0-9]$/.test(String(event.key || "")) && event.code === `Digit${chord.key}`;
}

/**
 * Whether `event` is the bracket chord `chord` by the place of its key: Ctrl or Cmd and Shift on
 * Digit9 for "(", on Digit0 for ")", where that key types no bracket — a layout whose shifted
 * digit row is digits, or a browser that reports the digit while Cmd is held, as it may report a
 * letter (`typedKey`). `#425 collapsed-lines`. Never for a key that DID type a bracket, which
 * means the bracket it typed: on a German layout Shift+9 is ")" by place for "(", and it is the
 * ")" chord there, by its character, as every character chord is read. Never without Shift, which
 * is the browser's own tab key, Ctrl+9 or Ctrl+0.
 */
function parenByPlace(chord: KeyChord, event: KeyEvent): boolean {
  const place = chord.key === "(" ? "Digit9" : chord.key === ")" ? "Digit0" : null;
  if (place === null || !chord.mod || !event.shiftKey) return false;
  const key = String(event.key || "");
  return key !== "(" && key !== ")" && event.code === place;
}

/** A chord of one key that types a character: what the Settings switch turns off, and what never fires in a field. */
export const typesCharacter = (chord: KeyChord): boolean => !chord.mod && !chord.alt && chord.key.length === 1 && chord.key !== " ";

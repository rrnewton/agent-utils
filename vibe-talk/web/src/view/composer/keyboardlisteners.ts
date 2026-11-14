// The on-screen keyboard's and the columns' listeners (`#429 keyboard-hides-bar`, `#575`/`#610
// mobile-keyboard-compose`, `#340 composer-grow-box`, `#846 draft-chip-honest`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/composer for the plan's step
// 17): the window resized under the columns, the visual viewport resized or panned over the keyboard
// (or the window, where there is no visual viewport), the lists and the window scrolled in the jump's
// moment, the reader's own touch ending that moment, the debug readout's switch and its first drawing,
// the keyboard's geometry reported, and the focus arriving or leaving once the tap is over — then the
// first measure as the window loads. The guards stay the page's, so a page built without a browser
// hears none of this. What each does is the page's; mounted once, in the page's order, and taken down
// again.

import { createListening } from "../dom/index.ts";

export interface KeyboardListenersDependencies {
  /** An element of the page by id, and one of the left column's by id. */
  lookup(id: string): HTMLElement;
  leftLookup(id: string): HTMLElement;
  /** The window, where it can be listened to at all; null on a page built without one. */
  windowTarget(): EventTarget | null;
  /** The visual viewport, where the browser has one that can be listened to; null otherwise. */
  visualViewport(): EventTarget | null;
  /** `navigator.virtualKeyboard`, where the browser has one that can be listened to; null otherwise. */
  virtualKeyboard(): EventTarget | null;
  /** The columns redrawn for the window's new size, once a frame (`#340`, `#846`). */
  columnsResizeAsk(): void;
  /** Whether the on-screen keyboard is up, reckoned again. */
  noteKeyboard(): void;
  /** The reader touched: the jump's moment is over. */
  noteKeyboardReader(): void;
  /** A scroll of a list or the window in the jump's moment, heard by reference as it was. */
  settleOnScroll(): void;
  /** The debug readout's switch changed, and the readout drawn as stored. */
  applyKeyboardDebug(on: boolean): void;
  keyboardDebugChecked(): boolean;
  showKeyboardDebug(): void;
  /** Run `act` once the current task is done, as `setTimeout(act, 0)` did. */
  afterThisTask(act: () => void): void;
  /**
   * `ios-keyboard-send`. The Send at the keyboard's top pressed: the focused box's own Send.
   * Optional: a page built without the corner Send hears no press for it.
   */
  pressCornerSend?(): unknown;
}

/** The listeners, mounted. */
export interface KeyboardListeners {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the keyboard's and the columns' listeners, in the page's order, and take the first measure. */
export function mountKeyboardListeners(deps: KeyboardListenersDependencies): KeyboardListeners {
  const { listen, unmount } = createListening();
  const win = deps.windowTarget();
  if (win) listen<Event>(win, "resize", () => deps.columnsResizeAsk());
  const viewport = deps.visualViewport();
  if (viewport) {
    listen<Event>(viewport, "resize", () => deps.noteKeyboard());
    // `#575 mobile-keyboard-compose`. Safari pans the page up over its keyboard, after the resize.
    listen<Event>(viewport, "scroll", () => deps.noteKeyboard());
  } else if (win) {
    listen<Event>(win, "resize", () => deps.noteKeyboard());
  }
  // `#610 mobile-keyboard-compose`. Safari also scrolls the list itself, or the document, to show the
  // focused box as its keyboard comes up; in the jump's moment that scroll is the browser's.
  for (const id of ["scroll-area", "side-scroll"]) {
    listen<Event>(deps.leftLookup(id), "scroll", () => deps.settleOnScroll(), { passive: true });
  }
  if (win) listen<Event>(win, "scroll", () => deps.settleOnScroll(), { passive: true });
  for (const type of ["touchstart", "pointerdown", "wheel"]) {
    listen<Event>(document, type, () => deps.noteKeyboardReader(), { capture: true, passive: true });
  }
  listen<Event>(deps.lookup("keyboard-debug"), "change", () => deps.applyKeyboardDebug(deps.keyboardDebugChecked()));
  // `ios-keyboard-send`. The Send at the keyboard's top keeps the focus in its box through the
  // press (`mousedown`, which a tap is given too, would otherwise take the keyboard down), as
  // the new-line key does (`wireNewlineKey`).
  const cornerSend = deps.lookup("keyboard-send");
  if (cornerSend && typeof deps.pressCornerSend === "function") {
    listen<Event>(cornerSend, "mousedown", (event) => event.preventDefault());
    listen<Event>(cornerSend, "click", () => deps.pressCornerSend!());
  }
  deps.showKeyboardDebug();
  const keys = deps.virtualKeyboard();
  if (keys) listen<Event>(keys, "geometrychange", () => deps.noteKeyboard());
  // A focus that moves is decided once the tap that moved it is over, from where the focus went.
  for (const type of ["focusin", "focusout"]) {
    listen<Event>(document, type, () => deps.afterThisTask(() => deps.noteKeyboard()));
  }
  // The window as it loads, which is the first measure of its tallest.
  deps.noteKeyboard();
  return { unmount };
}

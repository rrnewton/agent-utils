// The keys' and the selection's listeners (`#224 keyboard-shortcuts`, `#344 search-shortcut`, `#296
// copy-highlighted-message`, `#239 single-selection`; `#624 voice-page-modularity`, moved from the legacy script's
// wiring into view/shell for the plan's step 17): every key the page answers heard in one place, the chords that act
// whatever holds the focus heard first, a copy with nothing selected, the single-key switch, and the clicks, the
// focus and the pointer that move the one selection. Mounted once, in the page's order, and taken down again.

/** A list the keys move through, as far as its listeners read it. */
export interface ListenedPane {
  scroller(): HTMLElement;
}

export interface KeyListenersDependencies<Pane extends ListenedPane> {
  /** The keydown listeners: at the document, and at the window in the capture phase (`createKeyDispatch`). */
  onPageKey(event: KeyboardEvent): void;
  onEverywhereKey(event: KeyboardEvent): void;
  /** A copy, and the moment before it, with nothing selected (`createHighlightCopy`). */
  onPageBeforeCopy(event: Event): void;
  onPageCopy(event: ClipboardEvent): void;
  /** Restore the single-key switch as kept, say the keys as they will act, and turn the keys on or off. */
  restoreSingleKeys(): void;
  renderKeyHints(): void;
  applySingleKeys(on: boolean): void;
  /** The single-key switch in Settings. */
  singleKeysBox(): HTMLInputElement;
  /** The lists the keys move through. */
  keyPanes(): readonly Pane[];
  /** A click in `pane`, and the focus arriving in it (`createPointerSelection`). */
  noteClickedRow(pane: Pane, node: unknown): void;
  onPaneFocus(pane: Pane, event: FocusEvent): void;
  /** Every pointer move and press on the page. */
  onPointerMove(event: PointerEvent): void;
  onPointerDown(event: PointerEvent): void;
}

/** The listeners, mounted. */
export interface KeyListeners {
  /** Take every listener down again. */
  unmount(): void;
}

/**
 * Mount the keys' and the selection's listeners. The single-key switch is restored before the keys are said, so
 * the keys are said as they will act. Listeners on the same target keep the page's order relative to the ones it
 * adds before and after this call.
 */
export function mountKeyListeners<Pane extends ListenedPane>(deps: KeyListenersDependencies<Pane>): KeyListeners {
  const taken: (() => void)[] = [];
  const listen = <Kind extends Event>(target: EventTarget, type: string, listener: (event: Kind) => void,
    options?: AddEventListenerOptions): void => {
    const heard = listener as EventListener;
    target.addEventListener(type, heard, options);
    taken.push(() => target.removeEventListener(type, heard, options));
  };
  // `#224 keyboard-shortcuts`. Every key the page answers, in one place: see `keyDispatch.onPageKey`.
  listen<KeyboardEvent>(document, "keydown", (event) => deps.onPageKey(event));
  // `#344 search-shortcut`. ...but for the chords that act whatever holds the focus, heard before any other
  // listener of the page: see `keyDispatch.onEverywhereKey`.
  listen<KeyboardEvent>(window, "keydown", (event) => deps.onEverywhereKey(event), { capture: true });
  // `#296 copy-highlighted-message`. A copy with nothing selected copies the highlighted message.
  listen<Event>(document, "beforecopy", (event) => deps.onPageBeforeCopy(event));
  listen<ClipboardEvent>(document, "copy", (event) => deps.onPageCopy(event));
  deps.restoreSingleKeys();
  deps.renderKeyHints();
  const box = deps.singleKeysBox();
  listen<Event>(box, "change", () => deps.applySingleKeys(box.checked));
  // A click in a list selects its row, so that the next key starts there; the focus arriving in a list makes it the
  // one in use, and a row it lands in the selection. `#239 single-selection`: a real mouse move over a row selects
  // it, and a pointer's kind says whether its click or tap is drawn.
  for (const pane of deps.keyPanes()) {
    listen<MouseEvent>(pane.scroller(), "click", (event) => deps.noteClickedRow(pane, event && event.target));
    listen<FocusEvent>(pane.scroller(), "focusin", (event) => deps.onPaneFocus(pane, event));
  }
  listen<PointerEvent>(document, "pointermove", (event) => deps.onPointerMove(event));
  listen<PointerEvent>(document, "pointerdown", (event) => deps.onPointerDown(event));
  return {
    unmount() {
      for (const take of taken.splice(0)) take();
    },
  };
}

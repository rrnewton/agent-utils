// What a key does in a message box, and the new line a phone's keyboard cannot type (`#225
// enter-to-send`, `paste-never-sends`, `#575 mobile-keyboard-compose`; `#624 voice-page-modularity`,
// moved from the legacy script; plan step 16, view/composer). Whether Enter sends is the reader's
// setting, which the page keeps and draws; it is passed in, as is the clock.

/** The message boxes, each of which hands its Enter to `onKey`. */
const COMPOSER_FIELDS = ["compose-text", "channel-compose-text", "reply-text", "side-compose-text"];

/**
 * How long after a paste, a drop or a run of text put in outside a composition an Enter in the same
 * box is taken to be part of it, in milliseconds. `paste-never-sends`; see `onComposerKey`.
 *
 * Chrome delivers the Enter a pasted newline becomes in the same task as the text before it, so a few
 * milliseconds would do; the rest is room for a loaded phone. A person tapping the clipboard and then
 * Send's key takes far longer than this.
 */
const PASTE_ENTER_MS = 250;

/**
 * `#575 mobile-keyboard-compose`. Each box's new-line key, drawn in its corner while a phone's keyboard is up
 * and Enter sends (web/voice.css), by the box it writes in.
 *
 * The owner, 2026-10-27: "I thought previously there was a way to put in new lines by pressing shift and
 * having the send button turn into an enter button but that's not working for me. It just stays a send
 * button and I have no way to place a new line." Whether a keyboard draws a return key under its Shift is
 * the keyboard's own choice, and none tells the page: Gboard and iOS both send their return key as an
 * Enter with no `shiftKey`, so from a phone's keys Shift+Enter is a bare Enter, and a bare Enter sends
 * while Enter to send is on. Nothing in this page took the Shift away (`paste-never-sends` leaves a
 * typed Enter as it was), and there was never a Shift to take. So the new line is a key of the page's.
 */
const NEWLINE_KEYS = [["channel-newline", "channel-compose-text"], ["side-newline", "side-compose-text"],
  ["reply-newline", "reply-text"]];

/** What the keys ask of the page. */
export interface ComposerKeysDependencies {
  /** The wall clock, in milliseconds. */
  now(): number;
  /** Whether a bare Enter sends: the reader's "Enter to send". */
  enterSends(): boolean;
}

/** A keystroke, as far as deciding a send reads it. */
export interface ComposerKeyEvent {
  readonly key?: string;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
  readonly ctrlKey?: boolean;
  readonly metaKey?: boolean;
  readonly shiftKey?: boolean;
  readonly altKey?: boolean;
  readonly repeat?: boolean;
  readonly target?: unknown;
  preventDefault?(): void;
}

/** The keys' commands. */
export interface ComposerKeys {
  /** What a keystroke in a message box does: what `send` returned, or undefined when nothing was sent. */
  onKey(event: ComposerKeyEvent | null | undefined, send: () => unknown, box: unknown): unknown;
  /** A message box's `beforeinput`, `input` or `paste`: note text arriving other than by a key or a word. */
  noteInsertion(box: unknown, event: unknown): void;
  /** Whether `box` took a paste or a run of text so lately that an Enter now is part of it. */
  insertedJustNow(box: unknown): boolean;
}

/** Nothing pasted yet. */
export function createComposerKeys(deps: ComposerKeysDependencies): ComposerKeys {
  /** When each message box last took a paste, a drop or a run of text outside a composition, by box. */
  const boxInsertedAt = new WeakMap<object, number>();
  const insertedJustNow = (box: unknown): boolean => {
    const at = box ? boxInsertedAt.get(box as object) : undefined;
    return typeof at === "number" && deps.now() - at <= PASTE_ENTER_MS;
  };
  return {
    onKey(event, send, box) {
      if (!event || event.key !== "Enter") return undefined;
      if (event.isComposing || event.keyCode === 229) return undefined;
      if (insertedJustNow(box)) return undefined;
      const chord = Boolean(event.ctrlKey || event.metaKey);
      const sends = chord || (deps.enterSends() && !event.shiftKey && !event.altKey);
      if (sends && event.preventDefault) {
        event.preventDefault();
      }
      return sends && !event.repeat ? send() : undefined;
    },
    noteInsertion(box, event) {
      const held = event as { readonly inputType?: unknown; readonly data?: unknown; readonly type?: unknown; readonly isComposing?: unknown } | null;
      if (!box || !held) return;
      const type = typeof held.inputType === "string" ? held.inputType : "";
      const data = typeof held.data === "string" ? held.data : "";
      const pasted = held.type === "paste" || /FromPaste$|FromDrop$|FromYank$/.test(type);
      const run = type === "insertText" && !held.isComposing && (data.length > 1 || data.includes("\n"));
      if (pasted || run) boxInsertedAt.set(box as object, deps.now());
    },
    insertedJustNow,
  };
}

/**
 * Put a new line in `box` where its caret is, over any selection, and tell the box as typing does, by an
 * `input`, so its draft, its Grow and what its Send says follow. Never through `onComposerKey`: there is
 * no Enter, so nothing here can send, and no paste is noted, so the reader's next Enter still sends.
 *
 * Through the browser's editor where it will (`insertLineBreak`, an `input` of that type, which
 * `noteBoxInsertion` does not take for a paste), so the box's undo keeps the new line and a word an input
 * method is still composing is committed by the editor rather than overwritten under it. Written into
 * the value, with its `input` made here, where the editor will not.
 */
export function insertNewline(field: HTMLElement): void {
  const box = field as HTMLTextAreaElement;
  const was = box.value;
  if (typeof document.execCommand === "function" && document.activeElement === box &&
    document.execCommand("insertLineBreak") && box.value !== was) return;
  const value = String(box.value || "");
  const start = typeof box.selectionStart === "number" ? box.selectionStart : value.length;
  const end = typeof box.selectionEnd === "number" ? Math.max(start, box.selectionEnd) : start;
  box.value = `${value.slice(0, start)}\n${value.slice(end)}`;
  box.setSelectionRange(start + 1, start + 1);
  box.dispatchEvent(typeof InputEvent === "function"
    ? new InputEvent("input", { bubbles: true, inputType: "insertLineBreak" })
    : ({ type: "input", inputType: "insertLineBreak", target: box } as unknown as Event));
}

/**
 * Give `key` its box. The focus is kept in the box through the press — `mousedown`, which a tap is
 * given too, would otherwise move it to the key and take the keyboard down — so the reader goes on
 * typing on the next line with the keyboard where it was.
 */
export function wireNewlineKey(key: HTMLElement, box: HTMLElement): void {
  key.addEventListener("mousedown", (event) => event.preventDefault());
  key.addEventListener("click", () => {
    insertNewline(box);
    if (document.activeElement !== box) box.focus({ preventScroll: true });
  });
}

/** Listen to `box` for what `note` notes (`noteInsertion`). Each message box, as it is given its Enter. */
export function watchBoxInsertions(box: EventTarget, note: (box: unknown, event: unknown) => void): void {
  for (const type of ["beforeinput", "input", "paste"]) {
    box.addEventListener(type, (event) => note(box, event));
  }
}

export { COMPOSER_FIELDS, NEWLINE_KEYS, PASTE_ENTER_MS };

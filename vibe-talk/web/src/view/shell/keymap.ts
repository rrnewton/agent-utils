// The keymap (`#224 keyboard-shortcuts`, the proposal's section 5; `#624 voice-page-modularity`, moved from the
// legacy script into view/shell for the plan's step 17): every key the page answers, in the order the list of keys
// shows them, with what each does in the words of that list. The keyboard dispatch walks it (`createKeyDispatch`),
// and the list of keys, the single-key switch's line and every `aria-keyshortcuts` are read off it
// (`createKeyHints`), so the list a reader is shown cannot drift from what the keys do. What each key does is the
// page's: its acts come in as dependencies.

import { isLink, isTextField } from "../dom/index.ts";
import type { AfterDone } from "./donekeys.ts";
import { modHeld, typedKey } from "./chords.ts";
import type { KeyChord } from "./chords.ts";
import type { DispatchedKey } from "./keydispatch.ts";
import type { ViewChoice } from "./viewkeys.ts";

export const KEYS_PAGE = "Scrolling, search and Settings";
export const KEYS_MOVING = "Moving through the messages";
export const KEYS_MESSAGE = "The selected message";
export const KEYS_VIEW = "Views and filters";
export const KEYS_WRITING = "Writing";

/** One binding of the keymap. */
export interface KeymapBinding<Target> {
  /** Which part of the list of keys it is in. */
  readonly group: string;
  /**
   * Its chords. A character such as "?" or ":" is matched as it is typed, whatever it took to type it on the
   * layout of the reader (`typedKey`): Ctrl+? is Ctrl+Shift+/ on a US keyboard, and Ctrl+/ is Ctrl+Shift+7 on a
   * German one. `mod` is Ctrl or Cmd, either on every platform but for `keyDispatch.macFieldEditKey`; `alt` comes
   * only ever with an arrow, as Option and a letter types a character on a Mac.
   */
  readonly chords: readonly KeyChord[];
  /**
   * What it does, as the list of keys says it: asked each time the list is drawn where it names a chord, so that
   * the chord is in the words on the reader's keyboard.
   */
  readonly does: string | (() => string);
  /**
   * Answered by the browser's own scrolling, by the own handler of a message box (`onComposerKey`), or by the
   * reading strip grip's own handler (`onReadGripKey`), rather than by `run`: listed so the list is whole.
   */
  readonly by?: "browser" | "box" | "grip";
  /**
   * A held key repeats it. Only the keys that move, and a chord whose `run` keeps the default of the browser shut
   * on every repeat of a held key (Ctrl+S, the Save page).
   */
  readonly repeats?: boolean;
  /** Fires in a text field too, where `run` decides for itself. */
  readonly inField?: boolean;
  /**
   * Its Ctrl/Cmd chords are taken at the window in the capture phase, before any element's own listener
   * (`keyDispatch.onEverywhereKey`), so they act whatever holds the focus.
   */
  readonly everywhere?: boolean;
  /** Ids of the buttons of the page this key presses, for `aria-keyshortcuts`. */
  readonly controls?: readonly string[];
  /** Classes of the buttons of a row this key presses, the same. */
  readonly rowControls?: readonly string[];
  /** Do it; whether it did. */
  readonly run?: (event: DispatchedKey, target: Target) => unknown;
}

/** What the keys do: the page's acts, each answering whether it did anything. */
export interface KeymapActs<Target> {
  /** The name of the reader's Ctrl or Cmd key. */
  modKeyName(): string;
  /** Whether the channels come from more than one chat service, and the one service's name. */
  manyChatServices(): boolean;
  chatServiceName(): string;
  /** The orders of the Thread list's cards, as the list of keys names them in turn. */
  threadOrderWords(): string;
  /** The fewest and the most lines a collapsed message shows. */
  minCollapsedLines(): number;
  maxCollapsedLines(): number;
  /** The pane and row the keys are on from `target`, in sight or not. */
  keyContext(target: Target): { readonly row: unknown };
  /** Press the row button of class `className` on the row the keys act on: what it pressed, "took", or null. */
  pressRowButton(target: Target, className: string): unknown;
  onEscape(event: DispatchedKey, target: Target): unknown;
  placeFromKeyboard(step: number): unknown;
  listEdge(target: Target, end: boolean): unknown;
  jumpList(target: Target, end: boolean): unknown;
  searchFromAnywhere(): unknown;
  searchFromKeyboard(): unknown;
  settingsFromKeyboard(): unknown;
  keysFromKeyboard(): unknown;
  stepFromKeyboard(target: Target, direction: number, held?: boolean): unknown;
  stepChannel(event: DispatchedKey): unknown;
  pickChannel(): unknown;
  openFromKeyboard(event: DispatchedKey, target: Target): unknown;
  threadFromKeyboard(target: Target): unknown;
  threadOutFromKeyboard(): unknown;
  threadStripFromKeyboard(target: Target): unknown;
  columnFromKeyboard(right: boolean): unknown;
  replyFromKeyboard(target: Target): unknown;
  quoteFromKeyboard(): unknown;
  menuFromKeyboard(target: Target): unknown;
  readFromKeyboard(target: Target): unknown;
  detailsFromKeyboard(target: Target): unknown;
  copyLinkFromKeyboard(target: Target): unknown;
  /** `#889 voice-page-copy-link-key`. Copy the selected message's vibe-talk link. */
  deepLinkFromKeyboard(target: Target): unknown;
  /** Whether the page runs as the installed app (no address bar), where Ctrl+L (Cmd+L) is the page's alone. */
  standalone(): boolean;
  copyChordFromKeyboard(): unknown;
  copyFromKeyboard(): unknown;
  nextUnreadFromKeyboard(target: Target): unknown;
  stepUnreadFromKeyboard(target: Target, direction: number): unknown;
  /** `#1321 after-done-setting`. What e does after marking Done, as kept in Settings. */
  afterDone(): AfterDone;
  doneFromKeyboard(target: Target): unknown;
  readCollapseFromKeyboard(target: Target): unknown;
  foldOneFromKeyboard(target: Target): unknown;
  foldStepFromKeyboard(step: number, event: DispatchedKey, target: Target): unknown;
  collapsedLinesFromKeyboard(step: 1 | -1, target: Target): unknown;
  packFromKeyboard(target: Target): unknown;
  unpackFromKeyboard(): unknown;
  bookmarkFromKeyboard(target: Target): unknown;
  bookmarkJumpFromKeyboard(): unknown;
  viewFromKeyboard(view: ViewChoice, repeat?: boolean): unknown;
  voiceFromKeyboard(repeat?: boolean): unknown;
  readModeFromKeyboard(): unknown;
  filterFromKeyboard(name: string): unknown;
  undoFromKeyboard(): unknown;
  orderFromKeyboard(): unknown;
  mainRowsFromKeyboard(): unknown;
  columnsFromKeyboard(): unknown;
  /** Toggle the open document's outline pop-up (`#1038 documents-outline-popup`); whether it did. */
  outlineFromKeyboard(): unknown;
  composeFromKeyboard(): unknown;
  writeFromKeyboard(): unknown;
  newestFromComposer(target: Target): unknown;
}

/**
 * Every key the page answers, in the order the list of keys shows them. The first binding whose chord
 * matches and may fire there, and that does something, takes the key; one that does nothing (→ on a
 * message with no thread) leaves it to the next, and at the end to the browser.
 */
export function createKeymap<Target>(acts: KeymapActs<Target>): KeymapBinding<Target>[] {
  const {
    modKeyName, manyChatServices, chatServiceName, threadOrderWords, minCollapsedLines, maxCollapsedLines, keyContext,
    pressRowButton, onEscape, placeFromKeyboard, listEdge, jumpList, searchFromAnywhere, searchFromKeyboard,
    settingsFromKeyboard, keysFromKeyboard, stepFromKeyboard, stepChannel, pickChannel, openFromKeyboard,
    threadFromKeyboard, threadOutFromKeyboard, threadStripFromKeyboard, columnFromKeyboard, replyFromKeyboard, quoteFromKeyboard,
    menuFromKeyboard, readFromKeyboard, detailsFromKeyboard, copyLinkFromKeyboard, deepLinkFromKeyboard, standalone,
    copyChordFromKeyboard, copyFromKeyboard, nextUnreadFromKeyboard, stepUnreadFromKeyboard, afterDone, doneFromKeyboard, readCollapseFromKeyboard, foldOneFromKeyboard,
    foldStepFromKeyboard, collapsedLinesFromKeyboard, packFromKeyboard, unpackFromKeyboard, bookmarkFromKeyboard,
    bookmarkJumpFromKeyboard, viewFromKeyboard, voiceFromKeyboard, readModeFromKeyboard, filterFromKeyboard,
    undoFromKeyboard, orderFromKeyboard, mainRowsFromKeyboard, columnsFromKeyboard, outlineFromKeyboard, composeFromKeyboard,
    writeFromKeyboard, newestFromComposer,
  } = acts;
  return [
    {
      group: KEYS_PAGE, chords: [{ key: "Escape" }],
      does: "Back: closes a menu, the search or a sheet, takes " +
        "the highlight off a thread card, lets go of a thread selected in All, takes the mark off a linked message, " +
        "leaves Settings, " +
        "Help, Reply or a thread — one opened from the thread list goes back to it — and leaves the thread list for " +
        "the view it was chosen from. In a message box with text in it, leaves the box first and keeps the draft; " +
        "on the channel or view picker, back to the messages",
      controls: ["close-settings", "close-help", "thread-back", "side-close"],
      run: (event, target) => onEscape(event, target),
    },
    {
      // `#423 back-key-consistency`. Heard first, at the window, as Ctrl+S is (`keyDispatch.onEverywhereKey`): on a Mac
      // Cmd+[ and Cmd+] are the browser's own Back and Forward, which go back only through what `#304` gives
      // an entry. These go back and forward through every place visited.
      group: KEYS_PAGE, chords: [{ key: "[", mod: true }, { key: "]", mod: true }], everywhere: true,
      does: () => `${modKeyName()}+[ goes back to where you were before, and ${modKeyName()}+] forward again, in every ` +
        "view: Main, All, the thread list, two columns, a thread (across the window or on the right), Settings and " +
        "Help. From a thread opened from the thread list, back to the list; from the list, to the view it was chosen " +
        "from; from Settings or Help, to where you opened them. A menu closes first, and on Back a reply open in a " +
        "list (its words kept). At either end it says so and stays. The browser's own Back and a phone's back " +
        "gesture close what is open, as Esc does",
      run: (event) => placeFromKeyboard(typedKey(event) === "]" ? 1 : -1),
    },
    {
      group: KEYS_PAGE, by: "browser",
      chords: [{ key: "PageUp" }, { key: "PageDown" }, { key: " " }, { key: " ", shift: true }],
      does: "Scroll a screenful, up or down; the selected message follows. In a message box, PgUp and PgDn " +
        "scroll the list beside it",
    },
    {
      group: KEYS_PAGE, chords: [{ key: "Home" }, { key: "End" }],
      does: "The top of the list or its newest message, and select it",
      run: (event, target) => listEdge(target, event.key === "End"),
    },
    {
      group: KEYS_PAGE, chords: [{ key: "Home", mod: true }, { key: "End", mod: true }],
      does: "The top or the newest from anywhere, a message box included",
      run: (event, target) => jumpList(target, event.key === "End"),
    },
    {
      // `#344 search-shortcut`. The chord is heard first, at the window, so it searches whatever holds
      // the focus (`keyDispatch.onEverywhereKey`); the bare "/" is as it was, a character in a field.
      group: KEYS_PAGE, chords: [{ key: "/" }, { key: "/", mod: true }], everywhere: true,
      // `#248 composer-key`. Ctrl+S was the search's too, from `#224 keyboard-shortcuts`; it is the
      // message box's now, and the list of keys says so, for a reader whose hands still reach for it.
      does: () => `Search the messages; pressed again, back to what you typed. ${modKeyName()}+/ searches from ` +
        "anywhere: a message box, Settings, Help, Reply (its draft kept), an open menu, with single keys on or off. " +
        `${modKeyName()}+S, which searched until now, goes to the message box`,
      controls: ["search-toggle"],
      run: (event) => (modHeld(event) ? searchFromAnywhere() : searchFromKeyboard()),
    },
    {
      // `#262 search-load-older`. The field's own handler, which marks the hits and walks back.
      group: KEYS_PAGE, by: "box", chords: [{ key: "Enter" }],
      does: "In the search field: search older messages, a few pages at a time; the step back at the head " +
        "of the list says Stop meanwhile",
    },
    {
      group: KEYS_PAGE, chords: [{ key: "," }, { key: ",", mod: true }],
      does: "Settings, and again (or Esc) back to where you were. Settings keep themselves as they change",
      controls: ["open-settings"],
      run: (event) => settingsFromKeyboard() || modHeld(event),
    },
    {
      // `#344 search-shortcut`. Ctrl+/ (Cmd+/) was this list's until the owner gave it to the search:
      // "map it to Cmd-? (i.e. Cmd-shift-/)". The list says where it went, as for Ctrl+S.
      group: KEYS_PAGE, chords: [{ key: "?" }, { key: "?", mod: true }],
      does: () => `This list of keys. ${modKeyName()}+? is ${modKeyName()}+Shift+/; ${modKeyName()}+/, which opened ` +
        "this list until now, searches",
      controls: ["help-link-keyboard"],
      run: () => keysFromKeyboard(),
    },
    {
      // `#1038 documents-outline-popup`. The reader's outline without the button: g toggles wherever the
      // single keys reach, and the modified chord from anywhere, as for / and ? (a message box included).
      group: KEYS_PAGE, chords: [{ key: "g" }, { key: "g", mod: true }], everywhere: true,
      does: () => `In Documents, the document outline: opens the section list (with where you are marked), ` +
        "and again closes it. Inside it the arrow keys move, Enter jumps to a section, and Esc closes it. " +
        `${modKeyName()}+g toggles it from anywhere, a message box included`,
      controls: ["doc-outline-open"],
      run: () => outlineFromKeyboard(),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowDown" }, { key: "j" }], repeats: true,
      does: "The next message. One taller than the screen is scrolled through first",
      run: (event, target) => stepFromKeyboard(target, 1, Boolean(event.repeat)),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowUp" }, { key: "k" }], repeats: true,
      does: "The previous message",
      run: (_event, target) => stepFromKeyboard(target, -1),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowDown", shift: true }], repeats: true,
      does: "The next unread message, skipping read ones",
      run: (_event, target) => stepUnreadFromKeyboard(target, 1),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowUp", shift: true }], repeats: true,
      does: "The previous unread message, skipping read ones",
      run: (_event, target) => stepUnreadFromKeyboard(target, -1),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "n" }], repeats: true,
      does: "The next unread message below",
      run: (_event, target) => nextUnreadFromKeyboard(target),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "Enter" }, { key: "o" }],
      does: "Open or close the selected message, as a tap does (in Read, read it aloud). On a “… read messages …” line, " +
        "open the run. On a thread card, highlight it and show all of it, or put that out",
      rowControls: ["fold", "side-card"],
      run: (event, target) => openFromKeyboard(event, target),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowRight" }],
      does: "Into the thread of the selected message. On the thread cards, into the highlighted card's thread, or " +
        "else the selected card's. In two columns it opens on the right, and the keys go with it; from the thread " +
        "list, across the window",
      rowControls: ["thread-badge", "side-card-open", "thread-card-open"],
      run: (_event, target) => threadFromKeyboard(target),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowLeft" }],
      does: "Out of a thread, back where you came from. In two columns, from the right column back to Main, or " +
        "back to the thread cards from a thread entered from one",
      controls: ["thread-back"],
      run: () => threadOutFromKeyboard(),
    },
    // `#525 cmd-0-voice-view`. The call's view, on the digit before the four. It repeats as they
    // do, each repeat being the browser's zoom reset as well; a repeat chooses nothing.
    {
      group: KEYS_VIEW, chords: [{ key: "0", mod: true }], repeats: true,
      does: "Voice: the call's view, as the view switch goes there. From anywhere, a message box or the search included",
      controls: ["view-switch"],
      run: (event) => voiceFromKeyboard(event.repeat),
    },
    // `#281 view-keys`. The four views, in the picker's order. Each repeats so that a held chord's
    // repeats are taken too, each being the browser's tab switch as well; a repeat chooses nothing.
    {
      group: KEYS_VIEW, chords: [{ key: "1", mod: true }], repeats: true,
      does: "Main: what was posted to the channel itself. From anywhere, a message box or the search included",
      controls: ["thread-select"],
      run: (event) => viewFromKeyboard("main", event.repeat),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "2", mod: true }], repeats: true,
      does: "All: every message, thread replies included",
      controls: ["thread-select"],
      run: (event) => viewFromKeyboard("flat", event.repeat),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "3", mod: true }], repeats: true,
      does: "Thread list: a card for each thread, across the window. Enter or a click highlights one; its replies " +
        "button or → opens it, and Esc comes back to the list. Esc on the list takes the highlight off, then goes " +
        "back to the view it was chosen from",
      controls: ["thread-select"],
      run: (event) => viewFromKeyboard("threads", event.repeat),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "4", mod: true }], repeats: true,
      does: "Two columns: Main on the left and the thread list, or the thread chosen from it, on the right. " +
        "On a desk at least 1000px wide",
      controls: ["thread-select", "column-toggle"],
      run: (event) => viewFromKeyboard("columns", event.repeat),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "\\", mod: true }],
      does: "Two columns on or off, as the layout button beside the view picker does",
      controls: ["column-toggle"],
      run: () => columnsFromKeyboard(),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "[" }, { key: "]" }],
      does: "In two columns: the left column, Main, or the right one, the thread or the list of threads",
      run: (event) => columnFromKeyboard(event.key === "]"),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "ArrowDown", alt: true }, { key: "ArrowUp", alt: true }],
      does: "The next or previous channel",
      controls: ["discord-channel"],
      run: (event) => stepChannel(event),
    },
    {
      group: KEYS_MOVING, chords: [{ key: "k", mod: true }],
      does: "Choose a channel",
      controls: ["discord-channel"],
      run: () => pickChannel(),
    },
    {
      // `#538 c-key-copy`: b alone. m, the place marker's old key, is the message box's now.
      group: KEYS_MOVING, chords: [{ key: "b" }],
      does: "Back to your bookmark, and the message below it selected",
      controls: ["jump-bookmark"],
      run: () => bookmarkJumpFromKeyboard(),
    },
    {
      // `cmd-e-done`. The chord fires in a field, where e is typed. It repeats so that a held chord's
      // repeats are taken too, each being the browser's own Ctrl+E (Cmd+E) as well; a repeat marks nothing.
      // Not `everywhere`, as the search's chord is (`keyDispatch.onEverywhereKey`): that listener refuses the chord's
      // default before anything is done, and this one is the browser's while there is no message to mark;
      // and it is heard here, after `keyDispatch.macFieldEditKey`, so Ctrl+E in a field on a Mac stays the field's.
      group: KEYS_MESSAGE, chords: [{ key: "e" }, { key: "e", mod: true }], repeats: true,
      does: () => `Done, or not Done. ${afterDone() === "next"
        ? "Marked Done, on to the next message below."
        : afterDone() === "unread"
          ? "Marked Done, on to the next unread below."
          : "Marked Done, the selection stays on it."} ${modKeyName()}+E does it from a ` +
        "message box or the search too, where e is typed, with single keys on or off; the box keeps what you are typing",
      rowControls: ["done-button"],
      run: (event, target) => (event.repeat ? modHeld(event) && keyContext(target).row !== null : doneFromKeyboard(target)),
    },
    // `#295 shift-e-read-collapse`. No row button: the Done button only marks and the fold only folds,
    // and this does both. Done through here, which this key was, has no key at all now.
    {
      group: KEYS_MESSAGE, chords: [{ key: "e", shift: true }],
      does: "Read and collapse: marks the message Done, as e does, and read in the chat service too where it " +
        "can be, as a sent reply does; folds it; the selection stays on it. Under Fold read, a message that " +
        "joins a run of read messages leaves the selection on the run's “… read messages …” line; under Hide " +
        "read it goes, and the selection moves to the next message. Never puts one back. " +
        "Done through here, which this key used to be, is in the ⋯ menu only",
      run: (_event, target) => readCollapseFromKeyboard(target),
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "b", shift: true }],
      does: "Bookmark: the line above this message. On the message just below it, take it away, as the X on its " +
        "line does. Marks nothing Done",
      rowControls: ["row-bookmark-button"],
      run: (_event, target) => bookmarkFromKeyboard(target),
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "z" }],
      does: "Undo the newest thing that can still be taken back — a Done, a send dismissed from the outbox, " +
        "or a bookmark taken away — whichever was done last, as its Undo chip does. Pressed again it takes " +
        "the next one back, while its chip is still up",
      controls: ["undo-dismiss", "undo-send-dismiss", "undo-bookmark"],
      run: () => undoFromKeyboard(),
    },
    // `#443 undo-key-general`. The same undo on the chord every other program uses for it. Chrome has
    // no page-level Ctrl+Z of its own, so nothing is given up for it — except in a text field, where
    // the chord is the browser's undo of what was typed and this binding steps aside for it.
    {
      group: KEYS_MESSAGE, chords: [{ key: "z", mod: true }],
      does: () => `Undo, as z. In a message box or the search, ${modKeyName()}+Z stays the browser's ` +
        "undo of what was typed",
      controls: ["undo-dismiss", "undo-send-dismiss", "undo-bookmark"],
      run: (_event, target) => (isTextField(target) ? false : undoFromKeyboard()),
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "r" }],
      does: "Reply, in the list: its replies gathered under it and the box under them, the list's own box " +
        "hidden until the reply is sent or put down. With words of it selected, Quote reply: the reply starts " +
        "with them quoted",
      rowControls: ["reply-button"],
      run: (_event, target) => replyFromKeyboard(target),
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "q" }],
      does: "Quote reply, with words of a message selected: the reply starts with them quoted, as the Quoted " +
        "reply button does. With nothing selected, nothing",
      controls: ["selection-chip"],
      run: () => quoteFromKeyboard(),
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "p" }],
      does: "Pin or unpin",
      rowControls: ["row-pin-button"],
      run: (_event, target) => pressRowButton(target, "row-pin-button") !== null,
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "s" }],
      does: "Read it aloud, or stop",
      run: (_event, target) => readFromKeyboard(target),
    },
    {
      // `#451 read-strip-followups`. The grip's own handler (`onReadGripKey`), listed so the list is
      // whole: focused on the grip, its keys never reach this table (it refuses their default, which
      // `keyDispatch.onPageKey` leaves alone), and off the grip the arrows are the list's, above.
      group: KEYS_MESSAGE, by: "grip",
      chords: [{ key: "ArrowLeft" }, { key: "ArrowRight" }, { key: "ArrowUp" }, { key: "ArrowDown" },
        { key: "ArrowLeft", shift: true }, { key: "ArrowRight", shift: true },
        { key: "ArrowUp", shift: true }, { key: "ArrowDown", shift: true }],
      does: "On the reading controls' grip, on a desk: move the reading controls; with Shift, by more. " +
        "The place is remembered on this device",
      controls: ["read-steer-grip"],
    },
    {
      group: KEYS_MESSAGE, by: "grip", chords: [{ key: "Home" }],
      does: "On the reading controls' grip: Home docks it on the chips row again, as a double-click does",
      controls: ["read-steer-grip"],
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "." }],
      does: "Its ⋯ menu; ↑ and ↓ move through it",
      rowControls: ["row-more-button"],
      run: (_event, target) => menuFromKeyboard(target),
    },
    {
      group: KEYS_MESSAGE, chords: [{ key: "i" }],
      does: "Who wrote it, when, and its id",
      run: (_event, target) => detailsFromKeyboard(target),
    },
    // `#299 all-thread-strip`. Its strip, pressed.
    {
      group: KEYS_MESSAGE, chords: [{ key: "t" }],
      does: "In All, select its thread, as a click on the colour strip at its left edge does: the messages not in " +
        "it dim, and All stays All. Again, or Esc, lets go",
      rowControls: ["thread-strip"],
      run: (_event, target) => threadStripFromKeyboard(target),
    },
    {
      // `#296 copy-highlighted-message`. y was free: no binding had it, and as a bare letter it never
      // fires in a text field, where it is typed. `#538 c-key-copy`, the owner: "if the text entry box is
      // not highlighted and I press the C key, I would like that to just copy, to be even lazier than
      // pressing Command-C." c was the message box's, which has Ctrl+S (Cmd+S) and now m.
      group: KEYS_MESSAGE, chords: [{ key: "c", mod: true }, { key: "c" }, { key: "y" }],
      does: () => "Copy the highlighted message: all of its text as written, a summarised or folded one's too; on a " +
        `thread card, its first message. With text selected, c and y copy that, and ${modKeyName()}+C copies it as usual`,
      rowControls: ["row-copy-button"],
      // The chord leaves its default to the browser whatever it finds, since that default is what fires the
      // `copy` event the message goes by (`highlightCopy.copyChordFromKeyboard`).
      run: (event) => (modHeld(event) ? copyChordFromKeyboard() : copyFromKeyboard()),
    },
    {
      // `#408 copy-upstream-link`. l was free as a bare letter (Shift+L is the links filter), and
      // as one it never fires in a text field. On a row with no link to copy the key does
      // nothing, as the menu offers nothing. The menu's item for the address is the link itself
      // now (`#409 open-upstream-link`), which a pointer can copy from; this key stays the
      // keyboard's way to the same copy.
      group: KEYS_MESSAGE, chords: [{ key: "l" }],
      does: () => `Copy its link in ${manyChatServices() ? "its chat service" : chatServiceName()}: the message's own address there, where the server could form one`,
      rowControls: ["row-copy-link-button"],
      run: (_event, target) => copyLinkFromKeyboard(target),
    },
    {
      // `#889 voice-page-copy-link-key`. Ctrl+L (Cmd+L): this app's own link to the message, the ⋯ menu's "Copy
      // vibe-talk link". The owner, 2026-11-05: "Let's ad Cmd-l for copying the vibe talk link instead of the slack
      // link"; bare l stays the service's link. Not in a text field, where it is the field's and the browser's. In a
      // browser tab the chord may be the address bar's before the page hears it; in the installed app it is the
      // page's alone. So Ctrl+Shift+L (Cmd+Shift+L) does the same, and the list names it where there is a tab. A held
      // chord's repeats are taken too, as Ctrl+S's are: each would be the address bar's otherwise. Copying again is
      // the same link.
      group: KEYS_MESSAGE, chords: [{ key: "l", mod: true }, { key: "l", mod: true, shift: true }], repeats: true,
      does: () => "Copy its vibe-talk link: the address that opens this app on this message, as the ⋯ menu's Copy " +
        "vibe-talk link does" + (standalone() ? "" : `. In a browser tab, where ${modKeyName()}+L can go to the ` +
        `address bar, ${modKeyName()}+Shift+L does the same`),
      rowControls: ["row-copy-deep-link-button"],
      run: (_event, target) => deepLinkFromKeyboard(target),
    },
    {
      // `#480 fold-one-message-key`. Shift+O: bare o shrinks or expands the message here, its
      // shifted form folds it away altogether into a "… read messages …" line and steps above
      // it, for closing up one thread at a time. Marks it read, as dealt with.
      group: KEYS_MESSAGE, chords: [{ key: "o", shift: true }],
      does: () => `Fold it away into a "… read messages …" line and step to the message above, marking it read: press again and again to close up a thread from the bottom; Enter on the line opens it again`,
      run: (_event, target) => foldOneFromKeyboard(target),
    },
    {
      // `#409 open-upstream-link`, moved from Shift+O by `#480 fold-one-message-key`. Cmd+O (or
      // Ctrl+O): the ⋯ menu's "Open in …" link, pressed, so the opening is the link's own,
      // exactly as a tap on it — and the browser's own Open File does not come up over it. On a
      // row with no address the link is not there to press and the key is left alone, as l is.
      group: KEYS_MESSAGE, chords: [{ key: "o", mod: true }],
      does: () => `Open it in ${manyChatServices() ? "its chat service" : chatServiceName()}, in a new tab: the same address Copy its link copies, where the server could form one`,
      rowControls: ["row-open-link-button"],
      // `pressRowButton` answers "took" where it took the row's selection but found no link:
      // that is this key not acted on, not an opening, so it is left to the browser like any
      // unbound key rather than swallowed in silence.
      run: (_event, target) => {
        const pressed = pressRowButton(target, "row-open-link-button");
        return pressed !== null && pressed !== "took";
      },
    },
    {
      group: KEYS_VIEW, chords: [{ key: "h" }],
      does: "Show read, Fold read, Hide read, in turn",
      controls: ["todo-filter"],
      run: () => readModeFromKeyboard(),
    },
    // `#404 threads-unpack-all`. u was free; it acts only over the Threads view's cards, as its chip does.
    {
      group: KEYS_VIEW, chords: [{ key: "u" }],
      does: "In the Thread list, Unpack, in turn: Pack all, Unpack unread (the threads with an unread message " +
        "unpacked under their cards, the read ones packed and greyed), Unpack all. It gives every thread back to " +
        "it. The read modes apply inside each thread",
      controls: ["side-unpack"],
      run: () => unpackFromKeyboard(),
    },
    // `#440 thread-card-pack-toggle`. ^ was free; over the Threads view only, as the triangle is.
    {
      group: KEYS_VIEW, chords: [{ key: "^" }],
      does: "In the Thread list, pack the thread of the selected card or message back into its card, the card " +
        "kept where it is, as its triangle does. Enter or o on a card goes round collapsed, whole and unpacked",
      run: (_event, target) => packFromKeyboard(target),
    },
    // `#419 threads-view-main-messages`. a was free; it acts only over the Threads view's cards, as its chip does.
    {
      group: KEYS_VIEW, chords: [{ key: "a" }],
      does: "In the Thread list, Main channel: the main channel's messages that are not threads yet among the " +
        "threads, and its message box at the foot; Thread or Reply on one makes it a thread where it is",
      controls: ["side-main"],
      run: () => mainRowsFromKeyboard(),
    },
    // `#426 threads-sort-toggle`. d was free; it acts only over the Threads view's cards, as its chip does.
    {
      group: KEYS_VIEW, chords: [{ key: "d" }],
      does: () => `In the Thread list, the order of the cards, in turn: ${threadOrderWords()}. ` +
        "Main channel's messages stand among them by when they were posted",
      controls: ["side-order"],
      run: () => orderFromKeyboard(),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "p", shift: true }],
      does: "Only the pinned messages, or all of them again",
      controls: ["pinned-filter"],
      run: () => filterFromKeyboard("pinned"),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "l", shift: true }],
      does: "Only the links, or the messages again",
      controls: ["links-filter"],
      run: () => filterFromKeyboard("links"),
    },
    {
      group: KEYS_VIEW, chords: [{ key: "m", shift: true }],
      does: "Only your messages, then only everyone else's, then everyone's again",
      controls: ["author-filter"],
      run: () => filterFromKeyboard("author"),
    },
    // `#301 semicolon-expand-cycle`. Not the keys of the Expand all and Collapse all chips any more: a press
    // does what one of them does only one time in three, so neither chip names them in `aria-keyshortcuts`,
    // and no other control is what they press. The chords are the same steps from anywhere, at the owner's
    // word, a message box and the search included; they repeat only so that a held chord's repeats are
    // taken from the browser, and step nothing (`foldStepFromKeyboard`).
    {
      group: KEYS_VIEW, chords: [{ key: ";" }, { key: ";", mod: true }], repeats: true,
      does: () => "The long messages, a step a press: expand all, then open the unread and fold the read, then " +
        "collapse all, and round again. Each list, in either column, goes on from the step it was left at; the first " +
        "press in one expands all, or if they are all open already goes on round to the first step that changes " +
        `something. The message you are on stays where it is, as far as the list allows. ${modKeyName()}+; works from ` +
        "anywhere, a message box or the search included, on the list of the column you are in; pressed in a message " +
        "box, the box stays where it is",
      run: (event, target) => foldStepFromKeyboard(1, event, target),
    },
    {
      group: KEYS_VIEW, chords: [{ key: ":" }, { key: ":", mod: true }], repeats: true,
      does: () => "The same steps the other way round: the first press in a list collapses all, or if they are all " +
        `folded already goes on round to the first step that changes something. ${modKeyName()}+: ` +
        `(${modKeyName()}+Shift+;) from anywhere, as ${modKeyName()}+; does`,
      run: (event, target) => foldStepFromKeyboard(-1, event, target),
    },
    // `#425 collapsed-lines`. Ctrl+( and Ctrl+) — Ctrl+Shift+9 and Ctrl+Shift+0 where the brackets sit
    // there, matched by the character typed and, where Shift+9 types no bracket, by the place of the
    // key (`parenByPlace`). A chord nobody hits by accident while reading, as the owner asked. It
    // repeats so a held chord's every repeat is taken from the browser's zoom, and steps on until
    // the end of the range; in a text field it is never taken at all (`collapsedLinesFromKeyboard`).
    {
      group: KEYS_VIEW, chords: [{ key: "(", mod: true }], repeats: true,
      does: () => `One line fewer in every collapsed message everywhere: ${minCollapsedLines()} to ` +
        `${maxCollapsedLines()} lines, ${minCollapsedLines()} the header alone. ${modKeyName()}+( is ` +
        `${modKeyName()}+Shift+9. Kept on the server for every device, as Collapsed message lines in Settings, and ` +
        "never taken in a text field, where it types its bracket",
      controls: ["collapsed-lines"],
      run: (_event, target) => collapsedLinesFromKeyboard(-1, target),
    },
    {
      group: KEYS_VIEW, chords: [{ key: ")", mod: true }], repeats: true,
      does: () => `One line more, as ${modKeyName()}+( shows one fewer. ${modKeyName()}+) is ` +
        `${modKeyName()}+Shift+0`,
      controls: ["collapsed-lines"],
      run: (_event, target) => collapsedLinesFromKeyboard(1, target),
    },
    {
      // `#538 c-key-copy`: m, the owner's pick, c being the copy now.
      group: KEYS_WRITING, chords: [{ key: "m" }],
      does: "Write to the channel, or in the thread you are in; with a reply open, the reply; in the call view, to the agent",
      controls: ["channel-compose-text", "reply-text", "compose-text"],
      run: () => composeFromKeyboard(),
    },
    {
      // `#403 composer-key-global`. Heard first, at the window, so nothing the focus is on keeps it
      // from the box (`keyDispatch.onEverywhereKey`), and a held chord's repeats are refused there too: each is the
      // browser's Save page as well. The box is focused once.
      group: KEYS_WRITING, chords: [{ key: "s", mod: true }], everywhere: true,
      does: "The message box, from anywhere: typing in the search, on a button, a thread card, Settings, Help or " +
        "the thread list too. In two columns, the box of the column you are in, or of the one with the selected " +
        "message; Main's from the thread cards. With a reply open, its box, the one box drawn. It never changes " +
        "the view when the view has a box: in the Thread list with Main channel on, the box at the foot of the " +
        "cards; with neither that nor a reply there, back to the view the list was chosen from. " +
        "Never the Save page of the browser",
      controls: ["channel-compose-text", "reply-text", "compose-text"],
      repeats: true,
      run: (event) => {
        if (!event.repeat) writeFromKeyboard();
        return true;
      },
    },
    {
      group: KEYS_WRITING, by: "box", chords: [{ key: "Enter" }, { key: "Enter", shift: true }],
      does: "In a message box: send, or a new line. Which is which is Enter to send in Settings",
    },
    {
      group: KEYS_WRITING, chords: [{ key: "Enter", mod: true }],
      does: () => `In a message box: send, whatever Enter does. Anywhere else, the message box, as ${modKeyName()}+S does`,
      // In a message box the box's own handler sends (`onComposerKey`) and the key never gets here. In
      // another field it is that field's, and on a link it is the browser's (open it in a new tab).
      run: (_event, target) => !isTextField(target) && !isLink(target) && writeFromKeyboard(),
    },
    {
      group: KEYS_WRITING, chords: [{ key: "ArrowUp" }], inField: true,
      does: "In the box of the channel while it is empty: the newest message",
      run: (_event, target) => newestFromComposer(target),
    },
  ];
}

// The message boxes' growth and their Enter (`#225 enter-to-send`, `paste-never-sends`,
// `#575 mobile-keyboard-compose`, `#224 keyboard-shortcuts`; `#624 voice-page-modularity`,
// moved from the legacy script into view/composer for the plan's step 17): whether the boxes
// grow with their text and every pass that draws or sizes them, which list each box is in,
// what Enter does in the two places it can be said, and what a box last took as a paste. The
// growth itself (`createComposerGrow`), the boxes' keys (`createComposerKeys`) and the choice
// of whether Enter sends (`enterSends`) stay theirs; the lists, the storage and the setting
// are handed in. The words are the page's.

import { createComposerGrow } from "./grow.ts";
import type { ComposerGrow } from "./grow.ts";
import { COMPOSER_FIELDS, watchBoxInsertions as watchInsertions } from "./keys.ts";
import { nodeWithin } from "../dom/index.ts";

/** A message box, as the boxes' growth and Enter read it. */
export interface ComposerBox extends HTMLElement {
  id: string;
}

/** What the message boxes are handed by the page. */
export interface ComposerBoxesDependencies {
  el(id: string): HTMLElement & { checked?: boolean };
  leftEl(id: string): HTMLElement;
  replyScroller(): HTMLElement;
  atBottom(area: HTMLElement, slack?: number): boolean;
  followFoot(area: HTMLElement): void;
  /** Move `area` by a measured delta, and put it back at a measured place, through the scroll owner (`#1251`). */
  nudge(area: HTMLElement, delta: number): void;
  toPosition(area: HTMLElement, top: number): void;
  activeElement(): Element | null;
  /** Which screen is up (`screens.screen`). */
  screen(): string;
  bottomSlack: number;
  fieldSizing: boolean;
  keepGrow(on: boolean): void;
  storedGrow(): boolean;
  enterSendsCurrent(): boolean;
  platform(): string;
  noteInsertion(box: unknown, event: unknown): void;
}

/** The message boxes' growth and Enter, as the page calls them. */
export interface ComposerBoxes {
  readonly composerGrowth: ComposerGrow;
  storedComposerGrow(): boolean;
  applyComposerGrow(on: boolean, grow: HTMLElement): void;
  renderComposerGrow(on: boolean): void;
  growShown(field: ComposerBox): boolean;
  growingBoxList(field: ComposerBox): HTMLElement;
  fitGrowingBox(field: ComposerBox): void;
  fitGrowingBoxes(): void;
  keepGrowingBoxInView(field: ComposerBox): void;
  modKeyName(): string;
  sendChordName(): string;
  renderEnterSends(): void;
  noteBoxInsertion(box: unknown, event: unknown): void;
  watchBoxInsertions(box: EventTarget): void;
  /** Bring `box` into `area` whole (`composerGrowth.keepBoxInView`). */
  keepBoxInView(box: HTMLElement, area: HTMLElement): void;
  /** The box in `area` the reader is writing in, or null. */
  boxBeingWritten(area: HTMLElement): HTMLElement | null;
  /**
   * Keep the box being written in `area` in view as the list shrinks from `seen.list` to
   * `now.list`, if it was in view; whether it did.
   */
  keepWrittenBox(area: HTMLElement, seen: { list: number } | null, now: { list: number }): boolean;
}

/**
 * The platforms whose keyboards say Cmd where the rest say Ctrl, as `navigator.platform` begins
 * for them ("MacIntel", "iPhone", "iPad", "iPod"). An iPad reporting itself as a Mac lands in
 * the same answer, which is the right one: its keyboard says Cmd either way.
 */
const CMD_KEY_PLATFORMS = ["Mac", "iPhone", "iPad", "iPod"];

/** Make the message boxes' growth and Enter from the page's lists, storage and setting. */
export function createComposerBoxes(deps: ComposerBoxesDependencies): ComposerBoxes {
  /**
   * The list `field` is in: the right column's for its box, else the left's; for the reply's, the list it
   * is open in (`#410 inline-reply-editor`).
   */
  const growingBoxList = (field: ComposerBox): HTMLElement => field.id === "reply-text"
    ? deps.replyScroller()
    : deps.leftEl(field.id === "side-compose-text" ? "side-scroll" : "scroll-area");

  const composerGrowth = createComposerGrow({
    lookup: (id) => deps.leftEl(id),
    listOf: (field) => growingBoxList(field as ComposerBox),
    atBottom: (area, slack) => (slack === undefined ? deps.atBottom(area) : deps.atBottom(area, slack)),
    followFoot: (area) => deps.followFoot(area),
    nudge: (area, delta) => deps.nudge(area, delta),
    toPosition: (area, top) => deps.toPosition(area, top),
    activeElement: () => deps.activeElement(),
    bottomSlack: deps.bottomSlack,
    fieldSizing: deps.fieldSizing,
    keep: (on) => deps.keepGrow(on),
  });

  /**
   * `#429 keyboard-hides-bar`. The boxes in the lists a reader writes in, each field with the box it is in:
   * the channel's (in Main's list, or among the cards since `#419 threads-view-main-messages`), the right
   * column's, and a reply open in the stream (`#410 inline-reply-editor`), in whichever list holds it. The
   * call's is `writingToAgent`'s.
   */
  const LIST_BOXES: readonly (readonly [string, string])[] = [
    ["channel-compose-text", "channel-composer"],
    ["side-compose-text", "side-composer"],
    ["reply-text", "column-reply"],
    ["reply-text", "side-reply"],
  ];

  /**
   * The box in `area` the reader is writing in, or null. `#429 keyboard-hides-bar`: the on-screen keyboard
   * takes its height from the list, and a list that shrinks keeps its scroll position, so the channel's box
   * at its foot, or a reply's under its message, went below the window, Send with it, as the call's had
   * before `#241 voice-composer` kept it in view. The keepers of the two lists bring this box into view
   * whole as their list shrinks, if it was in view before (`boxWasInView`): a reader who scrolled up to read
   * the message being answered is left there.
   */
  function boxBeingWritten(area: HTMLElement): HTMLElement | null {
    if (deps.screen() !== "main") return null;
    const held = deps.activeElement();
    for (const [field, box] of LIST_BOXES) {
      if (held === deps.leftEl(field) && nodeWithin(area, deps.leftEl(box))) return deps.leftEl(box);
    }
    return null;
  }

  /**
   * Whether any of `box` was in the view of `area` while the list was `height` tall. The list shrinks from
   * its foot with its scroll position kept, so where the box stands below the list's top is where it stood.
   */
  function boxWasInView(box: HTMLElement, area: HTMLElement, height: number): boolean {
    const rect = box.getBoundingClientRect();
    const top = rect.top - area.getBoundingClientRect().top;
    return top < height && top + rect.height > 0;
  }

  /**
   * `#429 keyboard-hides-bar`. Keep the box being written in `area` in view as the list shrinks from
   * `seen.list` to `now.list`, if it was in view; whether it did.
   */
  function keepWrittenBox(area: HTMLElement, seen: { list: number } | null, now: { list: number }): boolean {
    if (seen === null || seen.list <= 0 || now.list >= seen.list) return false;
    const box = boxBeingWritten(area);
    if (box === null || !boxWasInView(box, area, seen.list)) return false;
    composerGrowth.keepBoxInView(box, area);
    return true;
  }

  /** What was stored. ABSENT MEANS OFF; a browser that refuses storage reads as nothing stored. */
  function storedComposerGrow(): boolean {
    return deps.storedGrow();
  }

  /**
   * Grow ticked or unticked by the reader, on the checkbox `grow`: kept, and drawn (`composerGrowth`). Each
   * list the reader was at the foot of is put back at its foot once the boxes in it are drawn at their new
   * height; the checkbox just ticked, if any of it was on screen, is kept where it was, under the pointer.
   */
  function applyComposerGrow(on: boolean, grow: HTMLElement): void {
    composerGrowth.apply(on, grow);
  }

  /** Draw the choice on #screen-main and every Grow checkbox (`composerGrowth`). */
  function renderComposerGrow(on: boolean): void {
    composerGrowth.render(on);
  }

  /** Whether the composer `field` is in draws its Grow checkbox, which web/voice.css does on a desk (`composerGrowth`). */
  function growShown(field: ComposerBox): boolean {
    return composerGrowth.shown(field);
  }

  /** Where the browser cannot size a box to its text, size it here (`composerGrowth`). */
  function fitGrowingBox(field: ComposerBox): void {
    composerGrowth.fit(field);
  }

  /** `fitGrowingBox` for each of the boxes. */
  function fitGrowingBoxes(): void {
    composerGrowth.fitAll();
  }

  /**
   * A growing box changed size: with the reader typing in it, its whole composer brought into its list
   * (`composerGrowth`).
   */
  function keepGrowingBoxInView(field: ComposerBox): void {
    composerGrowth.keepInView(field);
  }

  /**
   * The chord modifier, in the word printed on the reader's own keyboard: "Cmd" or "Ctrl".
   *
   * WORDING ONLY. Every chord this page answers takes either key on every platform — `onComposerKey`
   * and `keyDispatch.onPageKey` read `ctrlKey || metaKey` and never this — so a platform guessed wrong costs a
   * hint that names the other key, not a key that stops working. `navigator.platform` rather than the
   * user-agent string, which this page does not read for anything (the suite refuses it, because a
   * layout decided by it rots). `#224 keyboard-shortcuts` names every chord with it.
   */
  function modKeyName(): string {
    const platform = String(deps.platform() || "");
    return CMD_KEY_PLATFORMS.some((prefix) => platform.startsWith(prefix)) ? "Cmd" : "Ctrl";
  }

  /** The send chord, in the words printed on the reader's own keyboard. See `modKeyName`. */
  function sendChordName(): string {
    return `${modKeyName()}+Enter`;
  }

  /**
   * Say what Enter does, in the two places it can be said without costing the phone a line.
   *
   *   * The Settings hint, under the switch: with it off, the key that sends instead. That is where
   *     the owner asked for it — "that should be indicated in the settings where the user disables
   *     it" — and the one place a reader who has just turned Enter off is looking.
   *   * Each box's `enterkeyhint`, which is what a phone's keyboard draws on its own return key: a
   *     send arrow while Enter sends, a return arrow while it is a new line. The key itself is the
   *     hint, so no placeholder or extra line under a box has to carry it. With it the box keeps
   *     `inputmode="text"` (`ios-keyboard-send`): the pair is what WebKit reads for the return
   *     key, and stating the text mode keeps a box whose mode dictation put down (`"none"`, which
   *     this saying never undoes) the only one without a keyboard. WebKit still draws a return,
   *     not Send, in a multiline box on some phones; the Send at the keyboard's top
   *     (view/composer's keyboard) is the one that never depends on that drawing.
   */
  function renderEnterSends(): void {
    const sends = deps.enterSendsCurrent();
    (deps.el("enter-sends") as HTMLElement & { checked: boolean }).checked = sends;
    deps.el("enter-sends-hint").textContent = sends
      ? "Shift+Enter starts a new line, and on a phone's keyboard so does ↵ in the box's corner."
      : `${sendChordName()} sends. Enter starts a new line.`;
    for (const id of COMPOSER_FIELDS) {
      const box = deps.el(id) as HTMLElement & { getAttribute?: (name: string) => string | null };
      box.setAttribute("enterkeyhint", sends ? "send" : "enter");
      const mode = typeof box.getAttribute === "function" ? box.getAttribute("inputmode") : null;
      if (mode !== "none") box.setAttribute("inputmode", "text");
    }
  }

  /** A message box's `beforeinput`, `input` or `paste`, noted (`composerKeys`). `paste-never-sends`. */
  function noteBoxInsertion(box: unknown, event: unknown): void {
    deps.noteInsertion(box, event);
  }

  /** Listen to `box` for what `noteBoxInsertion` notes. Each message box, as it is given its Enter. */
  function watchBoxInsertions(box: EventTarget): void {
    watchInsertions(box, noteBoxInsertion);
  }

  return {
    composerGrowth, storedComposerGrow, applyComposerGrow, renderComposerGrow, growShown,
    growingBoxList, fitGrowingBox, fitGrowingBoxes, keepGrowingBoxInView, modKeyName,
    sendChordName, renderEnterSends, noteBoxInsertion, watchBoxInsertions,
    keepBoxInView: (box, area) => composerGrowth.keepBoxInView(box, area), boxBeingWritten, keepWrittenBox,
  };
}

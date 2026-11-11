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
  keepBoxInView(box: HTMLElement, area: HTMLElement): void;
  activeElement(): Element | null;
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
    keepBoxInView: (box, area) => deps.keepBoxInView(box, area),
    activeElement: () => deps.activeElement(),
    bottomSlack: deps.bottomSlack,
    fieldSizing: deps.fieldSizing,
    keep: (on) => deps.keepGrow(on),
  });

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
   *     hint, so no placeholder or extra line under a box has to carry it.
   */
  function renderEnterSends(): void {
    const sends = deps.enterSendsCurrent();
    (deps.el("enter-sends") as HTMLElement & { checked: boolean }).checked = sends;
    deps.el("enter-sends-hint").textContent = sends
      ? "Shift+Enter starts a new line, and on a phone's keyboard so does ↵ in the box's corner."
      : `${sendChordName()} sends. Enter starts a new line.`;
    for (const id of COMPOSER_FIELDS) {
      deps.el(id).setAttribute("enterkeyhint", sends ? "send" : "enter");
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
  };
}

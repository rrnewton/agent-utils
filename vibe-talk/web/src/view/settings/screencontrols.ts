// The screen's sliders and their steps, the page-build notice's reload, and the reading column's grip (the reading
// width and the text size, `#425 collapsed-lines`, the cards' line cap, the quoted read's share, `#529
// page-build-stale-notice`; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/settings for
// the plan's step 17). What each does is its own owner's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface ScreenControlsDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The reading column's width, from its slider. */
  readingWidthChanged(value: string): unknown;
  /** The messages' text size, from its slider, and a step smaller or larger. */
  textSizeChanged(value: string): unknown;
  stepTextSize(direction: -1 | 1): unknown;
  /** How many lines a long message shows folded, from its slider, and a step. */
  collapsedLinesChanged(value: string): unknown;
  stepCollapsedLines(direction: -1 | 1): unknown;
  /** The most lines a thread card shows, from its slider, and a step. */
  cardMaxLinesChanged(value: string): unknown;
  stepCardMaxLines(direction: -1 | 1): unknown;
  /** `#529 page-build-stale-notice`. Load the page again, only on the reader's press; and say which build is shown. */
  reloadPage(): unknown;
  showPageBuild(): unknown;
  /** How much of a quoted message is read aloud, from its slider, and a step. */
  quoteReadPercentChanged(value: string): unknown;
  stepQuoteReadPercent(direction: -1 | 1): unknown;
  /** The first share of a long message, and how long is long, from their sliders and steps. */
  markReadFirstPercentChanged(value: string): unknown;
  stepMarkReadFirstPercent(direction: -1 | 1): unknown;
  markReadLongWordsChanged(value: string): unknown;
  stepMarkReadLongWords(direction: -1 | 1): unknown;
  /** The reading column's grip: dragged by a pointer, let go, or moved by the keys. */
  gripDown(event: PointerEvent): unknown;
  gripMove(event: PointerEvent): unknown;
  gripUp(): unknown;
  gripKey(event: KeyboardEvent): unknown;
}

/** The listeners, mounted. */
export interface ScreenControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the screen's controls. The page build is said where it was, between the line cap's slider and its steps. */
export function mountScreenControls(deps: ScreenControlsDependencies): ScreenControls {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  /** A slider's value as it is now, read when it moves. */
  const value = (id: string): string => (el(id) as HTMLInputElement).value;
  listen<Event>(el("reading-width"), "input", () => deps.readingWidthChanged(value("reading-width")));
  listen<Event>(el("msg-scale"), "input", () => deps.textSizeChanged(value("msg-scale")));
  listen<Event>(el("msg-scale-smaller"), "click", () => deps.stepTextSize(-1));
  listen<Event>(el("msg-scale-larger"), "click", () => deps.stepTextSize(1));
  listen<Event>(el("collapsed-lines"), "input", () => deps.collapsedLinesChanged(value("collapsed-lines")));
  listen<Event>(el("collapsed-lines-smaller"), "click", () => deps.stepCollapsedLines(-1));
  listen<Event>(el("collapsed-lines-larger"), "click", () => deps.stepCollapsedLines(1));
  listen<Event>(el("card-max-lines"), "input", () => deps.cardMaxLinesChanged(value("card-max-lines")));
  listen<Event>(el("page-update"), "click", () => deps.reloadPage());
  deps.showPageBuild();
  listen<Event>(el("card-max-lines-smaller"), "click", () => deps.stepCardMaxLines(-1));
  listen<Event>(el("card-max-lines-larger"), "click", () => deps.stepCardMaxLines(1));
  listen<Event>(el("quote-read-percent"), "input", () => deps.quoteReadPercentChanged(value("quote-read-percent")));
  listen<Event>(el("quote-read-percent-smaller"), "click", () => deps.stepQuoteReadPercent(-1));
  listen<Event>(el("quote-read-percent-larger"), "click", () => deps.stepQuoteReadPercent(1));
  listen<Event>(el("mark-read-first-percent"), "input", () => deps.markReadFirstPercentChanged(value("mark-read-first-percent")));
  listen<Event>(el("mark-read-first-percent-smaller"), "click", () => deps.stepMarkReadFirstPercent(-1));
  listen<Event>(el("mark-read-first-percent-larger"), "click", () => deps.stepMarkReadFirstPercent(1));
  listen<Event>(el("mark-read-long-words"), "input", () => deps.markReadLongWordsChanged(value("mark-read-long-words")));
  listen<Event>(el("mark-read-long-words-smaller"), "click", () => deps.stepMarkReadLongWords(-1));
  listen<Event>(el("mark-read-long-words-larger"), "click", () => deps.stepMarkReadLongWords(1));
  const grip = el("width-grip");
  listen<PointerEvent>(grip, "pointerdown", (event) => deps.gripDown(event));
  listen<PointerEvent>(grip, "pointermove", (event) => deps.gripMove(event));
  listen<PointerEvent>(grip, "pointerup", () => deps.gripUp());
  listen<PointerEvent>(grip, "pointercancel", () => deps.gripUp());
  listen<KeyboardEvent>(grip, "keydown", (event) => deps.gripKey(event));
  return { unmount };
}

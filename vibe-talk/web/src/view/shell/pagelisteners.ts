// The page's own listeners at the document and the window (`#260 timestamp-hover`, `#479 message-time-full-date`,
// `#189 restore-ui-state`, `#330 voice-long-turn-drop`; `#624 voice-page-modularity`, moved from the legacy script's
// wiring into view/shell for the plan's step 17): a message's time worded as the pointer or the focus reaches it and
// its day said on a tap or Enter, and the reader's place and a turn still being said written down in the last
// moments a phone promises the page. Mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface PageListenersDependencies {
  /** Word a message's time for its day, as the pointer or the focus arrives at it. */
  wordDayTitle(event: Event): void;
  /** A tap on a time, or Enter on it, says its day: heard ahead of the row's own press. */
  onTimeTap(event: MouseEvent): void;
  onTimeKey(event: KeyboardEvent): void;
  /** Whether the page is hidden now. */
  hidden(): boolean;
  /** Write down the reader's place (`#189 restore-ui-state`). */
  rememberUiState(): void;
  /** Write down a turn still being said (`#330 voice-long-turn-drop`). */
  saveUnsettledTurns(): void;
}

/** The listeners, mounted. */
export interface PageListeners {
  /** Take every listener down again. */
  unmount(): void;
}

/**
 * Mount the page's listeners. Hidden comes first on a phone, and is the one an installed app reliably gets before
 * it is killed; being unloaded is the other. The place is written down before the turn, at each.
 */
export function mountPageListeners(deps: PageListenersDependencies): PageListeners {
  const { listen, unmount } = createListening();
  listen<Event>(document, "pointerover", (event) => deps.wordDayTitle(event));
  listen<Event>(document, "focusin", (event) => deps.wordDayTitle(event));
  listen<MouseEvent>(document, "click", (event) => deps.onTimeTap(event), { capture: true });
  listen<KeyboardEvent>(document, "keydown", (event) => deps.onTimeKey(event), { capture: true });
  listen<Event>(document, "visibilitychange", () => {
    if (deps.hidden()) deps.rememberUiState();
  });
  listen<Event>(window, "pagehide", () => deps.rememberUiState());
  listen<Event>(document, "visibilitychange", () => {
    if (deps.hidden()) deps.saveUnsettledTurns();
  });
  listen<Event>(window, "pagehide", () => deps.saveUnsettledTurns());
  return { unmount };
}

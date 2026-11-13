// The reading preferences' switches (the read-aloud pace's popover, `#386 hide-empty-messages`, `#242 rich-headers`,
// `#543 in-app-message-links`, `#580 open-in-chat-jump`, `#564 message-summary-toggle`, `#225 enter-to-send`, and Your
// own messages; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/settings for the plan's
// step 17). What each switch does is its owner's (`createMessagePreferences`, `mountOwnReadApply`); mounted once, in
// the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface PreferenceSwitchesDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The pace popover opened if shut, shut if open. */
  toggleSpeedPopover(): unknown;
  /** Your own messages: the choice applied to what is shown, or undone, and the switch changed. */
  applyOwnRead(): unknown;
  undoOwnRead(): unknown;
  markOwnReadChanged(): unknown;
  /** Each switch changed: empty messages, combining, rich headers, in-app links, open in view, summaries. */
  hideEmptyChanged(): unknown;
  combineChanged(): unknown;
  richHeadersChanged(): unknown;
  inAppLinksChanged(): unknown;
  openInViewChanged(): unknown;
  afterDoneChanged(): unknown;
  summariesChanged(): unknown;
  /** `#225 enter-to-send`. Whether Enter sends, as the switch now says. */
  enterSendsChanged(on: boolean): unknown;
  /** Mark read on reply: the No/Yes choice and each guard below it. */
  markReadChanged(): unknown;
  markReadQuoteGuardChanged(): unknown;
  markReadFoldGuardChanged(): unknown;
  markReadNonquoteChanged(): unknown;
  markReadFinalGuardChanged(): unknown;
  markReadFirstGuardChanged(): unknown;
}

/** The listeners, mounted. */
export interface PreferenceSwitches {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the reading preferences' switches in the page's order. */
export function mountPreferenceSwitches(deps: PreferenceSwitchesDependencies): PreferenceSwitches {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  listen<Event>(el("read-speed"), "click", () => deps.toggleSpeedPopover());
  listen<Event>(el("own-read-apply"), "click", () => deps.applyOwnRead());
  listen<Event>(el("own-read-undo"), "click", () => deps.undoOwnRead());
  listen<Event>(el("mark-own-read"), "change", () => deps.markOwnReadChanged());
  // `#386 hide-empty-messages`. Each column drawn again from what it holds, as for Your own messages: an empty message
  // comes back, or goes, with no read. The page without threads holds no store to draw from: turned on, its rows are
  // regrouped, which leaves the empty ones out; turned off, its newest page is read again in place, and an empty
  // message further back comes back when that history is re-read.
  listen<Event>(el("hide-empty-messages"), "change", () => deps.hideEmptyChanged());
  listen<Event>(el("combine-messages"), "change", () => deps.combineChanged());
  // `#242 rich-headers`. Nothing to redraw: the root's attribute moves every heading between the header and the words
  // it came from at once, in both columns.
  listen<Event>(el("rich-headers"), "change", () => deps.richHeadersChanged());
  // `#543 in-app-message-links`. Each column drawn again from what it holds, its links decided anew.
  listen<Event>(el("in-app-links"), "change", () => deps.inAppLinksChanged());
  // `#580 open-in-chat-jump`.
  listen<Event>(el("open-in-view"), "change", () => deps.openInViewChanged());
  // `#1321 after-done-setting`.
  listen<Event>(el("after-done"), "change", () => deps.afterDoneChanged());
  // `#564 message-summary-toggle`. The chip over the list follows it (`setSummaryMode`).
  listen<Event>(el("summarize-collapsed"), "change", () => deps.summariesChanged());
  // `#225 enter-to-send`. Nothing to redraw and nothing to say on the status line: the hint under the switch changes in
  // place, and the next Enter in any box follows it.
  listen<Event>(el("enter-sends"), "change", () => deps.enterSendsChanged((el("enter-sends") as HTMLInputElement).checked));
  listen<Event>(el("mark-read-on-reply"), "change", () => deps.markReadChanged());
  listen<Event>(el("mark-read-quote-guard"), "change", () => deps.markReadQuoteGuardChanged());
  listen<Event>(el("mark-read-fold-guard"), "change", () => deps.markReadFoldGuardChanged());
  listen<Event>(el("mark-read-nonquote"), "change", () => deps.markReadNonquoteChanged());
  listen<Event>(el("mark-read-final-guard"), "change", () => deps.markReadFinalGuardChanged());
  listen<Event>(el("mark-read-first-guard"), "change", () => deps.markReadFirstGuardChanged());
  return { unmount };
}

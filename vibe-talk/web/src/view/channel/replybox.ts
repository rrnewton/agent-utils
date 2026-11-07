// The reply box's listeners (`#410 inline-reply-editor`, `#846 draft-chip-honest`, `#225 enter-to-send`,
// `#185 reply-new-thread`, `#314 reply-focus-flake`; `#624 voice-page-modularity`, moved from the legacy
// script's wiring into view/channel for the plan's step 17): Cancel and its discard question, the lists'
// scrolls following the Draft chip, Send and the box's keys, what is pasted in, the new-thread tick and the
// words kept as they are typed, the focus leaving, and the box's changing height fitted again. What each
// does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";
import type { SizeWatcher, SizeWatching } from "../dom/index.ts";

export interface ReplyBoxDependencies extends SizeWatching {
  /** An element of the page by id, and one of the left column's by id. */
  lookup(id: string): HTMLElement;
  leftLookup(id: string): HTMLElement;
  /** Cancel the reply, and answer its discard question. */
  cancelReply(): unknown;
  answerReplyDiscard(discard: boolean): unknown;
  /** A list scrolled: the Draft chip follows a reply open in the stream, once a frame (`#846`). */
  draftChipAsk(): void;
  /** Send the reply. */
  sendReply(): unknown;
  /** A key in the box: the one rule for every message box, with this box's send (`#225`). */
  onBoxKey(event: KeyboardEvent): unknown;
  /** A paste only ever puts text in. */
  watchBoxInsertions(box: HTMLElement): unknown;
  /** Keep the draft, words and the new-thread tick alike, as they change (`#185`). */
  rememberDraft(): unknown;
  /** Say where the reply now goes, under the box. */
  renderReplyDestination(): void;
  /** The focus leaving the box's hand-off (`#314`). */
  noteFocusLeaving(event: FocusEvent): void;
  /** Anything has the focus again: where the reader went (`#314`). */
  noteFocusIn(): void;
  /** Fit the room the reply's row keeps under itself to the box as it now stands (`#410`). */
  fitInlineRoom(): void;
  /** The view switch, pressed: the call's view or the channel's, whichever is not up. */
  pressViewSwitch(): unknown;
}

/** The listeners, mounted. */
export interface ReplyBox {
  /** Take every listener down again and stop the size watcher. */
  unmount(): void;
}

/** Mount the reply box's listeners, in the page's order. */
export function mountReplyBox(deps: ReplyBoxDependencies): ReplyBox {
  const { listen, unmount: unlisten } = createListening();
  const watchers: SizeWatcher[] = [];
  listen<Event>(deps.lookup("reply-cancel"), "click", () => deps.cancelReply());
  listen<Event>(deps.lookup("reply-discard-yes"), "click", () => deps.answerReplyDiscard(true));
  listen<Event>(deps.lookup("reply-discard-no"), "click", () => deps.answerReplyDiscard(false));
  // `#846 draft-chip-honest`. A reply open in the stream scrolls on and off screen with its list.
  for (const id of ["scroll-area", "side-scroll"]) {
    listen<Event>(deps.leftLookup(id), "scroll", () => deps.draftChipAsk(), { passive: true });
  }
  listen<Event>(deps.lookup("reply-send"), "click", () => deps.sendReply());
  // `#225 enter-to-send`. The box that had no Enter handler at all.
  listen<KeyboardEvent>(deps.lookup("reply-text"), "keydown", (event) => deps.onBoxKey(event));
  deps.watchBoxInsertions(deps.lookup("reply-text"));
  // The tick is the draft's as the words are, so it is kept as it is changed (`#185 reply-new-thread`).
  listen<Event>(deps.lookup("reply-branch"), "change", () => {
    deps.rememberDraft();
    deps.renderReplyDestination();
  });
  // A draft survives leaving the screen without sending, so it is written as it is typed.
  listen<Event>(deps.lookup("reply-text"), "input", () => deps.rememberDraft());
  listen<FocusEvent>(deps.lookup("reply-compose"), "focusout", (event) => deps.noteFocusLeaving(event));
  listen<Event>(document, "focusin", () => deps.noteFocusIn());
  // `#410 inline-reply-editor`. A reply open in the stream changes height without the window changing,
  // and the room its row keeps under itself goes with it, in the same frame.
  if (deps.canWatchSizes()) {
    const watcher = deps.sizeWatcher(() => deps.fitInlineRoom());
    watchers.push(watcher);
    for (const id of ["column-reply", "side-reply"]) watcher.observe(deps.leftLookup(id));
  }
  listen<Event>(deps.lookup("view-switch"), "click", () => deps.pressViewSwitch());
  return {
    unmount() {
      unlisten();
      for (const watcher of watchers.splice(0)) watcher.disconnect();
    },
  };
}

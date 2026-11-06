// The call's controls (`#13 talk-button`, `#886 voice-page-autoplay`; `#624 voice-page-modularity`, moved from the
// legacy script's wiring into view/voice for the plan's step 17): Talk, Hang up, the speaker, clearing the view,
// forgetting the stored conversations, and trying a failed start again. What each does is the call's and the
// transcript's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface CallControlsDependencies {
  /** An element of the page by id. */
  byId(id: string): HTMLElement;
  /** Talk: start the call, or answer in it. */
  onTalk(): unknown;
  /** Hang up: end the call. */
  hangUp(): unknown;
  /** `#886 voice-page-autoplay`. Hang up ends the going on: a read in progress, heard to its end, reads nothing after it. */
  haltAutoplay(): unknown;
  /** Clear the transcript's view. */
  clearView(): unknown;
  /** Erase the stored conversations: never able to hang up a live call (`guardQuietly`). */
  forgetConversations(): unknown;
  /** The speaker turned off, or on again. */
  toggleSpeaker(): unknown;
  /** Try a start that failed again. */
  retryStart(): unknown;
}

/** The listeners, mounted. */
export interface CallControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the call's controls. */
export function mountCallControls(deps: CallControlsDependencies): CallControls {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  listen(el("talk"), "click", () => deps.onTalk());
  listen(el("hang-up"), "click", () => deps.hangUp());
  listen(el("hang-up"), "click", () => deps.haltAutoplay());
  listen(el("clear-view"), "click", () => deps.clearView());
  listen(el("forget-conversations"), "click", () => deps.forgetConversations());
  listen(el("speaker"), "click", () => deps.toggleSpeaker());
  listen(el("retry-start"), "click", () => deps.retryStart());
  return { unmount };
}

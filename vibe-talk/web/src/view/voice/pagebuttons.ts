// The page's own buttons beside the call (`#269 approval-card-inline`, read-aloud's source and voice; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/voice for the plan's step 17): the banner's,
// the status line's and the error's dismissals, the post card's Send, Cancel and More, Settings opened and closed,
// reading on the device, and the read-aloud source and voice. What each does is its owner's; mounted once, in the
// page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface PageButtonsDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** Acts handed the press itself, as their owners take it. */
  dismissBanner(event: Event): unknown;
  dismissStatus(event: Event): unknown;
  sendPostProposal(event: Event): unknown;
  cancelPostProposal(event: Event): unknown;
  clearError(event: Event): unknown;
  readOnDevice(event: Event): unknown;
  closeSettings(event: Event): unknown;
  /** `#269 approval-card-inline`. More opens a long draft in place, and Less folds it again. */
  togglePostDraft(): unknown;
  openSettings(): unknown;
  /** The read-aloud source stepped from the strip's button, and chosen, with the voice, in Settings. */
  toggleAudioSource(): unknown;
  setWitVoice(voice: string): unknown;
  setReadAudioSource(source: string): unknown;
}

/** The listeners, mounted. */
export interface PageButtons {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the page's buttons in the page's order. */
export function mountPageButtons(deps: PageButtonsDependencies): PageButtons {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  /** A select's choice as it is when it changes. */
  const value = (id: string): string => (el(id) as HTMLSelectElement).value;
  listen<Event>(el("dismiss-banner"), "click", (event) => deps.dismissBanner(event));
  listen<Event>(el("dismiss-status"), "click", (event) => deps.dismissStatus(event));
  listen<Event>(el("post-confirm-send"), "click", (event) => deps.sendPostProposal(event));
  listen<Event>(el("post-confirm-cancel"), "click", (event) => deps.cancelPostProposal(event));
  listen<Event>(el("post-confirm-more"), "click", () => deps.togglePostDraft());
  listen<Event>(el("open-settings"), "click", () => deps.openSettings());
  listen<Event>(el("dismiss-error"), "click", (event) => deps.clearError(event));
  listen<Event>(el("read-on-device"), "click", (event) => deps.readOnDevice(event));
  listen<Event>(el("audio-source"), "click", () => deps.toggleAudioSource());
  listen<Event>(el("settings-wit-voice"), "change", () => deps.setWitVoice(value("settings-wit-voice")));
  listen<Event>(el("settings-audio-source"), "change", () => deps.setReadAudioSource(value("settings-audio-source")));
  listen<Event>(el("close-settings"), "click", (event) => deps.closeSettings(event));
  return { unmount };
}

// The channel's view chips and the thread picker (`#231 two-column-polish`, `#281 view-keys`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/channel for the plan's step 17): Main,
// Threads and All, the thread's Back, a thread chosen, and the picker touched, which is the cue that the reader wants
// the full list of threads. What each does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

/** The channel's views, in the order their chips are heard. */
const CHANNEL_VIEWS = ["main", "threads", "flat"] as const;

export interface ChannelViewControlsDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The channel shown in a view, the thread left, and the thread the picker names opened. */
  changeView(view: (typeof CHANNEL_VIEWS)[number]): unknown;
  closeThread(): unknown;
  selectThread(): unknown;
  /** The full list of threads read again. Answers nothing to the press: the read goes on behind it. */
  refreshThreadDirectory(): unknown;
}

/** The listeners, mounted. */
export interface ChannelViewControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the view chips and the thread picker's listeners. */
export function mountChannelViewControls(deps: ChannelViewControlsDependencies): ChannelViewControls {
  const { listen, unmount } = createListening();
  for (const view of CHANNEL_VIEWS) listen<Event>(deps.lookup(`channel-view-${view}`), "click", () => deps.changeView(view));
  listen<Event>(deps.lookup("thread-back"), "click", () => deps.closeThread());
  const picker = deps.lookup("thread-select");
  listen<Event>(picker, "change", () => deps.selectThread());
  // Touching the picker is the cue that the reader wants the full list of threads. (`#231 two-column-polish` greyed
  // it in two columns, and touching that one read nothing; since `#281 view-keys` it chooses there too.)
  for (const type of ["focus", "pointerdown"]) {
    listen<Event>(picker, type, () => {
      deps.refreshThreadDirectory();
    });
  }
  return { unmount };
}

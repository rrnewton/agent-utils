// Help's ways in and out (`#85 voice-desktop-review`; `#624 voice-page-modularity`, moved from the legacy script's
// wiring into view/settings for the plan's step 17): Help closed, opened at its top, and each settings group's `?`
// opening its own entry. What each does is the page's; mounted once, in the page's order, and taken down again.

import { HELP_TOPICS } from "../../present/settings/index.ts";
import { createListening } from "../dom/index.ts";

export interface HelpLinksDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** Help closed, and opened at a topic's entry, or at its top. */
  closeHelp(): unknown;
  showHelp(topic: string | null): unknown;
}

/** The listeners, mounted. */
export interface HelpLinks {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount Help's ways in and out in the page's order. */
export function mountHelpLinks(deps: HelpLinksDependencies): HelpLinks {
  const { listen, unmount } = createListening();
  listen<Event>(deps.lookup("close-help"), "click", () => deps.closeHelp());
  listen<Event>(deps.lookup("open-help"), "click", () => deps.showHelp(null));
  for (const topic of HELP_TOPICS) listen<Event>(deps.lookup(`help-link-${topic}`), "click", () => deps.showHelp(topic));
  return { unmount };
}

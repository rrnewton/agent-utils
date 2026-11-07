// The prompts tray's and the action menu's openers (`#820 voice-page-action-menu`, `#880 voice-page-menu-placement`,
// `#886 voice-page-autoplay`; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/shell for
// the plan's step 17): each a plain toggle, and what they show stated once at load. Mounted once, in the page's order,
// and taken down again.

import { createListening } from "../dom/index.ts";

export interface MenuOpenersDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The tray and the menu opened if shut, shut if open. */
  togglePrompts(): unknown;
  toggleActionMenu(): unknown;
  /** The menu stated shut; the reader's row and autoplay drawn as this device keeps them; the tray stated shut. */
  shutActionMenu(): unknown;
  renderReaderRow(): unknown;
  renderAutoplay(): unknown;
  shutPrompts(): unknown;
}

/** The listeners, mounted. */
export interface MenuOpeners {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the openers, then state at load what they show. */
export function mountMenuOpeners(deps: MenuOpenersDependencies): MenuOpeners {
  const { listen, unmount } = createListening();
  // A plain toggle: the button that shows the tray is the button that hides it again, which is the only interaction
  // model that needs no second control and no tap-outside handler to escape from.
  listen<Event>(deps.lookup("prompts-open"), "click", () => deps.togglePrompts());
  // `#820 voice-page-action-menu`. The action menu's opener, a toggle as the tray's is.
  listen<Event>(deps.lookup("action-menu-open"), "click", () => deps.toggleActionMenu());
  deps.shutActionMenu();
  // `#880 voice-page-menu-placement`. The reader's row as this device keeps it, before the first frame.
  deps.renderReaderRow();
  // `#886 voice-page-autoplay`. Autoplay as this device keeps it, on the strip's toggle and the menu's item.
  deps.renderAutoplay();
  // ...and stated once at load, rather than left to the first tap. The markup ships shut, but shut is a state the
  // opener has to be ASSERTING for a screen reader to hear it — an `aria-expanded` that only appears after somebody
  // has already opened the tray is no use to the reader who needs it.
  deps.shutPrompts();
  return { unmount };
}

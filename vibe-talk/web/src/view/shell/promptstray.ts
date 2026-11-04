// The tray of canned prompts (`#60 canned-prompt-buttons`; `#624 voice-page-modularity`, moved from the legacy
// script into view/shell): whether it is open, said on its opener (`aria-expanded`) and by the tray's `hidden`,
// the same fact stated to two readers and set together so they cannot come apart.
//
// ONE BUTTON FOR ALL OF THEM, and the reason is arithmetic rather than tidiness. The bar is a single row on a
// 375px phone and its width is priced to the edge by the page suite; each canned prompt used to cost it a
// member, and the list is meant to GROW — custom prompts are the next thing asked for. A tray costs one slot
// however many prompts are in it.
//
// Not persisted, deliberately: it is where a menu happens to be standing, not a preference. It comes back shut
// on every load, and every render of the bar that takes the button away shuts it. Which prompts it holds, what
// each sends, and the back gesture's level are the page's.

import type { ElementById } from "../dom/index.ts";

/** What the tray asks of the page. */
export interface PromptsTrayDependencies {
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** The tray opened or closed: a level of the back gesture came or went (`#304 back-gesture-in-app`). */
  changed(): void;
}

/** The tray's commands and questions. */
export interface PromptsTray {
  /** Whether it is open. */
  open(): boolean;
  /** Open it (`true`) or shut it, and say so on the button. */
  setOpen(open: boolean): void;
}

/** Shut. */
export function createPromptsTray(deps: PromptsTrayDependencies): PromptsTray {
  let promptsOpen = false;
  return {
    open: () => promptsOpen,
    setOpen(open) {
      promptsOpen = open === true;
      deps.byId("prompts-tray").hidden = !promptsOpen;
      deps.byId("prompts-open").setAttribute("aria-expanded", promptsOpen ? "true" : "false");
      deps.changed();
    },
  };
}

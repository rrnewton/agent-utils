// The list's action menu (`#820 voice-page-action-menu`; `#624 voice-page-modularity`, an owner from the
// start): whether it is open, said on its opener (`aria-expanded`) and by the menu's `hidden`, the same fact
// stated to two readers and set together so they cannot come apart; and its line for a list with nothing to
// expand, collapse or summarize, shown where none of its items is. Which items are offered, what each does,
// and the back gesture's level are the page's.

import type { ElementById } from "../dom/index.ts";

/** The menu's items, each shown only where it has work to do. */
// `#880 voice-page-menu-placement`: and Hide reader, offered on the channel's view.
const ACTION_MENU_ITEMS = Object.freeze(["expand-all", "collapse-all", "summarise", "reader-toggle"] as const);

/** What the menu asks of the page. */
export interface ActionMenuDependencies {
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** The menu opened or closed: a level of the back gesture came or went. */
  changed(): void;
}

/** The menu's commands and questions. */
export interface ActionMenu {
  /** Whether it is open. */
  open(): boolean;
  /** Open it (`true`) or close it. */
  setOpen(open: boolean): void;
  /** Its line for a list with nothing to do, after its items changed. */
  render(): void;
}

/** Closed. */
export function createActionMenu(deps: ActionMenuDependencies): ActionMenu {
  let actionMenuOpen = false;

  function render(): void {
    const none = ACTION_MENU_ITEMS.every((id) => deps.byId(id).hidden);
    const empty = deps.byId("action-menu-empty");
    if (empty.hidden !== !none) empty.hidden = !none;
  }

  return {
    open: () => actionMenuOpen,
    setOpen(open) {
      actionMenuOpen = open === true;
      if (actionMenuOpen) render();
      deps.byId("action-menu").hidden = !actionMenuOpen;
      deps.byId("action-menu-open").setAttribute("aria-expanded", actionMenuOpen ? "true" : "false");
      deps.changed();
    },
    render,
  };
}

// What the page and the tests use of it.
export { ACTION_MENU_ITEMS };

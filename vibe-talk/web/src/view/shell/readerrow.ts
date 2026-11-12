// The reader's row of big buttons, shown or hidden (`#880 voice-page-menu-placement`; `#624
// voice-page-modularity`, an owner from the start): the owner's choice under the action menu, kept on this
// device, stated where the stylesheet sees it (`data-reader` on the root), and said by the menu's item, whose
// word is what a press does. Only the drawing goes: the row's buttons keep their `hidden` as the page sets it,
// so the keys that read them (`h`, `s`) work as before. Where the item is offered is the page's.

import type { ElementById } from "../dom/index.ts";

/** The item's word for each state: what a press does. */
const READER_WORDS = Object.freeze({ shown: "Hide reader", hidden: "Hide reader" } as const);

/** What the row asks of the page. */
export interface ReaderRowDependencies {
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** The page's root element, where the stylesheet reads the choice. */
  root(): HTMLElement;
  /** Whether the row is kept hidden on this device, and keeping the choice. */
  stored(): boolean;
  keep(hidden: boolean): void;
}

/** The row's commands and questions. */
export interface ReaderRow {
  /** Whether the row is hidden. */
  hidden(): boolean;
  /** Hide it (`true`) or show it, and keep the choice. */
  setHidden(hidden: boolean): void;
  /** State the kept choice on the root and on the item. */
  render(): void;
}

/** As kept on this device. */
export function createReaderRow(deps: ReaderRowDependencies): ReaderRow {
  // Read on first use, and held: a browser that will not keep the choice still gets it for this page.
  let chosen: boolean | null = null;
  const hidden = (): boolean => (chosen === null ? (chosen = deps.stored()) : chosen);

  function render(): void {
    if (hidden()) deps.root().setAttribute("data-reader", "hidden");
    else deps.root().removeAttribute("data-reader");
    deps.byId("reader-toggle-label").textContent = hidden() ? READER_WORDS.hidden : READER_WORDS.shown;
    const toggle = deps.byId("reader-toggle");
    const pressed = hidden() ? "true" : "false";
    if (toggle.getAttribute("aria-pressed") !== pressed) toggle.setAttribute("aria-pressed", pressed);
  }

  return {
    hidden,
    setHidden(wanted) {
      chosen = wanted === true;
      deps.keep(chosen);
      render();
    },
    render,
  };
}

// What the page and the tests use of it.
export { READER_WORDS };

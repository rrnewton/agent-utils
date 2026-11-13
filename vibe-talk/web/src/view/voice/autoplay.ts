// Autoplay (`#886 voice-page-autoplay`; `#624 voice-page-modularity`, an owner from the start): on, a read heard
// to its end goes on to the next unread message below it, in the order of the view it was read in, the way the
// owner works down a channel by ear. Off by default and kept on this device. Its toggle at the read strip's left
// and its line in the action menu say the same state. A read stopped, hung up on or replaced never ends, so it
// never goes on; a read the reader starts goes on from where it is. Reading, marking read, the panes and their
// rows are the page's.

import { cardRootOf } from "../channel/index.ts";
import type { ElementById } from "../dom/index.ts";

/** The menu item's word for each state. */
const AUTOPLAY_WORDS = Object.freeze({ on: "Autoplay: on", off: "Autoplay: off" } as const);

export interface AutoplayDependencies<Pane> {
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** Whether autoplay is kept on, on this device, and keeping the choice. */
  stored(): boolean;
  keep(on: boolean): void;
  /** The lists a read can be in, the one the reader is working in first. */
  panes(): readonly Pane[];
  /** The rows `pane` shows, in its order. */
  paneRows(pane: Pane): HTMLElement[];
  /** The message ids `row` stands for (none for a card). */
  idsOf(row: HTMLElement): string[];
  /** Whether the message `row` stands for is read, by the rule both read modes share. */
  rowIsRead(row: HTMLElement): boolean;
  /** The row of `pane` for the message or card `id` as the list is now, or null. */
  paneRowFor(pane: Pane, id: string, card: boolean): HTMLElement | null;
  /** Read `row` aloud: a message row, or a card by its thread's first message. */
  read(row: HTMLElement): void;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
}

export interface Autoplay {
  /** Whether autoplay is on. */
  on(): boolean;
  /** Turn it on or off, and keep the choice. */
  setOn(on: boolean): void;
  /** State the choice on the strip's toggle and the menu's item. */
  render(): void;
  /**
   * A read of `parts` heard to its end, asked BEFORE its messages are marked read (which can redraw the list
   * without them): null where autoplay is off or the read was halted; else what to do once the read's archive
   * (`archived`) is under way, answering whether it started a read: the next unread row below, or, with none
   * left, saying so once the archive's own lines are said.
   */
  nextAfter(parts: readonly string[]): ((archived: Promise<unknown>) => boolean) | null;
  /** The read of message `id`, in progress, goes on to nothing when it ends (Hang up); null forgets a halt (Stop). */
  halt(id: string | null): void;
}

/** Off unless kept on. */
export function createAutoplay<Pane>(deps: AutoplayDependencies<Pane>): Autoplay {
  // Read on first use, and held: a browser that will not keep the choice still gets it for this page.
  let chosen: boolean | null = null;
  const on = (): boolean => (chosen === null ? (chosen = deps.stored()) : chosen);
  let halted: string | null = null;

  function render(): void {
    const pressed = on() ? "true" : "false";
    const toggle = deps.byId("read-autoplay");
    if (toggle.getAttribute("aria-pressed") !== pressed) toggle.setAttribute("aria-pressed", pressed);
    deps.byId("autoplay-toggle-label").textContent = on() ? AUTOPLAY_WORDS.on : AUTOPLAY_WORDS.off;
  }

  /**
   * Whether `row` is the row read as `parts`: by its id, as one of the messages it stands for, or, a card, by its
   * thread's first message, which is what a card reads (its own id is the thread's, which a chat service may give
   * in another form than the message's).
   */
  const holds = (row: HTMLElement, parts: readonly string[]): boolean => {
    if (row.getAttribute("data-id") === parts[0] || deps.idsOf(row).some((id) => parts.includes(id))) return true;
    const root = row.hasAttribute("data-card") ? cardRootOf(row) : null;
    return root !== null && parts.includes(String(root.id));
  };

  return {
    on,
    setOn(wanted) {
      chosen = wanted === true;
      deps.keep(chosen);
      render();
    },
    render,
    nextAfter(parts) {
      const wasHalted = halted !== null && halted === parts[0];
      halted = null;
      if (!on() || wasHalted || parts.length === 0) return null;
      for (const pane of deps.panes()) {
        const rows = deps.paneRows(pane);
        const at = rows.findIndex((row) => holds(row, parts));
        if (at < 0) continue;
        // A card, which holds no message, is unread while it says its thread has unread replies, as the n key reads it.
        const next = rows.slice(at + 1).find((row) => !holds(row, parts) &&
          (row.hasAttribute("data-card") ? !row.hasAttribute("data-thread-read") : !deps.rowIsRead(row)));
        if (!next) {
          // After the archive's own lines ("1 message marked as dealt with"), or they would say over it at once.
          return (archived) => {
            void archived.then(() => deps.setStatus("Autoplay: nothing unread below."));
            return false;
          };
        }
        const id = next.getAttribute("data-id") || "";
        const card = next.hasAttribute("data-card");
        // Found again when it is read: marking the last one read may have drawn the list again.
        return () => {
          const row = deps.paneRowFor(pane, id, card) || (next.isConnected ? next : null);
          if (row) deps.read(row);
          return row !== null;
        };
      }
      return null;
    },
    halt(id) {
      halted = id;
    },
  };
}

// What the page and the tests use of it.
export { AUTOPLAY_WORDS };

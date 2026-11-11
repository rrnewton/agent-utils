// The Done and next-unread keys (`#224 keyboard-shortcuts`, question 2; `cmd-e-done`; `#407 unpack-row-parity`;
// `#624 voice-page-modularity`, moved from the legacy script into view/shell for the plan's step 17): n moves to
// the next unread message, and e and Ctrl+E (Cmd+E) mark the selected one Done or not and move on. Which rows are
// read, the Done state and marking it, and the selection are the page's.

import { isTextField } from "../dom/index.ts";

/** `#1321 after-done-setting`. What e does after marking Done: stay on it, the next message below, or the next unread below. Kept in Settings (`core/settings`' `afterDone`), said here so the keys need no setting but the choice. */
export type AfterDone = "stay" | "next" | "unread";

/** The row a key acts on: its pane and row, and whether the key was taken even with no row to act on. */
export interface DoneRow<Pane> {
  readonly pane: Pane | null;
  readonly row: HTMLElement | null;
  readonly took: boolean;
}

export interface DoneKeysDependencies<Pane> {
  /** The pane and row the keys are on from `target`, in sight or not. */
  keyContext(target: HTMLElement): { readonly pane: Pane | null; readonly row: HTMLElement | null };
  /** The row a key acts on from `target`; `stay` moves the selection without the focus. */
  rowForAct(target: HTMLElement, runLine: boolean, stay: boolean): DoneRow<Pane>;
  /** The rows of `pane`'s list. */
  paneRows(pane: Pane): HTMLElement[];
  /** Whether `row` of `pane` is in sight. */
  rowInView(pane: Pane, row: HTMLElement): boolean;
  /** Whether the message `row` stands for is read, by the rule both read modes share. */
  rowIsRead(row: HTMLElement): boolean;
  /** Select `row` of `pane`, brought into view. */
  selectRow(pane: Pane, row: HTMLElement): void;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
  /** The row of `pane` for the message or card `id`, or null. */
  paneRowFor(pane: Pane, id: string | null, card: boolean): HTMLElement | null;
  /** The row of `pane` now nearest the place `at`, or null. */
  nearRow(pane: Pane, at: number): HTMLElement | null;
  /** Make `row` the selection of `pane`; `focus` takes the focus there too. */
  takeRow(pane: Pane, row: HTMLElement, focus: boolean): void;
  /** Bring `row` of `pane` into view. */
  bringIntoView(pane: Pane, row: HTMLElement): void;
  /** The message ids `row` stands for. */
  idsOf(row: HTMLElement): string[];
  /** Whether the message `id` is Done. */
  isDone(id: string): boolean;
  /** Mark the messages `ids` of `row` Done, or not, as its Done button does; a failure reported without ending a call. */
  toggleDone(ids: string[], row: HTMLElement): void;
  /** `#1321 after-done-setting`. What e does after marking Done, as kept in Settings. */
  afterDone(): AfterDone;
}

/** The keys. Each returns whether it did anything, as a binding of the keymap does. */
export interface DoneKeys {
  /** n: the next unread message below the selected one, or below the top of the screen. */
  nextUnreadFromKeyboard(target: HTMLElement): boolean;
  /**
   * Shift+ArrowDown / Shift+ArrowUp (`#1314 unread-step-keys`): the next or previous unread message
   * in the view, skipping read ones; at either end the selection stays and a hint says so.
   */
  stepUnreadFromKeyboard(target: HTMLElement, direction: number): boolean;
  /**
   * e, and Ctrl+E (Cmd+E): Done or not Done, the act of the row's Done button; marked Done, on to
   * wherever Settings' After Done choice says (`#1321 after-done-setting`: stay by default, the
   * next message, or the next unread below — question 2's behaviour).
   *
   * The chord is e for a reader typing (`cmd-e-done`): in a message box or the search, where e is a
   * letter, it acts on the message the keys act on, the selected one of the column of the box, and moves
   * the selection on without taking the focus from the box. False when there is no message to act on,
   * so that the chord is left to the browser.
   */
  doneFromKeyboard(target: HTMLElement): boolean;
}

export function createDoneKeys<Pane>(deps: DoneKeysDependencies<Pane>): DoneKeys {
  /**
   * The first unread message after `at` in `rows`, by the read rule both read modes share. Over the cards
   * a card, which holds no message, is unread while it says its thread has unread replies (`#407
   * unpack-row-parity`: its messages unpacked under it are rows of their own, read by the same rule).
   */
  const unreadAfter = (rows: HTMLElement[], at: number): HTMLElement | null => rows.slice(at + 1).find((row) =>
    (row.hasAttribute("data-card") ? !row.hasAttribute("data-thread-read") : !deps.rowIsRead(row))) || null;

  /** The last unread message before `at` in `rows`, by the same rule as `unreadAfter`. */
  const unreadBefore = (rows: HTMLElement[], at: number): HTMLElement | null => rows.slice(0, Math.max(at, 0)).reverse().find((row) =>
    (row.hasAttribute("data-card") ? !row.hasAttribute("data-thread-read") : !deps.rowIsRead(row))) || null;

  /**
   * After a Done made from the keyboard: on to the unread row `next` it chose before the act, else back
   * on the message itself, else (it has gone, under Hide read) the row now where it was. The rows are
   * already the new ones: the list is redrawn before the write goes out, so `next` is found again by its
   * id, a card told from a message (`paneRowFor`). `stay` moves the selection without the focus, as
   * `rowForAct` says.
   */
  function landAfterDone(pane: Pane, next: HTMLElement | null, ownId: string, at: number, stay = false): void {
    const again = next ? deps.paneRowFor(pane, next.getAttribute("data-id"), next.hasAttribute("data-card")) : null;
    const row = again || deps.paneRowFor(pane, ownId, false) || deps.nearRow(pane, at);
    if (!row) return;
    deps.takeRow(pane, row, !stay);
    deps.bringIntoView(pane, row);
  }

  return {
    nextUnreadFromKeyboard(target) {
      const { pane, row } = deps.keyContext(target);
      const rows = pane ? deps.paneRows(pane) : [];
      if (!pane || rows.length === 0) return false;
      let from = row && deps.rowInView(pane, row) ? rows.indexOf(row) : -1;
      if (from < 0) {
        const seen = rows.findIndex((one) => deps.rowInView(pane, one));
        from = seen < 0 ? -1 : seen - 1;
      }
      const next = unreadAfter(rows, from);
      if (next) deps.selectRow(pane, next);
      else deps.setStatus("Nothing unread below.");
      return true;
    },
    stepUnreadFromKeyboard(target, direction) {
      const { pane, row } = deps.keyContext(target);
      const rows = pane ? deps.paneRows(pane) : [];
      if (!pane || rows.length === 0) return false;
      let from = row && deps.rowInView(pane, row) ? rows.indexOf(row) : -1;
      let upTo = from;
      if (from < 0) {
        const seen = rows.findIndex((one) => deps.rowInView(pane, one));
        from = seen < 0 ? -1 : seen - 1;
        upTo = seen < 0 ? rows.length : seen;
      }
      const next = direction > 0 ? unreadAfter(rows, from) : unreadBefore(rows, upTo);
      if (next) deps.selectRow(pane, next);
      else deps.setStatus(direction > 0 ? "No more unread below." : "No more unread above.");
      return true;
    },
    doneFromKeyboard(target) {
      const stay = isTextField(target);
      const { pane, row, took } = deps.rowForAct(target, false, stay);
      if (!pane || !row) return took;
      const ids = deps.idsOf(row);
      if (ids.length === 0) return false;
      const marking = !ids.every((id) => deps.isDone(id));
      const rows = deps.paneRows(pane);
      const at = rows.indexOf(row);
      // `#1321 after-done-setting`. Only a marking moves, and only as the choice says: "stay"
      // chooses no row (landAfterDone then lands back on the message itself), "next" the row
      // below read or not, "unread" the next unread below. Putting one back never moves.
      const choice = deps.afterDone();
      const next = !marking || choice === "stay" ? null
        : choice === "next" ? (rows[at + 1] || null)
        : unreadAfter(rows, at);
      deps.toggleDone(ids, row);
      landAfterDone(pane, next, ids[0] as string, at, stay);
      return true;
    },
  };
}

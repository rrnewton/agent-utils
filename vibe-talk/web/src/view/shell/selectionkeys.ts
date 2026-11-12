// Moving the selection from the keyboard (`#224 keyboard-shortcuts`, the proposal's question 10; `#221
// read-modes`, `#338 column-overscroll`, `#277 thread-card-focus`; `#624 voice-page-modularity`, moved from the
// legacy script into view/shell for the plan's step 17): ↓ and ↑, j and k through the messages and an open ⋯
// menu, and the row every key about one message acts on. The lists, their rows, the selection and the room past
// the right column's foot are the page's.

import { focusMinimally, nodeWithin } from "../dom/index.ts";

/** How far ↓ and ↑ scroll through a message taller than the room before moving on: half the room. */
export const READ_STEP = 0.5;

/** A list the keys move through, as far as moving the selection reads it. */
export interface SteppedPane {
  /** The list's scroller. */
  scroller(): HTMLElement;
  /** Where the list's view begins and ends, under the bars over it and above the tools at its foot. */
  head(): number;
  foot(): number;
}

/** The open ⋯ menu, as far as its arrows read it. */
export interface SteppedMenu {
  readonly root: unknown;
  readonly button: unknown;
}

/** A node of the open ⋯ menu, as its walk reads it. */
type MenuNode = { readonly children?: Iterable<HTMLElement> };

/** The row a key acts on: its pane and row, and whether the key was taken even with no row to act on. */
export interface KeyRow<Pane> {
  readonly pane: Pane | null;
  readonly row: HTMLElement | null;
  readonly took: boolean;
}

export interface SelectionKeysDependencies<Pane extends SteppedPane> {
  /** The list's pane `node` is in, or null. */
  paneFor(node: unknown): Pane | null;
  /** The rows of `pane`'s list. */
  paneRows(pane: Pane): HTMLElement[];
  /** The row of `pane` the keys are on from `node`, or null. */
  currentRow(pane: Pane, node: unknown): HTMLElement | null;
  /** Whether `row` of `pane` is in sight. */
  rowInView(pane: Pane, row: HTMLElement): boolean;
  /** The first row of `pane` in sight, or null. */
  firstRowInView(pane: Pane): HTMLElement | null;
  /** Whether the place a thread was entered on still holds, and its first row (`#221 read-modes`). */
  landingHolds(): boolean;
  landingFirst(): HTMLElement | null;
  /** Move a pane's scroller by a measured delta, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(scroller: HTMLElement, delta: number): void;
  /** Select `row` of `pane`; `bring` false leaves the scroll where it is (its default is true); `from` the way the keys came. */
  selectRow(pane: Pane, row: HTMLElement, bring?: boolean, from?: number): void;
  /** Make `row` the selection of `pane`; `focus` takes the focus there too. */
  takeRow(pane: Pane, row: HTMLElement, focus: boolean): void;
  /** The room left above a row brought into view, in px. */
  landingGap(): number;
  /** Open the room past the foot of `area`, for a reader resting there (`#338 column-overscroll`). */
  openOverscroll(area: HTMLElement): void;
  /** The ⋯ menu open on a row, or null. */
  openRowMenu(): SteppedMenu | null;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
}

/** The keys, and the questions every key about one message asks. */
export interface SelectionKeys<Pane> {
  /**
   * The items in the open ⋯ menu that are shown, in order: what ↓ and ↑ move through there. A link
   * counts as the buttons do — "Open in …" is one (`#409 open-upstream-link`) — so the arrows can
   * land on every item the eye can see.
   */
  menuItems(): HTMLElement[];
  /** ↓/j or ↑/k, wherever the focus is: the items of a menu, else the selection in the pane. `held`: the key repeating. */
  stepFromKeyboard(target: HTMLElement, direction: number, held?: boolean): boolean;
  /** The pane the keys move and its current row. */
  keyContext(target: unknown): { readonly pane: Pane | null; readonly row: HTMLElement | null };
  /**
   * The row a key about one message acts on: `keyContext`, while any of the row is on screen. A row
   * scrolled out of sight, by the wheel or the scrollbar, is never acted on unseen: the key selects the
   * first message showing instead and does nothing else (`took`), so the next press acts on the row
   * the ring is round.
   *
   * The line of a collapsed run of read messages (`#221 read-modes`) stands for every message in the
   * run, so no key about ONE message acts on it; only Enter and o, which open the run (`runLine`).
   *
   * `stay` leaves the focus where it is, as for a chord pressed in a message box: the first message
   * showing is selected and drawn, and the box keeps the reader's typing.
   */
  rowForAct(target: unknown, runLine?: boolean, stay?: boolean): KeyRow<Pane>;
}

export function createSelectionKeys<Pane extends SteppedPane>(deps: SelectionKeysDependencies<Pane>): SelectionKeys<Pane> {
  /**
   * ↓ and ↑, j and k. The next or previous message, or, while the selected one runs past the foot of
   * the room (or above its head), half a room further through it first: the long answer of an agent is
   * read with the same key that moves on from it (the proposal's question 10).
   *
   * Starts from what is on screen when nothing is selected or the selection has been scrolled away,
   * never from a row far off: ↓ takes the top message showing, ↑ the bottom one. In a thread just
   * entered (`#221 read-modes`) the first key takes the message the entry landed on.
   *
   * `#338 column-overscroll`. ↓ past the last message of the right column, pressed afresh at its foot
   * (`held` is the key repeating), goes on into the room past it, as PgDn does.
   */
  function stepSelection(pane: Pane, node: HTMLElement, direction: number, held = false): boolean {
    const rows = deps.paneRows(pane);
    if (rows.length === 0) return false;
    const area = pane.scroller();
    const head = pane.head();
    const foot = pane.foot();
    const step = Math.max(1, Math.round((foot - head) * READ_STEP));
    const current = deps.currentRow(pane, node);
    const at = current ? rows.indexOf(current) : -1;
    if (at < 0 || !deps.rowInView(pane, current as HTMLElement)) {
      const landed = deps.landingHolds() && rows.includes(deps.landingFirst() as HTMLElement) ? deps.landingFirst() : null;
      const seen = rows.filter((row) => deps.rowInView(pane, row));
      const start = landed || (seen.length > 0 ? seen[direction > 0 ? 0 : seen.length - 1] : rows[direction > 0 ? 0 : rows.length - 1]);
      deps.selectRow(pane, start as HTMLElement);
      return true;
    }
    const row = current as HTMLElement;
    // A message a click selected silently takes the ring now: the key moves on from it, or reads on
    // through it, with the ring showing where the reader is.
    if (document.activeElement !== row) deps.selectRow(pane, row, false);
    const box = row.getBoundingClientRect();
    if (direction > 0 && box.bottom > foot + 1) {
      deps.nudge(area, Math.min(box.bottom - foot + deps.landingGap(), step));
      return true;
    }
    if (direction < 0 && box.top < head - 1) {
      deps.nudge(area, -Math.min(head - box.top + deps.landingGap(), step));
      return true;
    }
    const next = rows[at + direction];
    // Past the last message the list still has the box to write in below it, and past the first the
    // walk back to older history above: the key scrolls on toward either, as the arrow always did.
    if (!next) {
      if (direction > 0 && !held) deps.openOverscroll(area);
      deps.nudge(area, direction * step);
      return true;
    }
    deps.selectRow(pane, next, true, direction);
    return true;
  }

  function menuItems(): HTMLElement[] {
    const menu = deps.openRowMenu();
    if (!menu) return [];
    const items: HTMLElement[] = [];
    const walk = (node: MenuNode): void => {
      for (const kid of node.children || []) {
        if (kid.hidden) continue;
        const tag = String(kid.tagName).toUpperCase();
        if ((tag === "BUTTON" && kid !== menu.button) || tag === "A") items.push(kid);
        else walk(kid as MenuNode);
      }
    };
    walk(menu.root as MenuNode);
    return items;
  }

  /** ↓ and ↑ in an open ⋯ menu move through its items, as the arrows of a menu do, round from the ends. */
  function stepMenu(node: HTMLElement, direction: number): boolean {
    const menu = deps.openRowMenu();
    if (!menu || !nodeWithin(menu.root, node)) return false;
    const items = menuItems();
    if (items.length === 0) return false;
    const at = items.indexOf(node);
    const next = at < 0 ? items[direction > 0 ? 0 : items.length - 1] : items[(at + direction + items.length) % items.length];
    focusMinimally(next as HTMLElement);
    return true;
  }

  function keyContext(target: unknown): { readonly pane: Pane | null; readonly row: HTMLElement | null } {
    const pane = deps.paneFor(target);
    return { pane, row: pane ? deps.currentRow(pane, target) : null };
  }

  return {
    menuItems,
    stepFromKeyboard(target, direction, held = false) {
      if (stepMenu(target, direction)) return true;
      const pane = deps.paneFor(target);
      return pane ? stepSelection(pane, target, direction, held) : false;
    },
    keyContext,
    rowForAct(target, runLine = false, stay = false) {
      const { pane, row } = keyContext(target);
      if (!pane || !row) return { pane, row: null, took: false };
      // A card of the right column is a thread, not a message: only Enter and o act on it, and highlight
      // it (`#277 thread-card-focus`; → enters it, `cardForArrow`). `#229 desktop-two-column`.
      if (row.hasAttribute("data-card")) return { pane, row: runLine ? row : null, took: false };
      if (!runLine && row.getAttribute("data-read-run") === "head") {
        deps.setStatus("Read messages: Enter opens them.");
        return { pane, row: null, took: true };
      }
      if (deps.rowInView(pane, row)) return { pane, row, took: false };
      const first = deps.firstRowInView(pane);
      if (first) {
        deps.takeRow(pane, first, !stay);
        deps.setStatus("Selected the first message showing.");
      }
      return { pane, row: null, took: Boolean(first) };
    },
  };
}

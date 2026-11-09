// The mouse and the keys move one selection (`#239 single-selection`, `#243 filter-keeps-selection`, `#261
// two-column-connectors`, `#298 thread-card-header`, `#281 view-keys`; `#624 voice-page-modularity`, moved from the
// legacy script into view/shell for the plan's step 17): what the reader used last, a real mouse move or a click
// taking the selection, the focus arriving in a list, a jump landing, the ring following a page of scrolling, and
// handing the keys to the list on screen. The panes, their rows and marks, drawing the selection and the glide
// between the columns are the page's.

import { isTextField, keepsOwnKeys, nodeWithin, shownNode } from "../dom/index.ts";

/** A pointer's move, as far as telling a mouse really moving reads it. */
export interface PointerMoved {
  readonly pointerType?: string;
  readonly clientX?: unknown;
  readonly clientY?: unknown;
  readonly movementX?: unknown;
  readonly movementY?: unknown;
}

/** What the reader used last, as the selection reads it (web/src/view/channel `createInputKinds`). */
export interface SelectionInputKinds {
  keys(): void;
  mouse(): void;
  touch(): void;
  /** Whether a `pointermove` is a mouse really moving. */
  mouseMoved(event: PointerMoved | null | undefined): boolean;
  /** Whether a selection made now is drawn, with a ring and the focus: after the keys or the mouse. */
  drawnBy(): boolean;
}

/** A list the keys move through, as far as the pointer and the focus read it. */
export interface PointedPane {
  scroller(): HTMLElement;
  list(): unknown;
  head(): number;
  /** The row lit, and the row selected with a ring (which `releaseRow` changes). */
  readonly lit: HTMLElement | null;
  row: HTMLElement | null;
  /** The row a page key's scroll is to move the ring from, once it comes to rest. */
  follow: HTMLElement | null;
}

/** The selection a pane keeps, as far as a click and the focus compare it. */
export interface KeptMark {
  readonly drawn: boolean;
}

export interface PointerSelectionDependencies<Pane extends PointedPane, Column, Mark extends KeptMark = KeptMark> {
  /** What the reader used last. */
  readonly inputKinds: SelectionInputKinds;
  /** Which screen is up, and the view of the main screen. */
  screen(): string;
  view(): string;
  /** Whether the focus has been lost to the page's body. */
  focusLost(): boolean;
  /** The lists the keys move through. */
  keyPanes(): readonly Pane[];
  /** The list's pane `node` is in, or null. */
  paneFor(node: unknown): Pane | null;
  /** The channel's list, and the right column's. */
  channelPane(): Pane;
  sidePane(): Pane;
  /** The pane the keys moved last, and handing them another. */
  lastPane(): Pane;
  takeLastPane(pane: Pane): void;
  /** Whether a row's ⋯ menu is open. */
  rowMenuOpen(): boolean;
  /** Make `row` the selection of `pane`, drawn; `focus` takes the focus there too. */
  takeRow(pane: Pane, row: HTMLElement, focus: boolean): void;
  /** Make `row` the selection of `pane` silently: nothing lit. */
  noteRow(pane: Pane, row: HTMLElement): void;
  /** Light the selection of the pane in use. */
  drawSelection(): void;
  /** The row of `pane` that `node` is in, or null. */
  rowOf(pane: Pane, node: unknown): HTMLElement | null;
  /** Whether `row` is shown. */
  rowShown(row: HTMLElement): boolean;
  /** The selection `pane` keeps, or null, and whether it names `row`. */
  markOf(pane: Pane): Mark | null;
  markNames(mark: Mark | null, row: HTMLElement): boolean;
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /** Whether the code is working in the right column. */
  inSide(): boolean;
  /** The left column, the column of `pane`, and running `act` in `column`. */
  mainColumn(): Column;
  paneColumn(pane: Pane): Column;
  inColumn(column: Column, act: () => void): void;
  /** The reader acted in a list (`#243 filter-keeps-selection`). */
  readerActed(): void;
  /** Whether `row` of `pane` is in sight, and the first row in sight. */
  rowInView(pane: Pane, row: HTMLElement): boolean;
  firstRowInView(pane: Pane): HTMLElement | null;
  /** Select `row` of `pane`; `bring` false leaves the scroll where it is. */
  selectRow(pane: Pane, row: HTMLElement, bring: boolean): void;
  /** The selected row of `pane` when it is drawn, or null. */
  drawnRow(pane: Pane): HTMLElement | null;
  /** Take the `tabindex` back from the row of `pane` selected before `row`, and note `row` as the one holding it. */
  releaseRow(pane: Pane, row: HTMLElement): void;
  /** Give `row`'s time its tab stop, or take it away. */
  rowTimeTabStop(row: HTMLElement, on: boolean): void;
  /** The list on screen the focus would scroll, or null. */
  visibleScroller(): HTMLElement | null;
  /** Whether the thread list covers the left column, and whether the right column is drawn. */
  threadListOnScreen(): boolean;
  sideColumnUp(): boolean;
}

/** The selection the mouse and the keys share. */
export interface PointerSelection<Pane> {
  /** The keys are in use: noted, and said on the root element as `data-input="keys"`. */
  keysInUse(): void;
  /** The mouse is in use: noted, and the keys' `data-input` taken off the root element. */
  mouseInUse(): void;
  /** A finger or a pen is in use: noted, and said on the root as `data-input="touch"`. */
  touchInUse(): void;
  /** Whether a selection made now is drawn, with a ring and the focus: after the keys or the mouse. */
  drawnBy(): boolean;
  /**
   * Whether a selection made by the mouse or a jump takes the focus with it: while the focus is on a
   * list (a row, a row's control, or the scroller), on nothing, or on a control off the lists that a
   * click focused. Never out of a field the reader is typing in, a picker or a slider, nor from a
   * control the keys reached (Tab to a button in the dock): the reader there is working by keyboard.
   *
   * A CLICKED CONTROL GIVES THE FOCUS UP. A click on Summarise or Show read focuses the button, and
   * without a ring (it is not `:focus-visible`), so nothing on screen says it holds the keys. The
   * mouse then moves over a message, which is drawn selected, ring and all; if the button kept the
   * focus, Enter pressed it again and the lit message was not touched, the owner's ambiguity over
   * again. `#239 single-selection`.
   */
  focusFollowsSelection(): boolean;
  /** Every `pointermove` on the page: a real mouse move puts the keys' look away and, over a row, selects it. */
  onPointerMove(event: PointerMoved & { readonly buttons?: number; readonly target?: unknown }): void;
  /**
   * Every `pointerdown` on the page: which kind of pointer the click or tap after it comes from. One in
   * a list of messages is the reader acting there, before anything the press goes on to do: a fold, a
   * reply's arrow, a swipe, a scroll begun by a finger. `#243 filter-keeps-selection`.
   */
  onPointerDown(event: { readonly pointerType?: string; readonly target?: unknown } | null | undefined): void;
  /**
   * A click in a pane selects its row: drawn, after the mouse (the move to it has usually selected it
   * already), and SILENTLY after a tap on a touch screen, where the next key starts there and nothing
   * is lit. A click that did more than select (Reply, a thread's tag) has rebuilt the list or left the
   * screen, and finds no row here.
   */
  noteClickedRow(pane: Pane, node: unknown): void;
  /**
   * The focus arrived in a pane: by Tab, by a click on a row's control, by the page. The pane is the one
   * in use, and a row the focus is in is the selection, so that the keys act on the message that is
   * lit and the lit message is where a screen reader is. Drawn or silent as `drawnBy` says.
   */
  onPaneFocus(pane: Pane, event: { readonly target?: unknown } | null | undefined): void;
  /**
   * A jump landed on `row` (`revealRow`): it becomes the selection, drawn and with the focus as a
   * mouse's would be, or silently after a tap. Not a row of the list on screen (the Pinned filter's
   * stands in front of the channel's): nothing.
   */
  selectLanded(row: HTMLElement): void;
  /**
   * The keyboard selection follows a page of scrolling. PgUp, PgDn and Space are the browser's, and
   * scroll the list under a selected row without moving the focus; once the scroll has come to rest,
   * the ring moves to the first message shown whole, so the next key acts on what is on screen. Only
   * a selection the keyboard made, and only while it still holds the focus. Without a `scrollend` event
   * (older browsers) the ring stays put, and the next arrow starts from the screen as it always does.
   *
   * ONE WAIT PER PANE, and none for a key that cannot move the list (`up`: PgUp or Shift+Space at the
   * top; PgDn or Space at the end): a listener per key piled up under a held PgDn at the newest line,
   * and the next scroll to end, the wheel's, then moved the ring, which the wheel never does.
   */
  followPageScroll(pane: Pane, row: HTMLElement, up: boolean): void;
  /**
   * Give the list on screen the focus (the keyboard's selected message when it is drawn, else the
   * scroller) so that the scroll keys and the row keys reach it. `preventScroll`: taking the focus
   * must not move the list, because where the reader is has rules of its own.
   */
  focusList(pane: Pane | null | undefined): void;
  /** `focusList`, unless the reader is typing in a field still on screen, which is never taken from them. */
  settleFocus(): void;
  /**
   * Whether `node` is in the left column while the thread list covers it (`#281 view-keys`): drawn, as
   * far as its `hidden` says, but out of sight, so neither the focus nor the keys are left there.
   */
  behindThreadList(node: unknown): boolean;
  /**
   * Whether `node` is in the right column while it is not drawn — two columns or the thread list just
   * left, by a view key pressed in its box (`#281 view-keys`). Its `hidden` says nothing of that: the
   * stylesheet takes the column away, and the browser then drops the focus on the page's body.
   */
  inColumnGone(node: unknown): boolean;
}

export function createPointerSelection<Pane extends PointedPane, Column, Mark extends KeptMark = KeptMark>(
  deps: PointerSelectionDependencies<Pane, Column, Mark>,
): PointerSelection<Pane> {
  const kinds = deps.inputKinds;
  const drawnBy = (): boolean => kinds.drawnBy();
  const mouseInUse = (): void => {
    kinds.mouse();
  };

  /**
   * Whether `node` holds the focus the keys gave it, as the browser says by drawing its ring
   * (`:focus-visible`). A browser that cannot say counts as yes, which keeps the focus where it is.
   */
  function keyboardFocused(node: unknown): boolean {
    const element = node as { matches?: (selector: string) => boolean } | null;
    if (!element || typeof element.matches !== "function") return true;
    try {
      return element.matches(":focus-visible");
    } catch (_error) {
      return true;
    }
  }

  function focusFollowsSelection(): boolean {
    if (deps.screen() !== "main") return false;
    if (deps.focusLost()) return true;
    const held = document.activeElement;
    if (isTextField(held) || keepsOwnKeys(held)) return false;
    if (deps.keyPanes().some((pane) => nodeWithin(pane.scroller(), held))) return true;
    return !keyboardFocused(held);
  }

  /**
   * A real mouse move over `row` of `pane` takes the selection there: from the keyboard, from the
   * other column, from wherever it was. The focus goes with it while it is on a list (and stays on a
   * control of the row already holding it); the ring and the tint go with `data-selected`. Not while a
   * ⋯ menu is open: the menu holds the focus and its arrows, and a click outside it closes it first.
   *
   * Nothing scrolls for it (`#1144 scrollback-typing-jump`): the mouse resting here used to glide
   * the other column to the selection's counterpart; hover now only selects.
   */
  function pointRow(pane: Pane, row: HTMLElement): void {
    if (deps.rowMenuOpen()) return;
    const holding = nodeWithin(row, document.activeElement);
    // Lit is drawn selected (`drawSelection` lights only that), so a move within the row already lit
    // asks no more: not the pane's kept selection, which in the right column switches to that column
    // and back on every move to read.
    if (!(deps.lastPane() === pane && pane.lit === row && (holding || !focusFollowsSelection()))) {
      deps.takeRow(pane, row, !holding && focusFollowsSelection());
    }
  }

  /** The scrollers `followPageScroll` listens on: one `scrollend` listener each, added once, for good. */
  const followedScrollers = new Set<HTMLElement>();

  /** A page key's scroll has come to rest: the ring to the first message shown whole, if its row has gone by. */
  function settleFollow(pane: Pane): void {
    const row = pane.follow;
    pane.follow = null;
    if (!row || document.activeElement !== row || pane.row !== row) return;
    if (deps.rowInView(pane, row) && row.getBoundingClientRect().top >= pane.head() - 1) return;
    const first = deps.firstRowInView(pane);
    if (first && first !== row) deps.selectRow(pane, first, false);
  }

  function focusList(pane: Pane | null | undefined): void {
    const row = pane ? deps.drawnRow(pane) : null;
    if (row && pane) {
      deps.releaseRow(pane, row);
      row.setAttribute("tabindex", "0");
      deps.rowTimeTabStop(row, true);
    }
    // The pane handed the keys is the one in use, and its selection the one lit.
    if (pane && pane.list() !== null && deps.lastPane() !== pane) {
      deps.takeLastPane(pane);
      deps.drawSelection();
    }
    // The pane's own list when one is named: in two columns the keys can be handed to either column.
    const target = row || (pane ? pane.scroller() : deps.visibleScroller());
    if (target && document.activeElement !== target && typeof target.focus === "function") {
      target.focus({ preventScroll: true });
    }
  }

  function behindThreadList(node: unknown): boolean {
    return deps.threadListOnScreen() && nodeWithin(deps.leftById("scroll-area"), node);
  }

  function inColumnGone(node: unknown): boolean {
    return !deps.sideColumnUp() && nodeWithin(deps.leftById("side-column"), node);
  }

  return {
    keysInUse: () => {
      kinds.keys();
    },
    mouseInUse,
    touchInUse: () => {
      kinds.touch();
    },
    drawnBy,
    focusFollowsSelection,
    onPointerMove(event) {
      if (!kinds.mouseMoved(event)) return;
      mouseInUse();
      // A button held is a drag (text being selected, a scrollbar dragged), not a message being chosen.
      if (event.buttons) return;
      if (deps.screen() !== "main") return;
      const pane = deps.keyPanes().find((each) => nodeWithin(each.scroller(), event.target) && each.list() !== null);
      const row = pane ? deps.rowOf(pane, event.target) : null;
      if (pane && row && deps.rowShown(row)) pointRow(pane, row);
    },
    onPointerDown(event) {
      const kind = String((event && event.pointerType) || "");
      if (kind === "mouse") mouseInUse();
      else if (kind) kinds.touch();
      const target = event && event.target;
      if (deps.screen() !== "main" || !target) return;
      const pane = deps.keyPanes().find((each) => { const list = each.list(); return list !== null && nodeWithin(list, target); });
      if (pane) deps.inColumn(deps.paneColumn(pane), () => deps.readerActed());
      // The call's transcript, which no key moves through, is a list of messages a filter keeps a place in too.
      else if (deps.view() !== "discord" && nodeWithin(deps.byId("transcript"), target)) deps.inColumn(deps.mainColumn(), () => deps.readerActed());
    },
    noteClickedRow(pane, node) {
      const row = deps.rowOf(pane, node);
      if (!row) return;
      const mark = deps.markOf(pane);
      const drawn = drawnBy();
      if (mark && deps.markNames(mark, row) && (mark.drawn || !drawn) && deps.lastPane() === pane) return;
      if (!drawn) {
        deps.noteRow(pane, row);
        return;
      }
      deps.takeRow(pane, row, !nodeWithin(row, document.activeElement) && focusFollowsSelection());
    },
    onPaneFocus(pane, event) {
      const row = deps.rowOf(pane, event && event.target);
      const mark = deps.markOf(pane);
      const same = Boolean(row && mark && deps.markNames(mark, row));
      if (row && !drawnBy()) {
        if (!same) deps.noteRow(pane, row);
      } else if (row && !(same && mark && mark.drawn && pane.row === row)) {
        deps.takeRow(pane, row, false);
      }
      if (deps.lastPane() !== pane && pane.list() !== null) {
        deps.takeLastPane(pane);
        deps.drawSelection();
      }
    },
    selectLanded(row) {
      const pane = deps.inSide() ? deps.sidePane() : deps.channelPane();
      if (deps.rowOf(pane, row) !== row) return;
      if (drawnBy()) deps.takeRow(pane, row, focusFollowsSelection());
      else deps.noteRow(pane, row);
    },
    followPageScroll(pane, row, up) {
      const area = pane.scroller();
      pane.follow = null;
      if (!("onscrollend" in area)) return;
      const end = Math.max(0, Number(area.scrollHeight) - Number(area.clientHeight));
      if (up ? area.scrollTop <= 0 : area.scrollTop >= end - 1) return;
      pane.follow = row;
      if (followedScrollers.has(area)) return;
      followedScrollers.add(area);
      area.addEventListener("scrollend", () => {
        for (const each of deps.keyPanes()) if (each.scroller() === area) settleFollow(each);
      });
    },
    focusList,
    settleFocus() {
      const held = document.activeElement;
      if (held && isTextField(held) && shownNode(held) && !behindThreadList(held) && !inColumnGone(held)) return;
      focusList(deps.screen() === "main" ? deps.paneFor(held) : null);
    },
    behindThreadList,
    inColumnGone,
  };
}

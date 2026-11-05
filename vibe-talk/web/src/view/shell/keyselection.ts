// The selection the keys move (`#224 keyboard-shortcuts`, `#239 single-selection`, `#229 desktop-two-column`,
// `#284 redraw-keeps-pane`, `#407 unpack-row-parity`; `#624 voice-page-modularity`, moved from the legacy
// script into view/shell for the plan's step 17): which list a key is for, the rows it moves through, the
// selection kept per list by message id, drawing it as the one lit row of the page, selecting a row and
// bringing it into view, and putting the selection back after a redraw. The lists themselves (what each
// shows, the room over its head and foot) and the rows' parts are the page's.

import { nodeWithin } from "../dom/index.ts";

/** The selection a list keeps: on which message, and how it was made. */
export interface KeySelection {
  /** The message the selection is on: its row's `data-id`. */
  readonly id: string;
  /**
   * Whether that row is a thread's card, which shares its `data-id` with its root's row where the thread
   * is named after its root and the root stands unpacked under it (`#407 unpack-row-parity`). Absent where
   * it is not known, as for a selection a filter carried.
   */
  readonly card?: boolean;
  /** Whether it is drawn and holds the focus: the keyboard, the mouse or a jump chose it; not a tap. */
  readonly drawn: boolean;
  /** Where its row was in the list, for when the row goes. */
  readonly index: number;
}

/** A list the keys move through. */
export interface KeyPane {
  readonly name: string;
  /** The element that scrolls the pane's list. */
  scroller(): HTMLElement;
  /** The list whose rows the keys move through, or null where the pane takes no row keys at the moment. */
  list(): HTMLElement | null;
  /** Viewport y of the pane's floating line, and of its chips: a row above the one or below the other is under it. */
  head(): number;
  foot(): number;
  /** Which list the pane is showing. A selection is kept per list. */
  context(): string;
  /** The selected message in each list the pane has shown. */
  readonly marks: Map<string, KeySelection>;
  /** The row selected with a ring, while it holds the `tabindex`. */
  row: HTMLElement | null;
  /** The row carrying `data-selected`, the one lit row of the page while this is the pane in use. */
  lit: HTMLElement | null;
  /** The selected row a page key scrolled away under, while the ring waits for that scroll to end to follow it. */
  follow: HTMLElement | null;
}

export interface KeySelectionDependencies<Pane extends KeyPane> {
  /** Every list the keys can move through, the channel's first; and the channel's and the right column's. */
  keyPanes(): readonly Pane[];
  channelPane(): Pane;
  sidePane(): Pane;
  /** The pane the keys moved last, and handing them another. */
  lastPane(): Pane;
  takeLastPane(pane: Pane): void;
  /** Whether the thread list covers the left column, and whether the right column is drawn. */
  threadListOnScreen(): boolean;
  sideColumnUp(): boolean;
  /** Whether a reply is open in the stream, and the pane of its list (`#410 inline-reply-editor`). */
  replyDocked(): boolean;
  replyPane(): Pane;
  /** Which screen is up. */
  screen(): string;
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /** Whether `row` is shown; the message ids it stands for; and the row a member of a folded run is shown as. */
  rowShown(row: HTMLElement): boolean;
  idsOf(row: HTMLElement): string[];
  shownRowFor(row: HTMLElement): HTMLElement | null;
  /** Give `row`'s time its tab stop, or take it away. */
  rowTimeTabStop(row: HTMLElement, on: boolean): void;
  /** The reader acted in the list of `pane` (`#243 filter-keeps-selection`). */
  readerActedIn(pane: Pane): void;
  /** Put out the flash of a jump's landing, when it is on another row than `row`. */
  putOutLanding(row: HTMLElement): void;
  /** Draw the link of the selection between the columns again (`#261 two-column-connectors`). */
  noteLinkedSelection(): void;
  /** Whether nothing holds the focus. */
  focusLost(): boolean;
  /** The room left above and below a row brought into view, in px. */
  landingGap(): number;
}

/** The selection the keys move. */
export interface KeySelectionModel<Pane> {
  /**
   * The pane `node` is in; or else the one the keys moved last, while it takes row keys; or else the
   * first that does. Null when no pane takes them, as on the call's view.
   */
  paneFor(node: unknown): Pane | null;
  /**
   * The element the scroll keys are for, on the screen that is up: on the main screen the scroller of
   * the pane `node` is in (or the pane `paneFor` picks), elsewhere the screen. `node` is the focus by default.
   */
  visibleScroller(node?: unknown): HTMLElement;
  /**
   * The rows a pane's keys move through, top to bottom: its messages that are drawn, and over the cards
   * the cards with every thread's messages unpacked under them (`#407 unpack-row-parity`), in the order
   * they are read.
   */
  paneRows(pane: Pane): HTMLElement[];
  /** The row of `pane` that `node` is in, or null. */
  rowOf(pane: Pane, node: unknown): HTMLElement | null;
  /**
   * The row standing for message `id` in `pane` now: its own, the combined row holding it, or the
   * line of the collapsed run it is in (`#221 read-modes`); in the right column's list, the card of
   * thread `id`, which has a `data-id` and no messages. Null when the list does not draw it. `card` says
   * which of the two it is where that is known (`KeySelection`): a card and its root's row unpacked
   * under it can share an id.
   */
  paneRowFor(pane: Pane, id: string | null | undefined, card?: boolean): HTMLElement | null;
  /** The selection kept for the list `pane` is showing now, or null. */
  markOf(pane: Pane): KeySelection | null;
  /** The row `pane`'s kept selection `mark` is on now, or null: `paneRowFor`, a card told from a message. */
  markedRow(pane: Pane, mark: KeySelection | null): HTMLElement | null;
  /** Whether `row` is the row the kept selection `mark` names. */
  markNames(mark: KeySelection | null, row: HTMLElement): boolean;
  /** Take the `tabindex` back from the row selected last, when `row` is not that row. */
  releaseRow(pane: Pane, row?: HTMLElement | null): void;
  /** The pane in use: the one moved last, by a key, the mouse or the focus, while it shows a list. */
  activePane(): Pane | null;
  /**
   * Light the selection, and nothing else: `data-selected` on the selected row of the pane in use when
   * that selection is drawn, and on no other row of any pane. `#239 single-selection`. web/voice.css
   * draws the attribute, ring and tint, as the one look of the selected message.
   *
   * The flash of a jump's landing goes out when the lit row is another: it marked the message the jump
   * was for, and two lit messages are what this is here to end.
   */
  drawSelection(): void;
  /**
   * Select `row` in `pane` with a ring: noted, drawn, the `tabindex` on it, and the focus too when
   * `focus` says so. Nothing is scrolled.
   */
  takeRow(pane: Pane, row: HTMLElement, focus: boolean): void;
  /** Select `row` in `pane` silently, as a tap on a touch screen does: noted, and neither drawn nor focused. */
  noteRow(pane: Pane, row: HTMLElement): void;
  /**
   * The row the keys act on: the one holding the focus, or else the one selected in this list, by
   * the keyboard or silently by a click. Null when there is neither.
   */
  currentRow(pane: Pane, node: unknown): HTMLElement | null;
  /** The row selected with a ring in this list, while it is on screen: what the focus comes back to. */
  drawnRow(pane: Pane): HTMLElement | null;
  /** The row at `index` in `pane`, or the nearest one there is: where a row that went is replaced. */
  nearRow(pane: Pane, index: number): HTMLElement | null;
  /** Whether any of `row` shows between the pane's floating line and its chips. */
  rowInView(pane: Pane, row: HTMLElement): boolean;
  /** The first row of `pane` shown whole between its floating line and its chips, else the first showing at all. */
  firstRowInView(pane: Pane): HTMLElement | null;
  /**
   * Select `row`: the focus on it, without the browser's own jump, and then (unless `bring` is false)
   * the least scrolling that shows it whole. See `bringIntoView` for a row taller than the room.
   */
  selectRow(pane: Pane, row: HTMLElement, bring?: boolean, from?: number): void;
  /**
   * Scroll `pane` the least that puts `row` between its floating line and its chips. A row taller than
   * the room shows its head, where reading it starts, unless the reader arrived on it going up (`from`
   * below zero): then its foot, so that ↑ reads back up through it as ↓ read down.
   */
  bringIntoView(pane: Pane, row: HTMLElement, from?: number): void;
  /**
   * Put the keyboard's selection back after a redraw took its row away. Called at the end of every
   * pass over the scroll tools, which every bulk redraw of the lists ends with.
   *
   * The same message's new row takes the `tabindex`, and the focus too when the old row had it: a
   * browser drops the focus to the body when the focused row is removed, and the ring would vanish
   * under the reader at every refresh. A message that has gone (Done under Hide read) hands the
   * selection to the row now where it was: the next one, or the last at the end of the list.
   *
   * `#284 redraw-keeps-pane`. With the focus on nothing, it comes back only in the pane in use, the one
   * the reader is in. In two columns the focus can be lost while the reader is in the right one — a
   * click there on something that takes no focus, or a control that hides itself — and a live message
   * then redraws Main: given Main's row, the focus took the keys with it, and Esc no longer closed the
   * right column's thread. The other pane keeps its selection, unfocused, for when the reader goes back.
   */
  keepSelection(): void;
}

export function createKeySelection<Pane extends KeyPane>(deps: KeySelectionDependencies<Pane>): KeySelectionModel<Pane> {
  function paneFor(node: unknown): Pane | null {
    const channel = deps.channelPane();
    const side = deps.sidePane();
    for (const pane of deps.keyPanes()) {
      // The left column behind the thread list takes no keys: the cards are the list on screen; nor does
      // the right column while it is not drawn, as when a card has just opened its thread. `#281 view-keys`.
      if (pane === channel ? deps.threadListOnScreen() : !deps.sideColumnUp()) continue;
      if (node && nodeWithin(pane.scroller(), node)) return pane;
      // `#298 thread-card-header`. The right column's heading, and the card at the top of its thread in
      // it, are that column's: a key pressed there acts there, whichever column the keys moved last.
      if (pane === side && node && nodeWithin(deps.leftById("side-heading"), node)) return pane;
    }
    // `#410 inline-reply-editor`. A reply open in the stream is its list's: Escape out of its box goes to
    // that list, and the page keys there move it.
    if (deps.replyDocked() && node && nodeWithin(deps.leftById("reply-compose"), node)) return deps.replyPane();
    if (deps.lastPane().list() !== null) return deps.lastPane();
    return deps.keyPanes().find((pane) => pane.list() !== null) || null;
  }

  function paneRows(pane: Pane): HTMLElement[] {
    const list = pane.list();
    if (!list) return [];
    return ([...list.children] as HTMLElement[]).filter((row) => row.hasAttribute("data-id") && deps.rowShown(row));
  }

  function paneRowFor(pane: Pane, id: string | null | undefined, card: boolean | undefined = undefined): HTMLElement | null {
    const list = pane.list();
    if (!list || !id) return null;
    const held = ([...list.children] as HTMLElement[]).find((row) =>
      (card !== true && deps.idsOf(row).includes(String(id))) ||
      (card !== false && row.hasAttribute("data-card") && row.getAttribute("data-id") === String(id)));
    const shown = held ? deps.shownRowFor(held) : null;
    return shown && deps.rowShown(shown) ? shown : null;
  }

  const markOf = (pane: Pane): KeySelection | null => pane.marks.get(pane.context()) || null;

  const markedRow = (pane: Pane, mark: KeySelection | null): HTMLElement | null => (mark ? paneRowFor(pane, mark.id, mark.card) : null);

  const markNames = (mark: KeySelection | null, row: HTMLElement): boolean => Boolean(mark) && (mark as KeySelection).id === row.getAttribute("data-id") &&
    ((mark as KeySelection).card === undefined || (mark as KeySelection).card === row.hasAttribute("data-card"));

  /** Note `row` as the selection of the list `pane` is showing, without making `pane` the one in use. */
  function setMark(pane: Pane, row: HTMLElement, drawn: boolean): void {
    pane.marks.set(pane.context(), { id: String(row.getAttribute("data-id")), card: row.hasAttribute("data-card"),
      drawn, index: paneRows(pane).indexOf(row) });
  }

  /** Note `row` as the selection of the list `pane` is showing, `pane` as the one in use, and draw it. */
  function markRow(pane: Pane, row: HTMLElement, drawn: boolean): void {
    const was = markOf(pane);
    setMark(pane, row, drawn);
    // Another message chosen — by the keys, the mouse, a tap, the focus or a jump — is the reader acting
    // in the list. `#243 filter-keeps-selection`. A redraw keeping the same one (`keepSelection`) is not.
    if (!markNames(was, row)) deps.readerActedIn(pane);
    deps.takeLastPane(pane);
    drawSelection();
  }

  function releaseRow(pane: Pane, row: HTMLElement | null = null): void {
    if (pane.row && pane.row !== row && pane.row.getAttribute("tabindex") === "0") pane.row.removeAttribute("tabindex");
    if (pane.row && pane.row !== row) deps.rowTimeTabStop(pane.row, false);
    pane.row = row;
  }

  function activePane(): Pane | null {
    if (deps.lastPane().list() !== null) return deps.lastPane();
    return deps.keyPanes().find((pane) => pane.list() !== null) || null;
  }

  function drawSelection(): void {
    const active = activePane();
    for (const pane of deps.keyPanes()) {
      const mark = pane === active ? markOf(pane) : null;
      const held = pane.row;
      let row: HTMLElement | null = null;
      if (mark && mark.drawn) {
        row = held && markNames(mark, held) && held.parentNode === pane.list() && deps.rowShown(held)
          ? held : markedRow(pane, mark);
      }
      if (pane.lit === row) continue;
      if (pane.lit) pane.lit.removeAttribute("data-selected");
      if (row) {
        row.setAttribute("data-selected", "true");
        deps.putOutLanding(row);
      }
      pane.lit = row;
    }
    // `#261 two-column-connectors`. The selection's link is drawn whatever is on screen, and every redraw
    // of the lists ends here (`keepSelection`), so the links are drawn again from here too.
    deps.noteLinkedSelection();
  }

  function takeRow(pane: Pane, row: HTMLElement, focus: boolean): void {
    releaseRow(pane, row);
    markRow(pane, row, true);
    row.setAttribute("tabindex", "0");
    deps.rowTimeTabStop(row, true);
    if (focus) row.focus({ preventScroll: true });
  }

  function nearRow(pane: Pane, index: number): HTMLElement | null {
    const rows = paneRows(pane);
    return rows.length ? rows[Math.max(0, Math.min(index, rows.length - 1))] as HTMLElement : null;
  }

  function rowInView(pane: Pane, row: HTMLElement): boolean {
    const box = row.getBoundingClientRect();
    return box.bottom > pane.head() && box.top < pane.foot();
  }

  function bringIntoView(pane: Pane, row: HTMLElement, from = 1): void {
    const area = pane.scroller();
    const box = row.getBoundingClientRect();
    const head = pane.head() + deps.landingGap();
    const foot = pane.foot() - deps.landingGap();
    if (from < 0 && box.height > foot - head) {
      if (box.bottom > foot || box.bottom < head) area.scrollTop += box.bottom - foot;
      return;
    }
    if (box.top < head) area.scrollTop += box.top - head;
    else if (box.bottom > foot) area.scrollTop += Math.min(box.bottom - foot, box.top - head);
  }

  function rowOf(pane: Pane, node: unknown): HTMLElement | null {
    const list = pane.list();
    if (!list || !node) return null;
    for (let at = node as Node | null; at && at !== list; at = at.parentNode) {
      if (at.parentNode === list) {
        const element = at as HTMLElement;
        return typeof element.hasAttribute === "function" && element.hasAttribute("data-id") ? element : null;
      }
    }
    return null;
  }

  return {
    paneFor,
    visibleScroller(node = document.activeElement) {
      if (deps.screen() === "main") return (paneFor(node) || (deps.keyPanes()[0] as Pane)).scroller();
      return deps.byId(`screen-${deps.screen()}`);
    },
    paneRows,
    rowOf,
    paneRowFor,
    markOf,
    markedRow,
    markNames,
    releaseRow,
    activePane,
    drawSelection,
    takeRow,
    noteRow(pane, row) {
      releaseRow(pane);
      markRow(pane, row, false);
    },
    currentRow(pane, node) {
      const held = rowOf(pane, node);
      if (held && deps.rowShown(held)) return held;
      return markedRow(pane, markOf(pane));
    },
    drawnRow(pane) {
      const mark = markOf(pane);
      return mark && mark.drawn ? markedRow(pane, mark) : null;
    },
    nearRow,
    rowInView,
    firstRowInView(pane) {
      const seen = paneRows(pane).filter((row) => rowInView(pane, row));
      const head = pane.head();
      const foot = pane.foot();
      return seen.find((row) => {
        const box = row.getBoundingClientRect();
        return box.top >= head - 1 && box.bottom <= foot + 1;
      }) || seen[0] || null;
    },
    selectRow(pane, row, bring = true, from = 1) {
      takeRow(pane, row, true);
      if (bring) bringIntoView(pane, row, from);
    },
    bringIntoView,
    keepSelection() {
      const active = activePane();
      for (const pane of deps.keyPanes()) {
        const held = pane.row;
        const list = pane.list();
        if (held === null || list === null) continue;
        if (held.parentNode === list && deps.rowShown(held)) continue;
        releaseRow(pane);
        const mark = markOf(pane);
        if (!mark || !mark.drawn) continue;
        const had = document.activeElement === held;
        const lost = had || deps.focusLost();
        const row = markedRow(pane, mark) || (lost ? nearRow(pane, mark.index) : null);
        if (!row) continue;
        pane.row = row;
        // Not `markRow`: a redraw of both columns must not make whichever came last the one in use.
        setMark(pane, row, true);
        row.setAttribute("tabindex", "0");
        deps.rowTimeTabStop(row, true);
        if (had || (lost && pane === active)) row.focus({ preventScroll: true });
      }
      // The new row of the selected message is lit, the old one having gone with the redraw. A selection
      // handed on here was nobody's choice, and glides nothing: only the pointer resting on a selection arms
      // a glide (`dwellColumnGlide`). `#280 scroll-stability`.
      drawSelection();
    },
  };
}

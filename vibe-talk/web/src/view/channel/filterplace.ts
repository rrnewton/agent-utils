// A filter changed: the reader's message stays where it is on the screen (`#243
// filter-keeps-selection`, `#239 single-selection`, `#212 link-filter`, `#221 read-modes`,
// `#262 search-load-older`, `#727 vt-selection-quick-reply`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel for the plan's step 17): one rule, applied
// by one helper, for every change of what the list shows. The anchor before the change, its
// row put back after it, the hold while the reader has not acted, and the walk back to an
// anchor older than everything loaded. The columns, the panes and the list are handed in;
// the words are the page's.

import { createFilterChanges } from "./filterchanges.ts";
import type { FilterChanges } from "./filterchanges.ts";
import { idsOf, isMessageRow, standingRow } from "./rows.ts";
import { nodeWithin } from "../dom/index.ts";
import { timeOf } from "../../core/channels/index.ts";

/** The message the last filter change kept in place (`freshFilterState`). */
export interface FilterState {
  hold: FilterHold | null;
}

/** A fresh record of a column's filters' hold (`freshFilterState`). */
export function freshFilterState(): FilterState {
  return { hold: null };
}

/** The hold of the last filter change (`filterState().hold`). */
export interface FilterHold {
  key: string;
  row: HTMLElement | null;
  ids: string[] | null;
  offset: number;
  at: number;
  origin: FilterAnchor;
  mark: string | null;
}

/** Where the reader was before a filter change (`filterAnchor`). */
export interface FilterAnchor {
  newest: boolean;
  row: HTMLElement | null;
  ids: string[];
  selected: string | null;
  drawn: boolean;
  focused: boolean;
  time: number;
  offset: number;
  alternate: FilterAnchor | null;
}

/** A pane's selection mark, as the filter place reads it. */
export interface FilterMark {
  id: string;
  drawn?: boolean;
}

/** A column's pane, as the filter place reads it. */
export interface FilterPane {
  marks: Map<string, { id: string; card: boolean; drawn: boolean; index: number }>;
  context(): string;
  head(): number;
  foot(): number;
  head(): number;
  foot(): number;
}

/** What the filter-kept place is handed by the page. */
export interface FilterPlaceDependencies {
  el(id: string): HTMLElement;
  activeFilter(): FilterState;
  activeColumn(): unknown;
  columnSide(): boolean;
  mainColumn(): unknown;
  sideColumn(): unknown;
  inColumn<T>(column: unknown, act: () => T): T;
  eachColumn(act: () => void): void;
  screen(): string;
  view(): string;
  renderOlderControl(): void;
  dismissStatus(): void;
  filterPlace(): string;
  pinsView(): { only: boolean };
  visibleList(): HTMLElement;
  listEmptied(): boolean;
  atBottom(area: HTMLElement, slack?: number): boolean;
  /** How far above the foot rest still counts as following it (`FOLLOW_REST_PX`). */
  restSlackPx: number;
  quickReplyHolds(area: HTMLElement): boolean;
  endSearchWalks(): void;
  scrollToNewest(): void;
  renderJumpNewest(): void;
  guardQuietly<A extends unknown[]>(fn: (...args: A) => unknown): (...args: A) => unknown;
  setStatus(text: string): void;
  sidePane(): FilterPane;
  channelPane(): FilterPane;
  markOf(pane: FilterPane): FilterMark | null;
  markedRow(pane: FilterPane, mark: FilterMark | null): HTMLElement | null;
  rowInView(pane: FilterPane, row: HTMLElement): boolean;
  takeRow(pane: FilterPane, row: HTMLElement, focus: boolean): void;
  rowOf(pane: FilterPane, row: HTMLElement): HTMLElement | null;
  paneRows(pane: FilterPane): HTMLElement[];
  releaseRow(pane: FilterPane, row?: HTMLElement): void;
  rowTimeTabStop(row: HTMLElement, on: boolean): void;
  drawSelection(): void;
  hidingRead(): boolean;
  threading(): boolean;
  timelineRows(): { messages: readonly { id: unknown; timestamp?: unknown }[] };
  olderPages(): { moreAbove: boolean; cursor: unknown };
  loadOlder(): Promise<void> | null;
  walkCursorOwed(): boolean;
  channelContextKey(): string;
  scrollAnchor(area: HTMLElement): HTMLElement | null;
  listPlaceTop(area: Element): number;
  floatingClearance(area: Element): number;
  messageTop(row: HTMLElement, id: string | null): number;
  rowMessages(row: HTMLElement): readonly { timestamp?: unknown }[];
  clearEntryLanding(): void;
  sideShowing(): boolean;
}

/** The filter-kept place, as the page calls it. */
export interface FilterPlace {
  readonly filterChanges: FilterChanges;
  filterKey(): string;
  filterState(): FilterState;
  endFilterWalk(): void;
  anchorAt(row: HTMLElement, selection?: FilterMark | null): FilterAnchor;
  anchorHolder(anchor: FilterAnchor): HTMLElement | null;
  anchorRow(anchor: FilterAnchor): HTMLElement | null;
  nearestShownRow(anchor: FilterAnchor): HTMLElement | null;
  currentFilterHold(): { hold: FilterHold; row: HTMLElement | null } | null;
  heldFilterRow(): HTMLElement | null;
  readerActed(): void;
  followsNewest(area: HTMLElement): boolean;
  keepFilterHold(): boolean;
  filterAnchor(): FilterAnchor | null;
  keepingFilterPlace<T>(change: () => T): T;
  placeFilterAnchor(anchor: FilterAnchor | null, wasEmptied?: boolean, walked?: boolean): void;
  settlePendingFilterPlace(): void;
}

/** Make the filter-kept place from the page's columns, panes and list. */
export function createFilterPlace(deps: FilterPlaceDependencies): FilterPlace {
  const filterChanges = createFilterChanges();

  /** The column being worked on's record of its filters' hold (`freshFilterState`). */
  function filterState(): FilterState {
    return deps.activeFilter();
  }


  /** How many older pages a filter change reads, at most, to find the message it is keeping. */
  const FILTER_WALK_PAGES = 10;

  /** What the status line says while a walk back is under way: put away with the walk when it ends early. */
  const FILTER_WALK_STATUS = "Loading earlier messages…";

  /**
   * End the walk back for a filter change, where it is: another change, a jump, a view or channel, or the
   * reader's own act has taken over. Its line on the status strip goes with it, unless something has
   * said something else there since.
   */
  function endFilterWalk(): void {
    if (!filterChanges.endWalk()) return;
    deps.inColumn(deps.mainColumn(), deps.renderOlderControl);
    if (!deps.el("status-line").hidden && deps.el("status").textContent === FILTER_WALK_STATUS) deps.dismissStatus();
  }

  /** Which list a filter's place is in: the view on screen, and whether the Pinned list stands in for it. */
  const filterKey = () => JSON.stringify([deps.filterPlace(), deps.pinsView().only]);

  /** The anchor `row` makes, the selection's details given in `selection` when it is the selected row. */
  function anchorAt(row: HTMLElement, selection: FilterMark | null = null): FilterAnchor {
    const messages = deps.rowMessages(row);
    const ids = idsOf(row);
    const kept = selection ? [selection.id, ...ids.filter((id) => id !== selection.id)] : ids;
    return {
      newest: false,
      row,
      ids: kept,
      selected: selection ? selection.id : null,
      drawn: Boolean(selection && selection.drawn),
      focused: Boolean(selection && nodeWithin(row, document.activeElement)),
      time: messages.length ? timeOf(messages[0], "timestamp") : NaN,
      // Of the message itself, which is the row's top but in a combined row (`messageTop`).
      // From the list's place, not its top edge, which the search moves (`listPlaceTop`).
      offset: deps.messageTop(row, kept.length ? (kept[0] as string) : null) - deps.listPlaceTop(deps.el("scroll-area")),
      alternate: null,
    };
  }

  /**
   * The row holding the anchor's message in the list on screen, shown or not: the same element when
   * the list kept it, else the row holding its selected message, else any of its messages.
   */
  function anchorHolder(anchor: FilterAnchor): HTMLElement | null {
    const list = deps.visibleList();
    if (anchor.row && anchor.row.parentNode === list) return anchor.row;
    const rows = [...list.children] as HTMLElement[];
    for (const id of anchor.ids) {
      const found = rows.find((row) => idsOf(row).includes(id));
      if (found) return found;
    }
    return null;
  }

  /** The row on screen standing for the anchor's message, or null when the list does not show it. */
  const anchorRow = (anchor: FilterAnchor): HTMLElement | null => standingRow(anchorHolder(anchor)) as HTMLElement | null;

  /**
   * The message shown nearest the anchor's, for an anchor the new filter hides: in the list's own order
   * when the list holds its row — the next one shown first, then the one before, then one further each
   * way — and else by time, the later first when two are as near.
   */
  function nearestShownRow(anchor: FilterAnchor): HTMLElement | null {
    const rows = ([...deps.visibleList().children] as HTMLElement[]).filter(isMessageRow);
    const holder = anchorHolder(anchor);
    const at = holder ? rows.indexOf(holder) : -1;
    if (at >= 0) {
      for (let step = 1; step < rows.length; step += 1) {
        for (const index of [at + step, at - step]) {
          const shown = index >= 0 && index < rows.length ? standingRow(rows[index] as HTMLElement) as HTMLElement | null : null;
          if (shown) return shown;
        }
      }
      return null;
    }
    if (!Number.isFinite(anchor.time)) return null;
    let best: HTMLElement | null = null;
    let bestGap = Infinity;
    let bestLater = false;
    for (const row of rows) {
      const shown = standingRow(row) as HTMLElement | null;
      const times = deps.rowMessages(row).map((message) => timeOf(message, "timestamp")).filter(Number.isFinite);
      if (!shown || times.length === 0) continue;
      const first = Math.min(...times);
      const last = Math.max(...times);
      const gap = anchor.time < first ? first - anchor.time : anchor.time > last ? anchor.time - last : 0;
      const later = first >= anchor.time;
      if (gap < bestGap || (gap === bestGap && later && !bestLater)) {
        best = shown;
        bestGap = gap;
        bestLater = later;
      }
    }
    return best;
  }

  /**
   * The hold of the last filter change, while it still holds — the same list on the main screen, and
   * the reader not moved: the list where the change left it, or the kept row where the change put it —
   * as `{ hold, row }`, `row` null for a list the filters emptied, or one its message has gone from.
   * Null, and the hold forgotten, once the reader has moved.
   */
  function currentFilterHold(): { hold: FilterHold; row: HTMLElement | null } | null {
    const hold = filterState().hold;
    if (hold === null) return null;
    if (deps.screen() !== "main" || hold.key !== filterKey()) {
      filterState().hold = null;
      return null;
    }
    if (hold.ids === null) return deps.listEmptied() ? { hold, row: null } : null;
    const area = deps.el("scroll-area");
    const row = anchorRow({ row: hold.row, ids: hold.ids } as FilterAnchor);
    const unmoved = Math.abs(area.scrollTop - hold.at) <= 1 || (row !== null &&
      Math.abs(row.getBoundingClientRect().top - deps.listPlaceTop(area) - hold.offset) <= 1);
    if (!unmoved) {
      filterState().hold = null;
      return null;
    }
    // `row` null: its message has gone from the list — a read of what Hide read leaves — under a reader
    // who has not moved, and the anchor that change was asked to keep is placed again (`keepFilterHold`).
    return { hold, row };
  }

  /** The row a filter change is holding in place now, or null. What the place-keeping below asks first. */
  function heldFilterRow(): HTMLElement | null {
    const held = currentFilterHold();
    return held ? held.row : null;
  }

  /**
   * The reader did something in the list of the column being worked on: pressed in it, chose another
   * message, opened or closed one, pressed a key that acts on the messages. The message the last filter
   * change held is let go, and the place-keeping measures from wherever the reader is now — their own act
   * on a message from that message, as it would have been with no filter change before it — and a walk
   * back still under way for that change ends where it is.
   *
   * From review: a hold that lasted until the reader scrolled was still every `preservingScroll`'s
   * anchor after they had moved on, so folding a message just above the kept one, fully on screen, put
   * the first 140px of the message folded above the screen. The reader's own act is the end of what a
   * filter change put in place.
   */
  function readerActed(): void {
    filterState().hold = null;
    if (!deps.columnSide()) endFilterWalk();
  }

  /**
   * Whether the reader on the list's newest line is following it: not while a filter change holds a
   * message in place, which nothing but the reader moves. Every "follow the newest line" asks this.
   */
  function followsNewest(area: HTMLElement): boolean {
    // `#727 vt-selection-quick-reply`. Never a list held still for a quick reply.
    if (deps.quickReplyHolds(area)) return false;
    // Rest, not the slack (`#1209`'s near-foot case, owner reply 298): a reader parked above
    // the list's own end room is reading, and an arrival offers them the jump instead of
    // taking them to the foot.
    return deps.atBottom(area, deps.restSlackPx) && heldFilterRow() === null;
  }

  /**
   * Put the held row back where the filter change put it, after something below or above it changed —
   * or, when its message has gone from the list, put the anchor that change was asked to keep in its
   * place again, as the change would have. False, moving nothing, when nothing is held.
   */
  function keepFilterHold(): boolean {
    const held = currentFilterHold();
    if (held === null || held.hold.ids === null) return false;
    if (held.row === null) {
      placeFilterAnchor(held.hold.origin, false, true);
      return filterState().hold !== null;
    }
    const row = held.row;
    const area = deps.el("scroll-area");
    area.scrollTop += row.getBoundingClientRect().top - deps.listPlaceTop(area) - held.hold.offset;
    filterState().hold = { ...held.hold, row, ids: idsOf(row).length ? idsOf(row) : held.hold.ids, at: area.scrollTop };
    return true;
  }

  /** Measure the anchor of a filter change in the column being worked on, or null with no list on screen. */
  function filterAnchor(): FilterAnchor | null {
    if (deps.screen() !== "main") return null;
    const area = deps.el("scroll-area");
    const pane = deps.view() === "discord" ? (deps.columnSide() ? deps.sidePane() : deps.channelPane()) : null;
    const mark = pane ? deps.markOf(pane) : null;
    const chosen = pane ? deps.markedRow(pane, mark) : null;
    const showing = Boolean(pane && mark && chosen && deps.rowInView(pane, chosen));
    const held = currentFilterHold();
    // The anchor the last change was asked to keep, while the reader has neither moved nor chosen
    // another message since: a filter turned on and off again is a round trip. Focused only where the
    // focus is now.
    if (held && (!showing || held.hold.mark === (mark ? mark.id : null))) {
      const origin = held.hold.origin;
      const asked = { ...origin, focused: Boolean(origin.selected !== null && held.row &&
        nodeWithin(held.row, document.activeElement)) };
      return held.row === null ? asked : { ...asked, alternate: anchorAt(held.row) };
    }
    if (showing && pane && mark && chosen) return anchorAt(chosen, mark);
    if (deps.atBottom(area)) {
      return { newest: true, row: null, ids: [], selected: null, drawn: false, focused: false, time: NaN, offset: 0,
        alternate: null };
    }
    const head = pane ? pane.head() : deps.floatingClearance(area);
    const foot = pane ? pane.foot() : area.getBoundingClientRect().top + (area.clientHeight || 0);
    const first = [...deps.visibleList().children].find((row) => {
      if (!isMessageRow(row) || standingRow(row) !== row) return false;
      const box = row.getBoundingClientRect();
      return box.bottom > head && box.top < foot;
    });
    return first ? anchorAt(first as HTMLElement) : null;
  }

  /**
   * Run `change`, a change of what the list shows, keeping the reader's message where it is on the
   * screen in every column showing messages. A `change` that returns a promise — a read — is waited
   * for, and the reader placed when it lands. Returns what `change` returns.
   *
   * @template T
   * @param {() => T} change
   * @returns {T}
   */
  function keepingFilterPlace<T>(change: () => T): T {
    if (filterChanges.changing()) return change();
    /** @type {Array<[ColumnFields, FilterAnchor | null, boolean]>} */
    const kept: Array<[unknown, FilterAnchor | null, boolean]> = [];
    deps.eachColumn(() => {
      kept.push([deps.activeColumn(), filterAnchor(), deps.listEmptied()]);
      // A landing a thread's entry made, or still owes, is spent: the reader has acted since. `#221`.
      deps.clearEntryLanding();
    });
    // A walk back for an earlier change is over: this one decides where the reader goes.
    endFilterWalk();
    const result = filterChanges.whileChanging(change);
    const place = (landed = false): void => {
      for (const [column, anchor, wasEmptied] of kept) {
        if (column === deps.sideColumn() && !deps.sideShowing()) continue;
        deps.inColumn(column, () => {
          // Once a read has landed, only for a reader still where the change put them.
          if (!landed || currentFilterHold() !== null) placeFilterAnchor(anchor, wasEmptied);
        });
      }
    };
    // At once, on the rows on screen; and for a change that reads, again on the rows the read drew.
    place();
    if (result && typeof (result as { then?: unknown }).then === "function") {
      return (result as unknown as Promise<unknown>).then((value) => {
        place(true);
        return value;
      }) as T;
    }
    return result;
  }

  /**
   * Put the reader back on `anchor` in the column being worked on, after a filter change. `wasEmptied`
   * is whether the filters had emptied the list before it; `walked` that older pages were read for the
   * anchor already, and none is read again.
   *
   * `#212 link-filter`, from review, still holds where nothing is left: an EMPTIED list is taken to its
   * head, where the sentence saying why is. Its rows are gone but the list is not short — the channel's
   * composer stays below every list — and a reader left anywhere else had that sentence under the bar
   * or the kinds of link it names, or above the screen. The anchor is kept for the change that fills it
   * again. A list filled again with no anchor to go back to opens at its newest line, as a list does.
   */
  function placeFilterAnchor(anchor: FilterAnchor | null, wasEmptied = false, walked = false): void {
    if (deps.screen() !== "main") return;
    const area = deps.el("scroll-area");
    if (deps.listEmptied()) {
      area.scrollTop = 0;
      // Kept for what fills the list again — the newest line too: Pinned turned on there before the pins
      // were read shows "Loading…" until they land, and they land at the newest pin, not the oldest.
      filterState().hold = anchor
        ? { key: filterKey(), row: null, ids: null, offset: 0, at: area.scrollTop, origin: anchor, mark: null } : null;
      deps.renderJumpNewest();
      return;
    }
    if (!anchor || anchor.newest) {
      filterState().hold = null;
      if (anchor || wasEmptied) deps.scrollToNewest();
      else deps.renderJumpNewest();
      return;
    }
    let row = anchorRow(anchor);
    let offset = anchor.offset;
    let id = anchor.ids.length ? (anchor.ids[0] as string) : null;
    if (!row && anchor.alternate) {
      row = anchorRow(anchor.alternate);
      offset = anchor.alternate.offset;
      id = anchor.alternate.ids.length ? (anchor.alternate.ids[0] as string) : null;
    }
    if (!row && !walked && olderThanLoaded(anchor)) {
      filterState().hold = null;
      deps.guardQuietly(walkToFilterAnchor)(anchor);
      return;
    }
    const substitute = row ? null : nearestShownRow(anchor);
    const placed = row || substitute;
    if (!placed) {
      filterState().hold = null;
      deps.renderJumpNewest();
      return;
    }
    // The message's own top where it is part of a combined row (`messageTop`), the row's otherwise.
    area.scrollTop += deps.messageTop(placed, row ? id : null) - deps.listPlaceTop(area) - offset;
    carrySelection(anchor, placed, substitute !== null);
    const pane = deps.view() === "discord" ? (deps.columnSide() ? deps.sidePane() : deps.channelPane()) : null;
    const mark = pane ? deps.markOf(pane) : null;
    filterState().hold = {
      key: filterKey(), row: placed, ids: idsOf(placed), at: area.scrollTop, origin: anchor,
      offset: placed.getBoundingClientRect().top - deps.listPlaceTop(area), mark: mark ? mark.id : null,
    };
    deps.renderJumpNewest();
    if (substitute && anchor.selected !== null) deps.setStatus(filterHiddenSentence(anchor, walked));
  }

  /**
   * The selection goes with the message a filter change kept: the selection of the list now drawn, its
   * ring and its focus as they were (`#239 single-selection`). For a message the filter hides, the
   * selection stays on it, to come back with it, unless the focus was in its row: then the focus and the
   * selection go to the message shown in its place, as `keepSelection` hands them on.
   */
  function carrySelection(anchor: FilterAnchor, row: HTMLElement, substituted: boolean): void {
    if (anchor.selected === null || deps.view() !== "discord") return;
    const pane = deps.columnSide() ? deps.sidePane() : deps.channelPane();
    if (deps.rowOf(pane, row) !== row) return;
    if (substituted) {
      if (anchor.focused) deps.takeRow(pane, row, true);
      return;
    }
    pane.marks.set(pane.context(), { id: anchor.selected, card: row.hasAttribute("data-card"), drawn: anchor.drawn,
      index: deps.paneRows(pane).indexOf(row) });
    if (anchor.drawn) {
      deps.releaseRow(pane, row);
      row.setAttribute("tabindex", "0");
      deps.rowTimeTabStop(row, true);
      // The focus too, when it was on the row: a change that took the row away dropped it to the page,
      // as a redraw does (`keepSelection`). A control the reader clicked keeps it, as #239 has it.
      if (anchor.focused) row.focus({ preventScroll: true });
    } else {
      deps.releaseRow(pane);
    }
    deps.drawSelection();
  }

  /**
   * What the status line says when the selected message is not shown after a filter change, and the
   * nearest message is in its place: why, in words that fit one line of a 360px phone (`setStatus`).
   */
  function filterHiddenSentence(anchor: FilterAnchor, walked: boolean): string {
    if (deps.pinsView().only) return "Not pinned — the nearest pin is here.";
    if (anchorHolder(anchor)) return "Filtered out — the nearest is here.";
    if (walked && olderThanLoaded(anchor)) return `Over ${FILTER_WALK_PAGES} pages back — oldest is here.`;
    // Under Hide read a channel without threads reads only what is left, so a message it hides is in
    // no list this page holds; with threads it is held, and drawn in none.
    if (deps.hidingRead() && (!deps.threading() || deps.timelineRows().messages.some((message) => anchor.ids.includes(String(message.id))))) {
      return "Hide read hides it — nearest is here.";
    }
    return "Not in this view — nearest is here.";
  }

  /** Whether the anchor's message is older than everything the channel's list has loaded, with more above it. */
  function olderThanLoaded(anchor: FilterAnchor): boolean {
    if (deps.columnSide() || deps.view() !== "discord" || deps.pinsView().only || deps.olderPages().moreAbove !== true ||
        !Number.isFinite(anchor.time)) return false;
    const loaded = deps.threading() ? deps.timelineRows().messages : ([...deps.el("discord-log").children] as HTMLElement[]).flatMap(deps.rowMessages);
    const times = loaded.map((message) => timeOf(message, "timestamp")).filter(Number.isFinite);
    return times.length === 0 || anchor.time < Math.min(...times);
  }

  /**
   * Read older pages until the anchor's message is loaded, or the history reaches back past it, and put
   * the reader on it then — at most `FILTER_WALK_PAGES` of them. Meanwhile the reader is at the head of
   * the list, where the step-back line says "Loading earlier messages…". Another filter change, another
   * view or another channel, a jump, the reader's own act in the list (`readerActed`), or the reader
   * scrolling while a page is read, ends the walk where it is: it never pulls them back.
   */
  async function walkToFilterAnchor(anchor: FilterAnchor): Promise<void> {
    // `#262 search-load-older`. A walk for the search gives way to this one.
    deps.endSearchWalks();
    const walk = filterChanges.startWalk(deps.channelContextKey());
    const area = deps.el("scroll-area");
    const current = () => filterChanges.isWalk(walk) && walk.context === deps.channelContextKey() && !deps.pinsView().only &&
      deps.screen() === "main" && deps.view() === "discord";
    area.scrollTop = 0;
    deps.renderOlderControl();
    deps.renderJumpNewest();
    deps.setStatus(FILTER_WALK_STATUS);
    let pages = 0;
    let scrolled = false;
    try {
      while (pages < FILTER_WALK_PAGES && current() && olderThanLoaded(anchor) &&
          (deps.olderPages().cursor || deps.walkCursorOwed())) {
        pages += 1;
        // The row the walk left the reader on, at the head of the list. A step keeps the row the reader
        // is on where it is (`preservingScroll`), so this one anywhere else afterwards is a reader who
        // scrolled while the page was read. A list too short to scroll follows its newest line instead,
        // and has nothing to scroll away in.
        const top = deps.atBottom(area) ? null : deps.scrollAnchor(area);
        const was = top ? top.getBoundingClientRect().top : 0;
        await deps.loadOlder();
        if (!current()) return;
        if (top && top.parentNode && Math.abs(top.getBoundingClientRect().top - was) > 1) {
          scrolled = true;
          return;
        }
        // The step keeps the rows the reader was looking at; the walk keeps the line saying it goes on.
        area.scrollTop = 0;
      }
    } finally {
      if (filterChanges.isWalk(walk)) {
        const place = !scrolled && walk.context === deps.channelContextKey() && !deps.pinsView().only;
        endFilterWalk();
        if (place) {
          placeFilterAnchor(anchor, false, true);
          if (anchorRow(anchor)) {
            deps.setStatus(anchor.selected !== null ? "Here is the selected message." : "Here is where you were.");
          }
        }
      }
    }
  }

  /**
   * A list the filters emptied has rows again by something other than the reader — the pins landing
   * under a Pinned filter turned on before they had — and the anchor of the change that emptied it is
   * placed now. A filter change it waited through has placed the reader already.
   */
  function settlePendingFilterPlace(): void {
    const hold = filterState().hold;
    if (hold === null || hold.ids !== null || hold.key !== filterKey() || deps.listEmptied() || filterChanges.changing()) return;
    placeFilterAnchor(hold.origin, true);
  }

  /**
   * Open or close the field.
   *
   * Closing CLEARS, rather than leaving a query parked behind a closed control. A filter still in
   * force with nothing on screen saying so is the worst state this feature can be in: the reader
   * concludes the messages are gone. Closing is a change of what the list shows, and keeps the
   * reader's message where it is (`keepingFilterPlace`).
   */


  return {
    filterChanges, filterKey,
    filterState, endFilterWalk, anchorAt, anchorHolder, anchorRow, nearestShownRow,
    currentFilterHold, heldFilterRow, readerActed, followsNewest, keepFilterHold, filterAnchor,
    keepingFilterPlace, placeFilterAnchor, settlePendingFilterPlace,
  };
}

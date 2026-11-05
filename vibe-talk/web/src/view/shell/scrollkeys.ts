// The scroll keys (`#224 keyboard-shortcuts`, `#338 column-overscroll`; `#624 voice-page-modularity`, moved from
// the legacy script into view/shell for the plan's step 17): the page keys, Home, End, Space and the arrows that
// no binding took, sent to the list on screen when they came from somewhere that cannot use them, and the
// bindings that move a list to its top or its newest line. Which list is on screen, its foot, the room past it
// and the selection are the page's.

import { nodeWithin } from "../dom/index.ts";
import { swallow } from "./chords.ts";
import type { KeyEvent } from "./chords.ts";

/** How far a page key moves a list: seven-eighths of what it shows, as Chromium pages a scroller. */
export function pageStep(area: { readonly clientHeight?: number }): number {
  return Math.max(1, Math.round((area.clientHeight || 0) * 0.875));
}

/** The keys a browser scrolls with. Space is " ", as `KeyboardEvent.key` spells it. */
export const SCROLL_KEYS: readonly string[] = ["PageUp", "PageDown", "Home", "End", " ", "ArrowUp", "ArrowDown"];

/** A keydown, as the scroll keys read it. */
export interface ScrollKeyEvent extends KeyEvent {
  readonly repeat?: boolean;
}

/** A list the keys move through, as far as the scroll keys read it. */
export interface ScrolledPane<Row> {
  /** The selected row, or null. */
  readonly row: Row | null;
  /** The list's scroller. */
  scroller(): HTMLElement;
}

export interface ScrollKeysDependencies<Pane extends ScrolledPane<HTMLElement>> {
  /** The main screen's list scroller of the column the code is working in (`inColumn`): the left one's, or the right one's. */
  scrollArea(): HTMLElement;
  /** The right column's scroller, as the left column's code sees it. */
  sideScroll(): HTMLElement;
  /** To the foot of `area`. */
  toFoot(area: HTMLElement): void;
  /** End a walk back a filter change was making (`#243 filter-keeps-selection`). */
  endFilterWalk(): void;
  /** Out of any room past the right column's foot (`#338 column-overscroll`). */
  closeOverscroll(): void;
  /** Open the room past the foot of `area`, for a reader resting there. */
  openOverscroll(area: HTMLElement): void;
  /** The main list to its newest line, as the Newest chip takes it. */
  scrollToNewest(): void;
  /** The list on screen that `node` would scroll, or null. */
  visibleScroller(node: HTMLElement): HTMLElement | null;
  /** Whether `node` is a field that types. */
  isTextField(node: HTMLElement): boolean;
  /** Whether a field holds more than it shows, so the page keys are its own. */
  scrollsItself(node: HTMLElement): boolean;
  /** Whether `node` answers its own keys (a picker, a slider, the width handle). */
  keepsOwnKeys(node: HTMLElement): boolean;
  /** Whether Space presses `node`. */
  pressedBySpace(node: HTMLElement): boolean;
  /** Whether the main screen is up. */
  onMain(): boolean;
  /** The list's pane `node` is in, or null. */
  paneFor(node: HTMLElement): Pane | null;
  /** The rows of `pane`'s list. */
  paneRows(pane: Pane): HTMLElement[];
  /** Select `row` of `pane`; `bring` false leaves the scroll where it is. */
  selectRow(pane: Pane, row: HTMLElement, bring: boolean): void;
  /** The ring follows a page key's scroll from the selected `row` once it comes to rest; `up` for PgUp. */
  followPageScroll(pane: Pane, row: HTMLElement, up: boolean): void;
}

/** The scroll keys. */
export interface ScrollKeys {
  /** To the end of `area`. On the main screen exactly what the Newest chip does, which End is. */
  toNewest(area: HTMLElement): void;
  /**
   * The reader's own way to the newest line — the Newest chip, End — which also ends a walk back a
   * filter change was making for a message older than anything loaded (`#243 filter-keeps-selection`):
   * the reader has gone somewhere else, and is not brought back when it lands.
   */
  jumpToNewest(): void;
  /**
   * A scroll key no binding took, sent to the list on screen when it came from somewhere that cannot
   * use it.
   *
   *   * In a text field, Home, End, Space and the arrows are the field's. So are the page keys while
   *     the field holds more than it shows; otherwise they page the list, and the field keeps the
   *     focus and the text, so the reader is never stuck in a box.
   *   * On a picker, the arrows and Space choose and open, as they always did; the page keys and
   *     Home/End page the list rather than skipping options. A slider and the width handle keep all.
   *   * Inside the list, the browser scrolls it already; from the selected message, the ring follows
   *     the page once it comes to rest (`followPageScroll`).
   *   * A focused button keeps Space, which presses it.
   *   * Anywhere else (nothing focused, a control in the dock or the header) the list takes the
   *     focus, and the browser's own default, which runs after this, scrolls it.
   *
   * With Ctrl or Alt held, nothing: Ctrl+Home and Ctrl+End are a binding of their own, and Alt with
   * the arrows belongs to the channels. Cmd is let through, for the Cmd+Up and Cmd+Down of a Mac.
   */
  routeScrollKey(event: ScrollKeyEvent, target: HTMLElement): void;
  /** Ctrl+Home and Ctrl+End (Cmd on a Mac): the list to its top or its newest line, from anywhere. */
  jumpList(target: HTMLElement, newest: boolean): boolean;
  /**
   * Home and End over the channel: its top or its newest message, as the browser would scroll there,
   * and that message selected. Home asks for no more history than scrolling to the top does: the
   * scroll listener steps back once, as it always has. Elsewhere they are the browser's.
   */
  listEdge(target: HTMLElement, newest: boolean): boolean;
}

export function createScrollKeys<Pane extends ScrolledPane<HTMLElement>>(deps: ScrollKeysDependencies<Pane>): ScrollKeys {
  function toNewest(area: HTMLElement): void {
    if (area === deps.scrollArea()) jumpToNewest();
    else deps.toFoot(area);
  }

  function jumpToNewest(): void {
    deps.endFilterWalk();
    // `#338 column-overscroll`. Onto the foot, out of any room past it: where the list opens.
    if (deps.scrollArea() === deps.sideScroll()) deps.closeOverscroll();
    deps.scrollToNewest();
  }

  /** Move `area` for a page key, or Home or End, that the browser would have spent on something else. */
  function scrollForKey(area: HTMLElement, key: string): void {
    if (key === "Home") area.scrollTop = 0;
    else if (key === "End") toNewest(area);
    else area.scrollTop += (key === "PageUp" ? -1 : 1) * pageStep(area);
  }

  /**
   * `#338 column-overscroll`. A page key about to scroll `area` down — PgDn, Space — opens the room past the
   * right column's foot first, for a reader resting there: pressed afresh, never repeating as it is held, so
   * a held key stops on the foot as a fling of the wheel does, and the next press goes on. ↓ is a binding of
   * its own (`stepSelection`), which opens the room only where it goes on past the last message.
   */
  function openOverscrollForKey(area: HTMLElement, event: ScrollKeyEvent): void {
    const down = event.key === "PageDown" || (event.key === " " && !event.shiftKey);
    if (down && !event.repeat && !event.metaKey) deps.openOverscroll(area);
  }

  function routeScrollKey(event: ScrollKeyEvent, target: HTMLElement): void {
    const key = event.key as string;
    if (!SCROLL_KEYS.includes(key) || event.ctrlKey || event.altKey) return;
    const area = deps.visibleScroller(target);
    if (!area) return;
    const paging = key === "PageUp" || key === "PageDown";
    if (deps.isTextField(target)) {
      if (paging && !event.shiftKey && !event.metaKey && !deps.scrollsItself(target)) {
        swallow(event);
        openOverscrollForKey(area, event);
        scrollForKey(area, key);
      }
      return;
    }
    if (deps.keepsOwnKeys(target)) {
      const picker = String(target.tagName || "").toUpperCase() === "SELECT";
      if (picker && (paging || key === "Home" || key === "End") && !event.shiftKey && !event.metaKey) {
        swallow(event);
        openOverscrollForKey(area, event);
        scrollForKey(area, key);
      }
      return;
    }
    if (target && nodeWithin(area, target)) {
      openOverscrollForKey(area, event);
      const pane = deps.onMain() ? deps.paneFor(target) : null;
      if (pane && pane.row === target && (paging || key === " ")) {
        deps.followPageScroll(pane, target, key === "PageUp" || (key === " " && Boolean(event.shiftKey)));
      }
      return;
    }
    if (key === " " && deps.pressedBySpace(target)) return;
    openOverscrollForKey(area, event);
    if (typeof area.focus === "function") area.focus({ preventScroll: true });
  }

  return {
    toNewest,
    jumpToNewest,
    routeScrollKey,
    jumpList(target, newest) {
      const area = deps.visibleScroller(target);
      if (!area) return false;
      if (newest) toNewest(area);
      else area.scrollTop = 0;
      return true;
    },
    listEdge(target, newest) {
      const pane = deps.paneFor(target);
      const rows = pane ? deps.paneRows(pane) : [];
      if (!pane || rows.length === 0) return false;
      if (newest) toNewest(pane.scroller());
      else pane.scroller().scrollTop = 0;
      deps.selectRow(pane, rows[newest ? rows.length - 1 : 0] as HTMLElement, false);
      return true;
    },
  };
}

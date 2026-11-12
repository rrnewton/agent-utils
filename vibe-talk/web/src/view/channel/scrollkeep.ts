// Leaving the reader looking at the same thing across a change (`#47 scrollback-stability`, `#51
// reply-view`, `#206 pin-message`, `#207 scrollback-jump`, `#221 read-modes`, `#243
// filter-keeps-selection`, `#262 search-load-older`; `#624 voice-page-modularity`, moved from the legacy
// script into view/channel): the list on screen, the message the reader's eye is on, where they are
// captured as and put back by, and following a new turn only where they were already at the newest line.
// Explicit, rather than the browser's scroll anchoring, which a change above the viewport defeats. The
// filters' held row, the search walk's control, and the jump to the newest are the page's.

import { shownRowFor } from "./rows.ts";

/** Where the reader is: on the newest line, or which element their eye is on and how far down it sits. */
export interface ScrollMark {
  readonly pinned: boolean;
  readonly anchor: Element | null;
  readonly top: number;
  /** The anchor row's message id, where it has one: a redraw that replaces the row is re-found by it. */
  readonly anchorId?: string | null;
}

/** A walk for the search whose control holds still while it reads, as far as keeping the reader reads it. */
interface AnchoredWalk {
  anchored: boolean;
  chipTop: number;
}

/** What keeping the reader asks of the page. */
export interface ScrollKeepingDependencies {
  /** The page's element by id (`el`), in the column being worked on. */
  el(id: string): HTMLElement;
  /** Whether the main screen is the one shown (`currentScreen === "main"`). */
  onMain(): boolean;
  /** The view the reader is on: "discord" for the channel, else the call's. */
  currentView(): string;
  /** The view the channel shows in the column being worked on (`selection().view`). */
  channelView(): string;
  /** Whether the Pinned filter stands in for the channel's list (`pinsView().only`). */
  pinnedOnly(): boolean;
  /** The row a filter change holds in place, or null (`heldFilterRow`). */
  heldFilterRow(): HTMLElement | null;
  /** The row a jump just landed on, held as the place until the reader moves (`#1224`). Absent: unkept. */
  jumpAnchorRow?(): HTMLElement | null;
  /** The walk for the search in the column being worked on, or null. */
  searchWalk(): AnchoredWalk | null;
  /** Whether `area` is at its newest line (`atBottom`), within `slack` where one is named. */
  atBottom(area: HTMLElement, slack?: number): boolean;
  /** How far above the foot rest still counts as following it (`FOLLOW_REST_PX`). */
  restSlackPx: number;
  /** Take the reader to the newest line (`scrollToNewest`). */
  scrollToNewest(): void;
  /** Decide the jump to the newest from where the reader is (`renderJumpNewest`). */
  renderJumpNewest(): void;
  /** Raise the jump to the newest over `view`, counting `turns` new turns (`setJumpNewest`). */
  setJumpNewest(on: boolean, view: string, turns: number): void;
  /**
   * The reader's text selection in `area`, where one stands (`#1209 selection-survives-updates`):
   * the message it stands in and its ends' distances into that row's words, or null. And putting
   * such a selection back where the row was rebuilt (`restoreSelectionIn`). Absent: unkept.
   */
  selectionIn?(area: HTMLElement): { id: string; start: number; end: number } | null;
  restoreSelectionIn?(area: HTMLElement, selection: { id: string; start: number; end: number }): void;
}

/**
 * How many of `rows` — one list's, in their order — stand below the reading position: below the
 * last row wholly on screen between `head` and `foot`, the edges of the list's window as its
 * caller reads them. What the jump to the newest counts (`#1057 newest-count-chip`): how
 * much reading is left between here and the newest line, in the rows this view draws.
 *
 * Rows stand in list order, so their tops do not decrease: a binary search finds the first row
 * starting at or below `foot`, and only the rows on screen around it are measured — a count asked
 * for as the reader scrolls never measures the whole of a long history.
 */
export function rowsBelowReadingPosition(rows: readonly HTMLElement[], head: number, foot: number): number {
  let lo = 0;
  let hi = rows.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((rows[mid] as HTMLElement).getBoundingClientRect().top < foot) lo = mid + 1;
    else hi = mid;
  }
  let at = -1;
  for (let i = lo - 1; i >= 0; i -= 1) {
    const box = (rows[i] as HTMLElement).getBoundingClientRect();
    if (box.bottom <= head) break;
    if (box.top >= head - 1 && box.bottom <= foot + 1) {
      at = i;
      break;
    }
  }
  if (at < 0) {
    // Nothing stands wholly on screen — a row taller than the room, most likely. The reader is on
    // the last row any of which shows, and what follows it is below them.
    at = lo > 0 && (rows[lo - 1] as HTMLElement).getBoundingClientRect().bottom > head ? lo - 1 : -1;
  }
  return rows.length - 1 - at;
}

/** The keeper's commands and questions. */
export interface ScrollKeeping {
  /** The list the reader is actually looking at. Both panes share one scrolling element. */
  visibleList(): HTMLElement;
  /** The channel's rows as they are on screen: the Pinned filter's list while it is on, the channel's otherwise. */
  shownChannelList(): HTMLElement;
  /** The message the reader's eye is on: the first one not already scrolled off the top. */
  scrollAnchor(area: HTMLElement): Element | null;
  /** Run a mutation and leave the reader looking at the same thing afterwards. */
  preservingScroll(mutate: () => void): void;
  /**
   * Collapse or expand `row` in `area` under the per-act anchor choice (`#1251
   * scroll-single-owner`, `#1215 collapse-anchor-below`): a reader following the foot stays on
   * it; while the row's top is on screen the row itself is held; with its top above the
   * viewport the next drawn row is held and the history above absorbs the change; with
   * neither on screen (inside a row taller than the room) the collapsed row is centered.
   */
  preservingCollapse(area: HTMLElement, row: HTMLElement, head: number, foot: number, mutate: () => void): void;
  /**
   * Run a redraw of the channel's rows with the reader held where they are, and nothing
   * else decided: no jump recount, which the landing that asked for the redraw owns.
   */
  holdingRows(mutate: () => void): void;
  /** Take `area` to its foot: the one foot write, for the keepers that decide when (`#1251`). */
  toFoot(area: HTMLElement): void;
  /** Move `area` by `delta` px: the one relative write, for a holder that measured the move (`#1251`). */
  nudge(area: HTMLElement, delta: number): void;
  /** Put `area` at scroll position `top`: the one absolute write, for a keeper's measured foot (`#1251`). */
  toPosition(area: HTMLElement, top: number): void;
  /** A wheel turn's smooth scroll of `area` by `px`, by the plain write where no `scrollBy` stands (`#1251`). */
  wheelBy(area: HTMLElement, px: number): void;
  /** Glide `area` to position `top` with `behavior` (the reader's glide preference), by the plain write where no `scrollTo` stands (`#1251`). */
  glideTo(area: HTMLElement, top: number, behavior: string): void;
  /** Bring `element` into view at `block` (and `inline`, where a caller names it), where the browser offers it: the one `scrollIntoView` write (`#1251`). */
  intoView(element: Element, block: string, inline?: string): void;
  /** `preservingScroll` for a change that resizes the list's window or the room at its head. */
  holdingReader(mutate: () => void): void;
  /** Where the reader is: which message their eye is on, and how far down the viewport it sits. */
  captureScroll(): ScrollMark;
  /** Put the reader back where `captureScroll` found them. */
  restoreScroll(mark: ScrollMark): void;
  /** A turn arrived: follow it only if the reader had not scrolled away. */
  followIfPinned(pinned: boolean, turns?: number, view?: string): void;
}

/** Keeping the reader in the page's lists. */
export function createScrollKeeping(deps: ScrollKeepingDependencies): ScrollKeeping {
  const el = deps.el;
  /** Owner holds currently being applied; a redraw inside one is held by the outer hold. */
  let holdDepth = 0;

  function visibleList(): HTMLElement {
    return el(deps.currentView() === "discord"
      ? (deps.channelView() === "threads" && !deps.pinnedOnly() ? "thread-list" : shownChannelList().id)
      : "transcript");
  }

  function shownChannelList(): HTMLElement {
    return el(deps.pinnedOnly() ? "pinned-log" : "discord-log");
  }

  /**
   * This is the anchor everything below is measured against. A browser has its own scroll anchoring,
   * and it is exactly the wrong tool here — it is defeated by a mutation ABOVE the viewport, which
   * is precisely what collapsing a message the reader has already scrolled past is.
   */
  function scrollAnchor(area: HTMLElement): Element | null {
    const edge = area.getBoundingClientRect().top;
    const items = visibleList().children;
    for (const li of items) {
      // A row hidden in a collapsed run of read messages has no box to be looking at. `#221 read-modes`.
      if (li.getAttribute("data-read-run") === "member") continue;
      if (li.getBoundingClientRect().bottom > edge) {
        return li;
      }
    }
    return items.length > 0 ? items[items.length - 1] as Element : null;
  }

  /**
   * Capture the anchor's offset, mutate, measure it again, and move the scroll position by the
   * difference. Explicit, because the alternative is trusting browser scroll anchoring, and the
   * whole point is that it does not hold for a change made above the viewport.
   *
   * A reader who was already at the bottom is the one exception: for them "the same thing" IS the
   * newest line, so the view follows it.
   */
  function preservingScroll(mutate: () => void): void {
    const mark = captureScroll();
    // A rebuild inside takes the reader's text selection with the rows it replaces: kept, and
    // put back on the rebuilt row after (`#1209 selection-survives-updates`).
    const area = el("scroll-area");
    const selection = deps.selectionIn ? deps.selectionIn(area) : null;
    holdDepth += 1;
    try {
      mutate();
    } finally {
      holdDepth -= 1;
    }
    restoreScroll(mark);
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  /**
   * The error panel above #scroll-area, the freshness pill's room inside it: browser scroll anchoring
   * holds for neither, so the line being read, or the newest line, would move by the change's height:
   * 74-89px for a failed refresh's panel (`#33 error-banner-scroll-shift`).
   *
   * Not for a reader at the very top, nor off the main screen. At the top the first row is the
   * anchor, and holding it would scroll the header out of view: that reader is meant to see the
   * header move down. Off the main screen there is no list on screen to hold.
   */
  function holdingReader(mutate: () => void): void {
    if (deps.onMain() && el("scroll-area").scrollTop > 0) preservingScroll(mutate);
    else mutate();
  }

  /**
   * Split out of `preservingScroll` so that `#51 reply-view` can use the same mechanism across a
   * screen change, where the two halves are separated by everything the reader does on the reply
   * screen. One mechanism, two callers: an anchor is strictly better than a saved `scrollTop` here
   * as well, because hiding an element is allowed to reset its scroll position to zero — and a
   * restore expressed as a DELTA against the anchor is correct whether the browser did that or not.
   */
  function captureScroll(): ScrollMark {
    const area = el("scroll-area");
    // `#1224 mention-jump-in-view`. A jump's landed row is the reader's latest explicit place:
    // what changes after it holds that row first, not the row that happens to stand at the top.
    const landed = deps.jumpAnchorRow ? deps.jumpAnchorRow() : null;
    if (landed) return { pinned: false, anchor: landed, top: landed.getBoundingClientRect().top };
    // `#243 filter-keeps-selection`. While a filter change holds a message in place, that message is
    // where the reader is, and the newest line is not followed: nothing but the reader moves it.
    const held = deps.heldFilterRow();
    if (held) return { pinned: false, anchor: held, top: held.getBoundingClientRect().top };
    // `#262 search-load-older`. While a walk for the search reads older pages, its control holds still and
    // the matches open under it — never the newest line followed, on a list short enough to be at its
    // foot — until the reader scrolls it away, which lets it go. At the head of the list it may have moved
    // without the reader: the sentence above it that nothing matched goes once something does, and there is
    // no room above to keep it where it stood. Measured only while a walk runs.
    const walk = deps.searchWalk();
    if (walk && walk.anchored) {
      const chip = el("load-older");
      const top = chip.getBoundingClientRect().top;
      if (!chip.hidden && (Math.abs(top - walk.chipTop) <= 1 || area.scrollTop <= 1)) {
        walk.chipTop = top;
        return { pinned: false, anchor: chip, top };
      }
      walk.anchored = false;
    }
    const anchor = scrollAnchor(area);
    return {
      // Rest, not the slack, as `followsNewest` reads it (`#1209`'s near-foot case).
      pinned: deps.atBottom(area, deps.restSlackPx),
      anchor,
      top: anchor ? anchor.getBoundingClientRect().top : 0,
      anchorId: anchor ? anchor.getAttribute("data-id") : null,
    };
  }

  /** The next drawn row below `row` in its list: the first sibling with a box of its own. */
  function nextDrawnRow(row: Element): Element | null {
    const parent = row.parentNode;
    if (!parent) return null;
    const kids = [...parent.children];
    for (let at = kids.indexOf(row) + 1; at < kids.length; at += 1) {
      const next = kids[at] as Element;
      if (next.getAttribute("data-read-run") === "member") continue;
      const box = next.getBoundingClientRect();
      if (box.bottom > box.top) return next;
    }
    return null;
  }

  /** Hold `anchor` at `targetTop` in `area` across `mutate`, the selection kept as in `preservingScroll`. */
  function preservingScrollAt(area: HTMLElement, anchor: Element, targetTop: number, mutate: () => void): void {
    const selection = deps.selectionIn ? deps.selectionIn(area) : null;
    mutate();
    if (!anchor.parentNode) {
      deps.renderJumpNewest();
    } else {
      const shown = shownRowFor(anchor);
      const held = shown !== anchor ? shown as Element : anchor;
      area.scrollTop += held.getBoundingClientRect().top - targetTop;
      deps.renderJumpNewest();
    }
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  function toFoot(area: HTMLElement): void {
    area.scrollTop = area.scrollHeight;
  }

  function nudge(area: HTMLElement, delta: number): void {
    area.scrollTop += delta;
  }

  function toPosition(area: HTMLElement, top: number): void {
    area.scrollTop = top;
  }

  function wheelBy(area: HTMLElement, px: number): void {
    if (typeof area.scrollBy === "function") area.scrollBy({ top: px, behavior: "smooth" });
    else area.scrollTop += px;
  }

  function glideTo(area: HTMLElement, top: number, behavior: string): void {
    if (typeof area.scrollTo === "function") area.scrollTo({ top, behavior: behavior as ScrollBehavior });
    else area.scrollTop = top;
  }

  function intoView(element: Element, block: string, inline?: string): void {
    if (typeof element.scrollIntoView !== "function") return;
    if (inline === undefined) element.scrollIntoView({ block: block as ScrollLogicalPosition });
    else element.scrollIntoView({ block: block as ScrollLogicalPosition, inline: inline as ScrollLogicalPosition });
  }

  function holdingRows(mutate: () => void): void {
    if (holdDepth > 0) {
      mutate();
      return;
    }
    const mark = captureScroll();
    const area = el("scroll-area");
    const selection = deps.selectionIn ? deps.selectionIn(area) : null;
    holdDepth += 1;
    try {
      mutate();
    } finally {
      holdDepth -= 1;
    }
    if (mark.anchor && mark.anchor.parentNode) {
      const shown = shownRowFor(mark.anchor);
      const held = shown !== mark.anchor ? shown as Element : mark.anchor;
      area.scrollTop += held.getBoundingClientRect().top - mark.top;
    } else if (mark.anchorId) {
      const refound = [...visibleList().children].find((one) => one.getAttribute("data-id") === mark.anchorId);
      if (refound) area.scrollTop += (refound as Element).getBoundingClientRect().top - mark.top;
    }
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  function preservingCollapse(area: HTMLElement, row: HTMLElement, head: number, foot: number, mutate: () => void): void {
    // A live held anchor (a filter hold, a search walk, a landed jump on another row) owns
    // the reader's place over this act's own choice, as `captureScroll` has it: the act's
    // readerActed is what ends such a hold, and it has not run here.
    const jump = deps.jumpAnchorRow ? deps.jumpAnchorRow() : null;
    if (deps.heldFilterRow() || (deps.searchWalk && deps.searchWalk()) || (jump && jump !== row)) {
      preservingScroll(mutate);
      return;
    }
    if (deps.atBottom(area, deps.restSlackPx)) {
      mutate();
      area.scrollTop = area.scrollHeight;
      deps.renderJumpNewest();
      return;
    }
    const box = row.getBoundingClientRect();
    if (box.top >= head - 1) {
      preservingScrollAt(area, row, box.top, mutate);
    } else if (box.bottom <= foot + 1) {
      const next = nextDrawnRow(row);
      if (next) preservingScrollAt(area, next, next.getBoundingClientRect().top, mutate);
      else preservingScrollAt(area, row, box.top, mutate);
    } else {
      preservingScrollCentered(area, row, head, foot, mutate);
    }
  }

  /** Center `row` in `area`'s room across `mutate`, the selection kept as in `preservingScroll`. */
  function preservingScrollCentered(area: HTMLElement, row: HTMLElement, head: number, foot: number, mutate: () => void): void {
    const selection = deps.selectionIn ? deps.selectionIn(area) : null;
    mutate();
    if (!row.parentNode) {
      deps.renderJumpNewest();
    } else {
      const rect = row.getBoundingClientRect();
      area.scrollTop += rect.top + (rect.bottom - rect.top) / 2 - (head + foot) / 2;
      deps.renderJumpNewest();
    }
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  function restoreScroll(mark: ScrollMark): void {
    const area = el("scroll-area");
    if (mark.pinned) {
      deps.scrollToNewest();
      return;
    }
    // A redraw that replaced the anchor's row has taken the held node with it: the row is
    // re-found by its message id in the list as drawn now, and held by the new node. Only a
    // mark whose row is gone altogether leaves the position alone, as before.
    if (mark.anchor && !mark.anchor.parentNode && mark.anchorId) {
      const refound = [...visibleList().children].find((one) => one.getAttribute("data-id") === mark.anchorId);
      if (refound) mark = { ...mark, anchor: refound as Element };
    }
    if (!mark.anchor || !mark.anchor.parentNode) {
      deps.renderJumpNewest();
      return;
    }
    // An anchor that has just joined a collapsed run of read messages is held by the run's line,
    // where it went. `#221 read-modes`.
    const shown = shownRowFor(mark.anchor);
    if (shown !== mark.anchor) mark = { ...mark, anchor: shown as Element };
    area.scrollTop += (mark.anchor as Element).getBoundingClientRect().top - mark.top;
    // Wherever that put the reader decides the jump, now: up the history it is offered, on the
    // newest line it is not. `#207 scrollback-jump`.
    deps.renderJumpNewest();
  }

  /**
   * `pinned` is measured BEFORE the append, because appending is itself what changes the answer.
   * When it is false the scroll position is left exactly alone and the "Newest" chip appears —
   * silently leaving the reader in place would be the other half of the same defect, since they
   * would have no way of knowing something had arrived at all.
   *
   * AND ONLY WHILE THE TRANSCRIPT IS THE LIST ON SCREEN. `pinned` is measured on #scroll-area, which
   * both panes share, so a turn arriving while the reader is over on the channel was asking "is the
   * reader at the bottom of the CHANNEL?" and following the answer: it scrolled the channel and put
   * the channel's chip away, and it never raised the transcript's. A call goes on talking while you
   * read a backlog, so that is the ordinary case, not a corner — and its cost was silence. The turn
   * belongs to a list nobody is looking at, so the only right act is to raise that list's chip.
   */
  function followIfPinned(pinned: boolean, turns = 0, view = "voice"): void {
    if (pinned && deps.currentView() === view) {
      deps.scrollToNewest();
      return;
    }
    // Named explicitly: `line()` and `seam()` append to the transcript, so they leave `view` as the
    // transcript's. Letting it default to `currentView` would raise a chip over the channel list. The
    // send card (`#269 approval-card-inline`) is the one caller that names a view: it is the next
    // message of whichever list is on screen. `turns` is how many new turns this was, for the count
    // the jump shows: a turn growing as it is spoken, a seam or the card is a change below and not
    // another message. `#207 scrollback-jump`.
    deps.setJumpNewest(true, view, turns);
  }

  return { visibleList, shownChannelList, scrollAnchor, preservingScroll, preservingCollapse, holdingRows, toFoot, nudge, toPosition, wheelBy, glideTo, intoView, holdingReader, captureScroll, restoreScroll, followIfPinned };
}

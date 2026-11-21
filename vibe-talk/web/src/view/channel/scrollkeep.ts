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
  /** The held composer's viewport top as the mark was taken, where it stood in view (`#1839`). */
  readonly composerTop?: number | null;
  /** When the reader last gave input in the view, as the mark was taken (`#1880`): a restore made after later input would move a view the reader has taken over. */
  readonly inputAt?: number;
  /** The list's scroll position as the mark was taken (`#1880b`): how far the reader has scrolled since is the part of a restore that is theirs, not the change's. */
  readonly scrollTop?: number;
  /**
   * Whether the change this mark was taken for is one the reader caused (`#2076
   * vibe-talk-top-anchor-policy`): their own act on a message — a click, a fold, a
   * details sheet — whose height change holds the touched row's top and never
   * follows the foot, even at the end of the history. Arrivals are not user-caused.
   */
  readonly userCaused?: boolean;
  /** The last hold-opening act as the mark was taken (`#2076`): a restore made
   * after a later fold or press would follow the foot over the anchor that act
   * just made, as `#1880`'s input stamp guards a reader's scroll. */
  readonly actAt?: number;
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
  /**
   * The composer the reader is writing in while it holds the view (`#1839
   * vibe-talk-composer-stays-in-view-while-dictating`): its element while a message box has
   * focus or input arrived in one within the last few seconds, else null. While it stands, a
   * redraw is held on the composer, never on a message: the reader is dictating into that box,
   * and a hold that puts it off screen is the defect, whatever message it preserves. Absent:
   * unkept, as before.
   */
  composerHeld?(): Element | null;
  /**
   * Whether Read is actively following (`#1856 vibe-talk-read-aloud-follow-not-yanked-up`):
   * a message is playing and not held. While it stands, a redraw is held on nothing: the
   * follow keeps the read sentence on screen, and restoring a pre-redraw anchor (or the
   * newest line) over it is the yank the owner reported. The priority lives here, in the
   * one scroll owner, beside the composer's (`#1839`). Absent: never following.
   */
  readFollowing?(): boolean;
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
   * scroll-single-owner`, `#1215 collapse-anchor-below`, `#2076
   * vibe-talk-top-anchor-policy`): the touched row's top is held wherever it stands
   * on screen, even at the end of the history — following the foot is for arrivals,
   * never for the reader's own act; with its top above the viewport the next drawn
   * row is held and the history above absorbs the change; with neither on screen
   * (inside a row taller than the room) the collapsed row is centered.
   */
  preservingCollapse(area: HTMLElement, row: HTMLElement, head: number, foot: number, mutate: () => void): void;
  /**
   * Whether a user-caused height change is settling (`#2076`): the reader's own act
   * opened a hold (a fold, a click in the list within the act window), and until it
   * clears the foot keepers write nothing and an arrival is counted on the jump
   * chip instead of followed. The one question every foot writer outside the
   * arrival path asks.
   */
  userChangeActive(): boolean;
  /**
   * Open the user-caused hold for an act that changes a message's height outside
   * the collapse act (`#2076`): an expand-all step, a details sheet. The act's own
   * restore and the keeper pass after it keep the touched top; arrivals chip.
   */
  beginUserChange(): void;
  /**
   * The room past the rows' foot this owner is holding in `area` (`#2076`): a
   * user-caused shrink at the foot (a fold, a sheet closing) whose touched top is
   * held by standing the list in room it adds below its last row, as the right
   * column's rooms do (`#338`, `#459`). No message stands in it; the foot measures
   * count it as room, and the next deliberate write to the foot reclaims it.
   */
  footRoom(area: HTMLElement): number;
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
  /** Whether the reader has scrolled or touched since `mark` was captured (`#1880`): holders outside this owner (the filter anchor's) ask it of their own capture instead of tracking input a second time. */
  userMovedSince(mark: ScrollMark): boolean;
  /** The reader-input stamp alone, cheaply (`#1894`): a `{ inputAt }` mark for `userMovedSince` without a capture's layout reads. */
  inputStamp(): number;
  /** Put the reader back where `captureScroll` found them. */
  restoreScroll(mark: ScrollMark): void;
  /** A turn arrived: follow it only if the reader had not scrolled away. */
  followIfPinned(pinned: boolean, turns?: number, view?: string): void;
  /**
   * Which of `rows` stand at least partly in the visible message area
   * (`#1734 vibe-talk-search-view-scroll-default`): the shared scroller's box,
   * cut down to the `visualViewport` where one stands, so an on-screen
   * keyboard shrinks the area exactly as it shrinks what the reader sees.
   */
  rowsOnScreen(rows: readonly Element[]): Element[];
}

/** Keeping the reader in the page's lists. */
export function createScrollKeeping(deps: ScrollKeepingDependencies): ScrollKeeping {
  const el = deps.el;
  /** Owner holds currently being applied; a redraw inside one is held by the outer hold. */
  let holdDepth = 0;

  // --- the reader's own input (`#1880 vibe-talk-no-programmatic-scroll-after-user-scroll`) ------
  //
  // Once the reader has scrolled or touched in a view, no programmatic write may move it
  // again except their own next action. The holders below therefore remember when the
  // reader last gave input, and a capture remembers it too: a restore whose capture is
  // older than the latest input would put back a place the reader has since left, which
  // is the upward jump the owner kept meeting (a settle restore, a composer keep and an
  // anchor re-place all did it, each after a live arrival rebuilt the rows). Wheel and
  // touch are the reader unambiguously; a scroll is theirs unless it promptly follows
  // one of this owner's own writes, as the follow's pause tells its own (`#1587`).

  /** When the reader last scrolled or touched in the view, in `Date.now` terms. */
  let userInputAt = Number.NEGATIVE_INFINITY;
  /** Whether a user-caused height change's hold stands (`#2076`), and which opening it is. */
  let userHold = false;
  let holdGen = 0;
  /** When a hold-opening act last ran (`#2076`): only the acts that change a
   * message's height open one — a fold, an expand-all step, a details sheet — never
   * a navigation key or a selection press, whose lists keep their own rules. */
  let holdActAt = Number.NEGATIVE_INFINITY;
  /** The room past the foot held for a user-caused shrink (`#2076`): its height, the
   * list it stands under, and that list's own padding as it was before the room. */
  let roomPx = 0;
  let roomList: HTMLElement | null = null;
  let roomPadding = "";
  /** When this owner last wrote the list's place, in `Date.now` terms. */
  let ownWriteAt = Number.NEGATIVE_INFINITY;
  /** The port the input watch stands on; re-armed when the column swaps its area. */
  let inputPort: HTMLElement | null = null;

  /** This owner is about to write the list's place: the scroll it causes is not the reader's. */
  function noteOwnWrite(): void {
    ownWriteAt = Date.now();
  }

  /**
   * Open the user-caused hold (`#2076`): for the act's own restore and the keeper
   * pass that follows it (three frames), no foot writer moves the list and no
   * arrival is followed — the touched row's top is the place kept. Where no frames
   * run (a fake DOM), the hold ends with the task: nothing async races it there.
   */
  function openUserHold(): void {
    userHold = true;
    holdActAt = Date.now();
    const gen = ++holdGen;
    if (typeof requestAnimationFrame === "function") {
      let frames = 3;
      const step = (): void => {
        if (gen !== holdGen) return;
        frames -= 1;
        if (frames <= 0) { userHold = false; return; }
        requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    } else if (typeof queueMicrotask === "function") {
      queueMicrotask(() => { if (gen === holdGen) userHold = false; });
    } else {
      userHold = false;
    }
  }

  /** Whether a user-caused height change is settling (`#2076`): its hold stands.
   * Only acts that change a message's height open one, so a keeper or arrival
   * gated by this is never held off by a navigation key or a selection press. */
  function userChangeActive(): boolean {
    return userHold;
  }

  /** Give the list under `anchor` `px` more room past its foot (`#2076`). */
  function addFootRoom(anchor: Element, px: number): void {
    const list = anchor.parentNode as HTMLElement | null;
    if (!list || typeof list.style !== "object" || px <= 0) return;
    if (roomList !== list) {
      clearFootRoom();
      roomList = list;
      roomPadding = list.style.paddingBottom || "";
      roomPx = 0;
    }
    roomPx += px;
    list.style.paddingBottom = `${roomPx}px`;
  }

  /** Take the held room past the foot away (`#2076`): the rows' foot is the foot again. */
  function clearFootRoom(): void {
    if (roomList === null) return;
    roomList.style.paddingBottom = roomPadding;
    roomList = null;
    roomPx = 0;
    roomPadding = "";
  }

  /** The room held past the foot of the list standing in `area`, else 0 (`#2076`). */
  function footRoom(area: HTMLElement): number {
    return roomList !== null && typeof area.contains === "function" && area.contains(roomList) ? roomPx : 0;
  }

  /**
   * Write `area`'s place to hold an anchor's top (`#2076`): where holding it would
   * scroll past the foot of what stands — a user-caused shrink at the end of the
   * history — the room past the foot is grown by the deficit first, so the top
   * holds and the foot lifts, as the policy asks, instead of the browser's clamp
   * taking every row up by the shrink.
   */
  function writeHold(area: HTMLElement, anchor: Element, delta: number): void {
    const wanted = area.scrollTop + delta;
    const max = area.scrollHeight - (area.clientHeight || 0);
    if (wanted > max) addFootRoom(anchor, wanted - max);
    noteOwnWrite();
    area.scrollTop = wanted;
  }

  /** Stand the input watch on `area`, once a port. */
  function watchInput(area: HTMLElement): void {
    if (inputPort === area || typeof area.addEventListener !== "function") return;
    inputPort = area;
    area.addEventListener("wheel", () => { userInputAt = Date.now(); }, { passive: true });
    area.addEventListener("touchstart", () => { userInputAt = Date.now(); }, { passive: true });
    area.addEventListener("scroll", () => {
      // The reader's own scroll (not this owner's write) reclaims the held room
      // past the foot (`#2076`): the place is theirs again, room included.
      if (Date.now() - ownWriteAt > 1000) { userInputAt = Date.now(); clearFootRoom(); }
    }, { passive: true });
  }

  /** Whether the reader has given input since `mark` was taken. */
  function userMovedSince(mark: ScrollMark): boolean {
    return mark.inputAt !== undefined && userInputAt > mark.inputAt;
  }

  /**
   * The write that holds an anchor at the place the reader's own scroll chose (`#1880b`):
   * a change above the anchor moves its drawn top by its growth, and holding the list's
   * scroll position still would let that growth push what the reader is reading down the
   * screen by the same amount. So the write is the change's own displacement — the
   * anchor's drawn-top delta plus however far the reader has scrolled since the
   * capture — never the capture's place. With no reader scroll since, it is the
   * classic hold exactly.
   */
  function compensatedDelta(area: HTMLElement, mark: ScrollMark, topNow: number): number {
    // Only a view the reader themselves scrolled since the capture is compensated
    // (`#1880b`, scoped): deliberate flows (a filter walk, an older-page load) move
    // the list between capture and restore by their own writes, and counting those
    // as the reader's scroll over-compensates exactly the places they anchor.
    if (!userMovedSince(mark)) return topNow - mark.top;
    const userDelta = mark.scrollTop !== undefined ? area.scrollTop - mark.scrollTop : 0;
    return (topNow - mark.top) + userDelta;
  }

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
    // `#1920 vibe-talk-main-thread-switch-and-scroll-lag`: the anchor is the first drawn row
    // reaching past the edge, and drawn rows stand in list order with non-decreasing bottoms,
    // so a binary search finds it in O(log n) measured rows however long the history is. The
    // `#1913` scans this replaces walked from the head, or up from the foot, and a reader
    // parked mid-list — the ordinary case on a long channel — paid one layout read per row
    // between the foot (or the head) and the screen on every capture: about a thousand reads
    // on a 2000-row channel, inside every view switch and every held redraw. Rows with no
    // box of their own (hidden, or members of a collapsed run of read messages, `#221
    // read-modes`) are left out before the search, as `readerPlace` leaves them out, so the
    // measured bottoms never decrease.
    const drawn: Element[] = [];
    for (const li of items) {
      if (li.getAttribute("data-read-run") === "member") continue;
      if ((li as HTMLElement).hidden) continue;
      drawn.push(li);
    }
    let lo = 0;
    let hi = drawn.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if ((drawn[mid] as HTMLElement).getBoundingClientRect().bottom > edge) hi = mid;
      else lo = mid + 1;
    }
    const found = drawn[lo];
    if (found) return found;
    return items.length > 0 ? items[items.length - 1] as Element : null;
  }

  /**
   * Capture the anchor's offset, mutate, measure it again, and move the scroll position by the
   * difference. Explicit, because the alternative is trusting browser scroll anchoring, and the
   * whole point is that it does not hold for a change made above the viewport.
   *
   * A reader who was already at the bottom is the one exception: for them "the same thing" IS the
   * newest line, so the view follows it — unless the change is their own act (`#2076`): then the
   * row they touched is held by its top, like any other reader, and only an arrival follows.
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
    watchInput(area);
    // `#1224 mention-jump-in-view`. A jump's landed row is the reader's latest explicit place:
    // what changes after it holds that row first, not the row that happens to stand at the top.
    const landed = deps.jumpAnchorRow ? deps.jumpAnchorRow() : null;
    if (landed) return { pinned: false, anchor: landed, top: landed.getBoundingClientRect().top, inputAt: userInputAt, scrollTop: area.scrollTop };
    // `#243 filter-keeps-selection`. While a filter change holds a message in place, that message is
    // where the reader is, and the newest line is not followed: nothing but the reader moves it.
    const held = deps.heldFilterRow();
    if (held) return { pinned: false, anchor: held, top: held.getBoundingClientRect().top, inputAt: userInputAt, scrollTop: area.scrollTop };
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
        return { pinned: false, anchor: chip, top, inputAt: userInputAt, scrollTop: area.scrollTop };
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
      composerTop: composerBefore(area),
      inputAt: userInputAt,
      scrollTop: area.scrollTop,
      // The top-anchor rule is the channel lists' (`#2076`): the call transcript
      // keeps its own following contract for a tapped card (`#431`), so a hold
      // never reclassifies a change there.
      userCaused: userHold && deps.currentView() === "discord",
      actAt: holdActAt,
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
      writeHold(area, held, held.getBoundingClientRect().top - targetTop);
      deps.renderJumpNewest();
    }
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  function toFoot(area: HTMLElement): void {
    // A deliberate write to the foot reclaims the held room (`#2076`): the rows'
    // foot is the foot the reader is taken to.
    clearFootRoom();
    noteOwnWrite();
    area.scrollTop = area.scrollHeight;
  }

  function nudge(area: HTMLElement, delta: number): void {
    noteOwnWrite();
    area.scrollTop += delta;
  }

  function toPosition(area: HTMLElement, top: number): void {
    noteOwnWrite();
    area.scrollTop = top;
  }

  function wheelBy(area: HTMLElement, px: number): void {
    noteOwnWrite();
    if (typeof area.scrollBy === "function") area.scrollBy({ top: px, behavior: "smooth" });
    else area.scrollTop += px;
  }

  function glideTo(area: HTMLElement, top: number, behavior: string): void {
    noteOwnWrite();
    if (typeof area.scrollTo === "function") area.scrollTo({ top, behavior: behavior as ScrollBehavior });
    else area.scrollTop = top;
  }

  function intoView(element: Element, block: string, inline?: string): void {
    noteOwnWrite();
    if (typeof element.scrollIntoView !== "function") return;
    if (inline === undefined) element.scrollIntoView({ block: block as ScrollLogicalPosition });
    else element.scrollIntoView({ block: block as ScrollLogicalPosition, inline: inline as ScrollLogicalPosition });
  }

  /** The band the reader actually sees, in viewport coordinates: shrunk by the keyboard. */
  function visibleBand(area: HTMLElement): { head: number; foot: number } {
    const box = area.getBoundingClientRect();
    const visual = typeof window !== "undefined" ? window.visualViewport : null;
    const band = visual ? { head: 0, foot: visual.height } : { head: box.top, foot: box.bottom };
    // The bottom chrome (the dock bar) stands over the foot of the screen: a composer
    // kept merely inside the visual viewport can still sit under it with its Send row
    // cut off (`#1839`, liaison on the first shots). The band ends at the bar's top.
    try {
      const dock = el("dock");
      if (dock) {
        const top = dock.getBoundingClientRect().top;
        if (top > band.head && top < band.foot) band.foot = top;
      }
    } catch (_error) {
      // No dock to measure: the viewport's own foot is the band's.
    }
    return band;
  }

  /** Whether `rect` stands even partly in `band`. */
  function inBand(rect: DOMRect, band: { head: number; foot: number }): boolean {
    return rect.height > 0 && rect.bottom > band.head && rect.top < band.foot;
  }

  /**
   * Keep the held composer where it stood, with one measured write (`#1839`): while the
   * reader is dictating, the box they are writing in is the place kept, not a message - but
   * only where the box stood in view as the change began. A reader who has scrolled up
   * into the scrollback with the box still focused is reading, and is left on their
   * message as before (`#1144`): the guard never yanks them back down to the box.
   * False where no composer holds the view, or it stood out of view, so the caller
   * keeps its own hold.
   */
  function keepComposerInView(area: HTMLElement, beforeTop: number | null, userDelta = 0): boolean {
    const composer = deps.composerHeld ? deps.composerHeld() : null;
    if (!composer || beforeTop === null) return false;
    const rect = composer.getBoundingClientRect();
    if (!rect || rect.height <= 0) return true;
    // `#1880b`: the place kept is where the reader's own scroll since the capture
    // left the box, not where it stood at the capture.
    noteOwnWrite();
    area.scrollTop += rect.top - (beforeTop - userDelta);
    // The restore can still leave the box past an edge (the list grew at the foot):
    // bring it just inside, never through the history between.
    const band = visibleBand(area);
    const now = composer.getBoundingClientRect();
    if (now.bottom > band.foot) { noteOwnWrite(); area.scrollTop += now.bottom - band.foot; }
    else if (now.top < band.head) { noteOwnWrite(); area.scrollTop += now.top - band.head; }
    return true;
  }

  /** The held composer's top as a change begins, where it stood in view; else null. */
  function composerBefore(area: HTMLElement): number | null {
    const composer = deps.composerHeld ? deps.composerHeld() : null;
    if (!composer) return null;
    const rect = composer.getBoundingClientRect();
    return rect && inBand(rect, visibleBand(area)) ? rect.top : null;
  }

  function holdingRows(mutate: () => void): void {
    if (holdDepth > 0) {
      mutate();
      return;
    }
    const mark = captureScroll();
    const area = el("scroll-area");
    const composerWas = composerBefore(area);
    const selection = deps.selectionIn ? deps.selectionIn(area) : null;
    holdDepth += 1;
    try {
      mutate();
    } finally {
      holdDepth -= 1;
    }
    if (keepComposerInView(area, composerWas, userMovedSince(mark) && mark.scrollTop !== undefined ? area.scrollTop - mark.scrollTop : 0)) {
      if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
      return;
    }
    // `#1856`: while Read is following, the redraw is held on nothing: the follow will
    // put the reader back on the sentence being read, and any anchor restore here is a
    // second writer moving the list the other way.
    if (deps.readFollowing && deps.readFollowing()) {
      if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
      return;
    }
    if (mark.anchor && mark.anchor.parentNode) {
      const shown = shownRowFor(mark.anchor);
      const held = shown !== mark.anchor ? shown as Element : mark.anchor;
      writeHold(area, held, compensatedDelta(area, mark, held.getBoundingClientRect().top));
    } else if (mark.anchorId) {
      const refound = [...visibleList().children].find((one) => one.getAttribute("data-id") === mark.anchorId);
      if (refound) writeHold(area, refound as Element, compensatedDelta(area, mark, (refound as Element).getBoundingClientRect().top));
    }
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  function preservingCollapse(area: HTMLElement, row: HTMLElement, head: number, foot: number, mutate: () => void): void {
    // The reader's own act in a channel list (`#2076`): its hold stands over the
    // keeper pass that follows, so nothing takes the list to its foot while the
    // fold settles. The call transcript is not a channel list: a tapped card
    // there keeps the foot it was following (`#431 voice-transcript-expand`).
    if (deps.currentView() === "discord") openUserHold();
    // A live held anchor (a filter hold, a search walk, a landed jump on another row) owns
    // the reader's place over this act's own choice, as `captureScroll` has it: the act's
    // readerActed is what ends such a hold, and it has not run here.
    const jump = deps.jumpAnchorRow ? deps.jumpAnchorRow() : null;
    if (deps.heldFilterRow() || (deps.searchWalk && deps.searchWalk()) || (jump && jump !== row)) {
      preservingScroll(mutate);
      return;
    }
    if (deps.currentView() !== "discord" && deps.atBottom(area, deps.restSlackPx)) {
      mutate();
      noteOwnWrite();
      area.scrollTop = area.scrollHeight;
      deps.renderJumpNewest();
      return;
    }
    // No foot branch in a channel list (`#2076 vibe-talk-top-anchor-policy`): at
    // the end of the history the touched row is held by its top like anywhere
    // else, and what is below it moves — the composer lifting off the bottom
    // included. Following the foot is the arrival path's (`followIfPinned`).
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
      noteOwnWrite();
      area.scrollTop += rect.top + (rect.bottom - rect.top) / 2 - (head + foot) / 2;
      deps.renderJumpNewest();
    }
    if (selection && deps.restoreSelectionIn) deps.restoreSelectionIn(area, selection);
  }

  function restoreScroll(mark: ScrollMark): void {
    const area = el("scroll-area");
    watchInput(area);
    // `#1839`: while the composer holds the view and stood in view as the change began,
    // it outranks both the newest line and the message anchor - putting either back can
    // carry the box being dictated into off screen. `mark.composerTop` is its place then.
    if (keepComposerInView(area, typeof mark.composerTop === "number" ? mark.composerTop : null, userMovedSince(mark) && mark.scrollTop !== undefined ? area.scrollTop - mark.scrollTop : 0)) return;
    // `#1880`: the reader has scrolled or touched since this mark was taken (a live
    // read landed while they were up the thread) and no composer keep answered for
    // the view: the newest line is not followed, and with no anchor there is
    // nothing to compensate. With an anchor, the write below compensates only the
    // change's own displacement (`#1880b`), never the capture's place.
    if (userMovedSince(mark) && (mark.pinned || !mark.anchor)) {
      deps.renderJumpNewest();
      return;
    }
    // `#1856`: while Read is following, no restore at all — neither the newest line nor
    // the captured anchor may be put back over the sentence being read.
    if (deps.readFollowing && deps.readFollowing()) return;
    // `#2076`: a user-caused change never follows the foot, even at the end of the
    // history: the anchor below is held by its top and everything below it moves.
    // Nor does a restore whose capture predates the reader's act (a read sent
    // before a fold, landing after it): following the foot there would undo the
    // anchor the act just made, so it is held like the act's own change.
    const actedSince = mark.actAt !== undefined && holdActAt > mark.actAt;
    if (mark.pinned && !mark.userCaused && !actedSince) {
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
    writeHold(area, mark.anchor as Element, compensatedDelta(area, mark, (mark.anchor as Element).getBoundingClientRect().top));
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
    // `#2076`: while a user-caused change is settling, an arrival never undoes the
    // touched row's anchor: it is counted on the jump chip like any arrival the
    // reader is not following, with no retroactive follow when the hold clears —
    // only a later arrival whose own capture finds the reader at the foot sticks.
    if (pinned && deps.currentView() === view && !(view === "discord" && userChangeActive())) {
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

  function rowsOnScreen(rows: readonly Element[]): Element[] {
    if (rows.length === 0) return [];
    const area = el("scroll-area");
    const box = area.getBoundingClientRect();
    let head = box.top;
    let foot = box.bottom;
    const visual = typeof window !== "undefined" ? window.visualViewport : null;
    if (visual) {
      // Rects are read in the visual viewport's own coordinates, whose
      // visible band is [0, height]; the keyboard's shrink shows up here.
      head = Math.max(head, 0);
      foot = Math.min(foot, visual.height);
    }
    if (foot <= head) return [];
    // `#1913`: the rows handed in stand displayed (the caller leaves out hidden ones, whose
    // boxes are all zero), in list order, so their bottoms do not decrease: a binary search
    // finds the first row reaching into the band, and only the rows on screen around it are
    // measured — noting the screen for a keystroke never measures a long history.
    let lo = 0;
    let hi = rows.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if ((rows[mid] as Element).getBoundingClientRect().bottom > head) hi = mid;
      else lo = mid + 1;
    }
    const onScreen: Element[] = [];
    for (let at = lo; at < rows.length; at += 1) {
      const rect = (rows[at] as Element).getBoundingClientRect();
      // Past the foot, nothing further down can come back onto the screen.
      if (rect.top >= foot && rect.bottom > rect.top) break;
      if (rect.bottom > rect.top && rect.bottom > head && rect.top < foot) onScreen.push(rows[at] as Element);
    }
    return onScreen;
  }

  return { visibleList, shownChannelList, scrollAnchor, preservingScroll, preservingCollapse, holdingRows, toFoot, nudge, toPosition, wheelBy, glideTo, intoView, holdingReader, captureScroll, restoreScroll, followIfPinned, rowsOnScreen, userMovedSince, inputStamp: () => userInputAt, userChangeActive, beginUserChange: openUserHold, footRoom };
}

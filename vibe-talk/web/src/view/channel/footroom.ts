// The room past the foot of the right column (`#338 column-overscroll`, `#459 pack-keeps-card-still`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): the room a reader going on down
// from the foot opens, fitted so the last card's top can come up to the middle of the column, and the room
// held for a card packed where it stands; how far past the foot each lets the column scroll, which every
// measure of the foot goes through; fitting each again as the rows change; keeping a follower and a reader in
// the room where they are; and the wheel and scroll events that open and close them. What the column shows,
// whether it is on screen, its last card and its page step are the page's; why the rooms open and close as
// they do is told where the legacy script opens its two sections. What the column shows, whether it is up,
// and its last card are read here from the page acts handed in.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long the wheel rests before a turn of it at the foot opens the room: longer than between its turns. */
const OVERSCROLL_REST_MS = 150;

/** How long the room stays open with the reader resting at the foot, not in it, before it closes. */
const OVERSCROLL_IDLE_MS = 700;

/** A scroller, as far as the rooms measure and move it. */
type Scroller = HTMLElement;

/** A turn of the wheel, as far as the rooms read it. */
interface Wheel {
  readonly timeStamp: number;
  readonly deltaY: number;
  readonly deltaMode: number;
  readonly ctrlKey: boolean;
}

/** What the rooms ask of the page. */
export interface FootRoomsDependencies {
  readonly clock: ClockPort;
  /** The right column's scroller (`side-scroll`). */
  side(): Scroller;
  /** The room past its foot that the reader opens (`side-overscroll`). */
  overscrollBox(): HTMLElement;
  /** The room past its foot held for a card (`side-overextend`). */
  overextendBox(): HTMLElement;
  /** The page's element by id, in the column being worked on and in the left one. */
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  /** Whether two columns are shown, and whether the thread list is (`layoutShown`). */
  layoutColumns(): boolean;
  layoutThreadList(): boolean;
  /** Which screen and view are up (`screens`). */
  screens(): { screen(): string; view(): string };
  /** What the right column shows (`columns.side.mode`). */
  sideMode(): string;
  /** The read mode over the right column's rows (`readModes.mode`). */
  readMode(): string;
  /**
   * The right column's own context, read in it: its channel context key, whether only
   * pinned rows show, and its search key.
   */
  sideContext(): readonly unknown[];
  /** Run `act` in the right column. */
  inSideColumn<T>(act: () => T): T;
  /** The list a channel's rows are shown in (`shownChannelList`). */
  shownChannelList(): HTMLElement;
  /** Whether the right column is on screen (`sideColumnUp`). */
  sideUp(): boolean;
  /** Put a scroller at a measured position, through the scroll owner (`#1251`). */
  toPosition(area: Scroller, top: number): void;
  /** End a pack slide still running (`finishPackSlide`). */
  finishPackSlide(): void;
  /** The card drawn for thread `id`, or null (`cardRow`). */
  cardRow(id: string): HTMLElement | null;
  /** Whether `row` is shown (`rowShown`). */
  rowShown(row: HTMLElement): boolean;
  /** The item after `row`'s block (`itemAfterBlock`). */
  itemAfterBlock(row: HTMLElement): HTMLElement | null;
  /** What an item is across a redraw (`slideKey`). */
  slideKey(item: HTMLElement): string;
  /** Slide what is below `row` into place from where `was` stood (`slideIntoPlace`). */
  slideIntoPlace(row: HTMLElement, was: { key: string; top: number }): void;
  /** The scroller's padding under its rows, in px. */
  paddingBottom(area: Scroller): number;
  /** What a page key moves `area` by (`pageStep`). */
  pageStep(area: Scroller): number;
  /**
   * The room in `area`'s height that is the on-screen keyboard's, not the rows' (`#1144
   * scrollback-typing-jump`): the inset a composer at the list's foot keeps under itself while the
   * keyboard is up, which `scrollHeight` counts but no message stands in. 0 where no keyboard is up
   * over a composer in `area`.
   */
  keyboardRoom?(area: Scroller): number;
  /** How far above the foot still counts as parked on the newest line (`BOTTOM_SLACK_PX`). */
  readonly bottomSlack: number;
  /** How far above the foot rest still counts as following it (`FOLLOW_REST_PX`). */
  readonly followRestPx: number;
  /** The page's element by id, in the column being worked on. */
  /** The filter change's held row record, whose `hold` a deliberate go to the newest lets go. */
  filterState(): { hold: unknown };
  /** Say whether the jump to the newest line is wanted (`setJumpNewest`). */
  setJumpNewest(wanted: boolean): void;
  /** Main's follow state as its keeper notes it, or null before the keepers mount. */
  mainFollowState(): (() => boolean) | null;
  /** Whether the reader of `area` follows its newest line, re-derived (`followsNewest`). */
  followsNewest(area: Scroller): boolean;
  /** The row a filter change holds in place, or null (`heldFilterRow`). */
  heldFilterRow(): unknown;
  /** Whether a quick reply holds `area` (`quickReplyHolds`). */
  quickReplyHolds(area: Scroller): boolean;
  /** Take `area` to its foot, and move it by a measured delta, through the scroll owner (`#1251`). */
  toFoot(area: Scroller): void;
  nudge(area: Scroller, delta: number): void;
  wheelBy(area: Scroller, px: number): void;
}

/** The rooms' commands and questions. */
export interface FootRooms {
  /** How far `area` scrolls past the foot of its rows in the reader's room, else 0; a stale room is closed first. */
  overscrollRoom(area: Scroller): number;
  /** How far `area` scrolls past the foot for a card held where it stood, else 0; a stale room is closed first. */
  overextendRoom(area: Scroller): number;
  /** How far the rows of `area` run below what it shows: 0 at the foot, below 0 in a room past it. */
  footGap(area: Scroller): number;
  /** The scroll position of the foot of `area`. */
  footTop(area: Scroller): number;
  /** Close the reader's room. */
  closeOverscroll(): void;
  /** Close the reader's room once the reader has rested at the foot without going into it. */
  waitOverscrollIdle(): void;
  /** Open the reader's room for the reader of `area` going on down from its foot. Whether it is open. */
  openOverscroll(area: Scroller): boolean;
  /** Fit the reader's room to the last card again, after the rows or the column changed size. */
  fitOverscroll(area: Scroller): void;
  /** Note where the last card stands while the reader of `area` is in the room, for `holdOverscrollReader`. */
  noteOverscrollReader(area: Scroller): void;
  /** The last card changing size under a reader in the room: fitted to at once, and the card left where it was. */
  holdOverscrollReader(area: Scroller): void;
  /** Keep the reader of `area` on its newest line, a reader in a room staying where they are while it fits. */
  followFoot(area: Scroller): void;
  /** Take the reader of `area` to its foot, out of any room past it. */
  toFoot(area: Scroller): void;
  /** Close the room held for a card. */
  closeOverextend(): void;
  /** Give `area` the room past its foot that lets it scroll to `top`, and no more (`fit`: never taller). */
  sizeOverextend(area: Scroller, top: number, fit?: boolean): void;
  /** Fit the room held for a card to where the reader of `area` now stands. */
  fitOverextend(area: Scroller): void;
  /** A turn of the wheel over the right column: at a rested foot, the room opens and takes the turn. */
  wheel(event: Wheel): void;
  /** The right column scrolled (`timeStamp` the event's, or -Infinity with none). */
  scrolled(timeStamp: number): void;
  /** Whether `area` is at its newest line, within `slack` where one is named. */
  atBottom(area: Scroller, slack?: number): boolean;
  /** Go to the newest line — deliberately, because somebody asked. */
  scrollToNewest(): void;
  /**
   * Whether the chips' and the strip's keepers may take Main's list to its foot as they
   * resize: the follow state while the keepers can say it, with a filter's or a quick
   * reply's hold still vetoing; the re-derived test only before they mount.
   */
  chipsFollowFoot(area: Scroller, grew?: number): boolean;
  /**
   * Run `change` to the cards with thread `id`'s card held exactly where it is on the screen, past the foot
   * of the list where that is what it takes. A card wholly above the screen — ^ pressed deep in its
   * unpacked block — is held at the top of it instead, so what is left is never a screen of blank room. Whether the
   * card was held: not where it was not drawn before, or is not after.
   */
  keepingCardStill(id: string, change: () => void, slide?: boolean): boolean;
}

/** Both rooms closed. */
export function createFootRooms(deps: FootRoomsDependencies): FootRooms {
  /**
   * The room open past the foot of the right column: its height in px, what the column showed as it was
   * opened (`context`), the wait to close it once the reader rests at the foot, and where the last card stood
   * while the reader was last seen in the room (`noteOverscrollReader`). Null while closed.
   */
  let overscroll: {
    px: number; context: string; idle: TimerHandle | null;
    reader: { row: Element; top: number; height: number } | null;
  } | null = null;
  /** When the wheel last turned over the right column (`Event.timeStamp`), for `OVERSCROLL_REST_MS`. */
  let overscrollWheelAt = -Infinity;
  /**
   * Since when the right column's list has been at its foot, by its scroll events (`Event.timeStamp`), or null
   * while it is not. The browser scrolls for a turn of the wheel before the page hears of the turn, so a list
   * found at its foot may have been brought there by that very turn: only one that has rested there as long
   * as the wheel rests (`OVERSCROLL_REST_MS`) was there before it.
   */
  let overscrollFootSince: number | null = null;
  /**
   * The room open past the foot for a card held where it stood: its height in px and what the column showed
   * as it was opened. Null while closed.
   */
  let overextend: { px: number; context: string } | null = null;

  function overscrollRoom(area: Scroller): number {
    if (overscroll === null || area !== deps.side()) return 0;
    if (overscroll.context !== overscrollContext() || !overscrollOffered()) {
      closeOverscroll();
      return 0;
    }
    return overscroll.px;
  }

  /**
   * Every room past the rows' foot that `scrollHeight` counts (`#1144 scrollback-typing-jump`):
   * the right column's two, and the keyboard's room under a composer at the foot. The foot every
   * measure below is of is the rows', so a reader resting on the composer with the keyboard up is
   * at it, and one scrolled up from it is that far from it, whatever the keyboard does.
   */
  function roomsPastFoot(area: Scroller): number {
    return overscrollRoom(area) + overextendRoom(area) + (deps.keyboardRoom ? deps.keyboardRoom(area) || 0 : 0);
  }

  function footGap(area: Scroller): number {
    // The room first: asking for it can close it (`overscrollRoom`), which takes its height out of
    // the list and clamps where the reader stands — heights read before it would count the room
    // twice, and a reader in it would measure a room's height short of the foot they are past.
    const room = roomsPastFoot(area);
    return area.scrollHeight - room - area.scrollTop - (area.clientHeight || 0);
  }

  function footTop(area: Scroller): number {
    const room = roomsPastFoot(area);
    return Math.max(0, area.scrollHeight - room - (area.clientHeight || 0));
  }

  function closeOverscroll(): void {
    if (overscroll === null) return;
    if (overscroll.idle !== null) deps.clock.clearTimeout(overscroll.idle);
    overscroll = null;
    deps.overscrollBox().style.height = "";
  }

  /**
   * The room `area` needs past its foot for the top of its last card to come up to half its height: as
   * much as half of it, less how far above the end of the rows (the room's own top) that card's top is,
   * and the scroller's padding under them. 0 where the card is already at the middle or above at the foot.
   */
  function overscrollFit(area: Scroller): number {
    const last = lastColumnCard();
    if (!last) return 0;
    const pad = deps.paddingBottom(area);
    const above = deps.overscrollBox().getBoundingClientRect().top - last.getBoundingClientRect().top;
    return Math.max(0, Math.round((area.clientHeight || 0) / 2 - above - pad));
  }

  function waitOverscrollIdle(): void {
    if (overscroll === null) return;
    if (overscroll.idle !== null) deps.clock.clearTimeout(overscroll.idle);
    overscroll.idle = deps.clock.setTimeout(() => {
      if (overscroll === null) return;
      overscroll.idle = null;
      const area = deps.side();
      if (overscrollRoom(area) > 0 && area.scrollTop <= footTop(area) + 1) closeOverscroll();
    }, OVERSCROLL_IDLE_MS);
  }

  /**
   * Give the open room `px`, or close it where even that leaves a list too short to scroll past its foot.
   * Whether it is still open.
   */
  function sizeOverscroll(area: Scroller, px: number): boolean {
    if (overscroll === null) return false;
    overscroll.px = px;
    const room = deps.overscrollBox();
    if (room.style.height !== `${px}px`) room.style.height = `${px}px`;
    if (area.scrollHeight - (area.clientHeight || 0) > footTop(area) + 1) return true;
    closeOverscroll();
    return false;
  }

  function openOverscroll(area: Scroller): boolean {
    if (area !== deps.side()) return false;
    if (overscrollRoom(area) > 0) {
      waitOverscrollIdle();
      return true;
    }
    // `#459 pack-keeps-card-still`. Not over the room a card held where it stood left: the reader is past
    // the foot already, and that room only ever gives way.
    if (overextendRoom(area) > 0) return false;
    if (!overscrollOffered() || footGap(area) > 1) return false;
    const px = overscrollFit(area);
    if (px < 1) return false;
    overscroll = { px, context: overscrollContext(), idle: null, reader: null };
    if (!sizeOverscroll(area, px)) return false;
    waitOverscrollIdle();
    return true;
  }

  function fitOverscroll(area: Scroller): void {
    if (overscrollRoom(area) === 0 || overscroll === null) return;
    const px = overscrollFit(area);
    if (px < 1 || area.scrollTop < footTop(area) - 1) {
      closeOverscroll();
      return;
    }
    sizeOverscroll(area, px);
  }

  function noteOverscrollReader(area: Scroller): void {
    if (overscroll === null || area !== deps.side()) return;
    const row = area.scrollTop > footTop(area) + 0.5 ? lastColumnCard() : null;
    if (overscroll === null) return;
    const box = row ? row.getBoundingClientRect() : null;
    overscroll.reader = row && box
      ? { row, top: box.top - area.getBoundingClientRect().top, height: box.height }
      : null;
  }

  function holdOverscrollReader(area: Scroller): void {
    if (overscroll === null || overscrollRoom(area) === 0) return;
    const held = overscroll.reader;
    fitOverscroll(area);
    if (overscroll === null || held === null) return;
    if (held.row === lastColumnCard()) {
      const box = held.row.getBoundingClientRect();
      const moved = box.top - area.getBoundingClientRect().top - held.top;
      if (Math.abs(box.height - held.height) > 0.5 && Math.abs(moved) > 0.5) deps.nudge(area, moved);
    }
    noteOverscrollReader(area);
  }

  function followFoot(area: Scroller): void {
    // `#459 pack-keeps-card-still`. A reader in the room a card held where it stood left past the foot stays
    // exactly where they are while what came fits in it; what does not takes them on to the foot.
    if (overextendRoom(area) > 0) {
      fitOverextend(area);
      if (overextend !== null) return;
    }
    if (overscrollRoom(area) > 0) {
      fitOverscroll(area);
      if (overscroll !== null && area.scrollTop >= footTop(area) - 0.5) return;
      closeOverscroll();
    }
    deps.toFoot(area);
  }

  function toFoot(area: Scroller): void {
    // `#459 pack-keeps-card-still`. Out of the room a card held where it stood left, too.
    if (area === deps.side()) closeOverextend();
    if (area === deps.side()) closeOverscroll();
    deps.toFoot(area);
  }

  function overextendRoom(area: Scroller): number {
    if (overextend === null || area !== deps.side()) return 0;
    if (overextend.context !== overscrollContext() || !deps.sideUp()) {
      closeOverextend();
      return 0;
    }
    return overextend.px;
  }

  function closeOverextend(): void {
    if (overextend === null) return;
    overextend = null;
    deps.overextendBox().style.height = "";
  }

  /**
   * Give `area` the room past its foot that lets it scroll to `top`, and no more: closed where it already
   * scrolls that far without it. Fitting it again (`fit`) never makes it taller, and closes it within a pixel
   * of the foot: the end of the rows is fractional and the browser rounds the scroll range, so a room fitted
   * up to wherever the reader stands could leave them a pixel more to scroll each time, and grow under them.
   * How far it scrolls without it is measured from where the room starts, the end of the rows as laid out,
   * rather than from `scrollHeight`, which counts the rows sliding into a gap (`#460 pack-gap-slide`) where
   * they are drawn: fitted by that, the room would be short by the slide, and the browser's clamp would take
   * the reader up as the slide ended.
   */
  function sizeOverextend(area: Scroller, top: number, fit = false): void {
    overextendRoom(area);
    const pad = deps.paddingBottom(area);
    const end = area.scrollTop + deps.overextendBox().getBoundingClientRect().top -
      area.getBoundingClientRect().top - (area.clientTop || 0);
    const furthest = end + pad - (area.clientHeight || 0);
    const want = top - furthest;
    const px = fit && overextend !== null ? Math.min(overextend.px, Math.ceil(want)) : Math.ceil(want);
    if (px < 1 || (fit && want < 1)) {
      closeOverextend();
      return;
    }
    if (overextend === null) overextend = { px, context: overscrollContext() };
    overextend.px = px;
    const room = deps.overextendBox();
    if (room.style.height !== `${px}px`) room.style.height = `${px}px`;
  }

  function fitOverextend(area: Scroller): void {
    if (overextendRoom(area) > 0) sizeOverextend(area, area.scrollTop, true);
  }

  /**
   * What the right column shows, for the room opened over it: the room closes on anything else. The search
   * and the filters over its rows too (`searchKey`): a search typed in the room hides cards under the reader,
   * and lands them on the foot of what it leaves rather than in blank room fitted to another card.
   */
  function overscrollContext(): string {
    return JSON.stringify([deps.layoutColumns(), deps.layoutThreadList(), deps.screens().screen(), deps.screens().view(), deps.sideMode(),
      deps.readMode(), deps.sideContext()]);
  }

  /** Whether the room may be open: the right column on screen at a desk, where web/voice.css gives it a box. */
  function overscrollOffered(): boolean {
    const room = deps.overscrollBox();
    return deps.screens().screen() === "main" && deps.screens().view() === "discord" && deps.sideUp() &&
      typeof room.getClientRects === "function" && room.getClientRects().length > 0;
  }

  /**
   * The last card of the right column that is drawn: its last thread card over the cards, else the last row
   * of its thread, a reply still in the outbox under it included. Null where none is drawn.
   */
  function lastColumnCard(): Element | null {
    const lists = deps.sideMode() === "thread"
      ? deps.inSideColumn(() => [deps.el("outgoing-log"), deps.shownChannelList()])
      : [deps.leftEl("side-cards")];
    for (const list of lists) {
      const rows = [...list.children];
      for (let at = rows.length - 1; at >= 0; at -= 1) {
        const row = rows[at]!;
        // `#419 threads-view-main-messages`: over the cards, the last card is a card, not a row among them.
        if (row.hasAttribute("data-unpack-thread") || row.hasAttribute("data-main-row")) continue;
        if (row.getBoundingClientRect().height > 0) return row;
      }
    }
    return null;
  }

  /**
   * Run `change` to the cards with thread `id`'s card held exactly where it is on the screen, past the foot
   * of the list where that is what it takes. A card wholly above the screen — ^ pressed deep in its
   * unpacked block — is held at the top of it instead, so what is left is never a screen of blank room. Whether the
   * card was held: not where it was not drawn before, or is not after.
   */
  function keepingCardStill(id: string, change: () => void, slide = false): boolean {
    const area = deps.side();
    // A slide still running is ended first: everything is measured where it is laid out.
    deps.finishPackSlide();
    const row = deps.cardRow(id);
    const box = row && deps.rowShown(row) ? row.getBoundingClientRect() : null;
    const stage = area.getBoundingClientRect().top;
    const before = box ? (box.bottom <= stage ? stage : box.top) : null;
    const next = slide && row ? deps.itemAfterBlock(row) : null;
    const was = next ? { key: deps.slideKey(next), top: next.getBoundingClientRect().top } : null;
    change();
    const now = deps.cardRow(id);
    if (before === null || !now || !deps.rowShown(now)) return false;
    const top = area.scrollTop + now.getBoundingClientRect().top - before;
    // The wheel's room past the foot is fitted to the last card and closed above the foot on its own
    // schedule, either of which would move the card; this room stands in for it.
    closeOverscroll();
    sizeOverextend(area, top);
    deps.toPosition(area, top);
    if (was) deps.slideIntoPlace(now, was);
    return true;
  }

  /**
   * Whether the chips' and the strip's keepers may take Main's list to its foot as they resize:
   * the follow state while the keepers can say it, with a filter's or a quick reply's hold still
   * vetoing; the re-derived test (`followsNewest`) only before they mount. A reader resting a few
   * pixels above the foot, inside the old slack, is not following, and is left where they are.
   */
  function chipsFollowFoot(area: Scroller, grew = 0): boolean {
    const follow = deps.mainFollowState();
    if (!follow) return deps.followsNewest(area);
    if (deps.heldFilterRow() !== null || deps.quickReplyHolds(area)) return false;
    // The keeper's state, or the reader standing at the foot within its rest room where the state
    // has not heard their landing (a screen coming back, a list whose scrolls were another's):
    // past that room a reader resting up the list is left where they are (`#1185`). Either way the
    // reader must stand within the slack plus THIS observer's own growth of the foot: content that
    // grew elsewhere (a row at the foot laid out taller) strands a follower where they stand, and
    // the jump to it is offered, not taken (`#207`).
    if (!atBottom(area, deps.bottomSlack + grew)) return false;
    return follow() || atBottom(area, deps.followRestPx);
  }

  /**
   * Go to the newest line — deliberately, because somebody asked.
   *
   * This used to run on EVERY arrival, which is the defect `#47 scrollback-stability` is about: the
   * reader scrolls up to find what the assistant said two minutes ago, a turn lands, and the page
   * throws them back to the bottom. Every remaining caller is a deliberate act — entering a view,
   * loading a channel for the first time, tapping "Newest" — or an arrival that happened while the
   * reader was already parked at the bottom, which is `followIfPinned` below.
   */
  function scrollToNewest(): void {
    const area = deps.el("scroll-area");
    // A message a filter change held in place is let go: the reader is going somewhere else. `#243`.
    deps.filterState().hold = null;
    // `#338 column-overscroll`: a reader in the room past the right column's foot is on that line already.
    followFoot(area);
    deps.setJumpNewest(false);
  }

  /**
   * Was the reader already parked on the newest message?
   *
   * A browser clamps scrollTop to scrollHeight - clientHeight, so "at the bottom" is never
   * scrollTop === scrollHeight however hard `scrollToNewest` pushes. The slack absorbs that, plus
   * sub-pixel layout and a thumb that stopped a few pixels short — at which point the reader still
   * means "I am at the bottom", and the page should still follow.
   *
   * `#338 column-overscroll`. Measured from the foot of the rows, so a reader anywhere in the room open
   * past the right column's foot is parked on the newest message too (`footGap`).
   */
  function atBottom(area: Scroller, slack: number = deps.bottomSlack): boolean {
    return footGap(area) <= slack;
  }

  return {
    overscrollRoom,
    overextendRoom,
    footGap,
    footTop,
    closeOverscroll,
    waitOverscrollIdle,
    openOverscroll,
    fitOverscroll,
    noteOverscrollReader,
    holdOverscrollReader,
    followFoot,
    toFoot,
    closeOverextend,
    sizeOverextend,
    fitOverextend,
    keepingCardStill,
    atBottom,
    scrollToNewest,
    chipsFollowFoot,
    wheel(event) {
      const rested = event.timeStamp - overscrollWheelAt >= OVERSCROLL_REST_MS;
      overscrollWheelAt = event.timeStamp;
      if (!rested || event.deltaY <= 0 || event.ctrlKey || overscroll !== null) return;
      const area = deps.side();
      // A list too short to scroll has no scroll events, and rests at its foot as it rests at its head.
      const resting = overscrollFootSince !== null
        ? event.timeStamp - overscrollFootSince >= OVERSCROLL_REST_MS
        : area.scrollHeight - (area.clientHeight || 0) <= 1;
      if (!resting || !openOverscroll(area)) return;
      // A line is 40px, as Chromium scrolls one; a page is what a page key moves (`pageStep`).
      const px = event.deltaMode === 1 ? event.deltaY * 40 : event.deltaMode === 2 ? event.deltaY * deps.pageStep(area) : event.deltaY;
      deps.wheelBy(area, px);
    },
    scrolled(at) {
      const area = deps.side();
      // `#459 pack-keeps-card-still`. The room held for a card goes as the reader goes up out of it.
      fitOverextend(area);
      if (footGap(area) > 1) overscrollFootSince = null;
      else if (overscrollFootSince === null) overscrollFootSince = at;
      if (overscroll === null || overscrollRoom(area) === 0) return;
      if (area.scrollTop < footTop(area) - 1) {
        closeOverscroll();
        return;
      }
      waitOverscrollIdle();
      noteOverscrollReader(area);
    },
  };
}

// What the page and the tests use of it.
export { OVERSCROLL_IDLE_MS, OVERSCROLL_REST_MS };

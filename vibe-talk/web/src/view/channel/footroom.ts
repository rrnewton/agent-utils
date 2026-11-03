// The room past the foot of the right column (`#338 column-overscroll`, `#459 pack-keeps-card-still`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): the room a reader going on down
// from the foot opens, fitted so the last card's top can come up to the middle of the column, and the room
// held for a card packed where it stands; how far past the foot each lets the column scroll, which every
// measure of the foot goes through; fitting each again as the rows change; keeping a follower and a reader in
// the room where they are; and the wheel and scroll events that open and close them. What the column shows,
// whether it is on screen, its last card and its page step are the page's; why the rooms open and close as
// they do is told where the legacy script opens its two sections.

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
  /** What the right column shows, for a room opened over it: the room closes on anything else. */
  context(): string;
  /** Whether the reader's room may be open: the right column on screen at a desk, where it has a box. */
  offered(): boolean;
  /** Whether the right column is on screen (`sideColumnUp`). */
  sideUp(): boolean;
  /** The last card of the right column that is drawn, or null. */
  lastCard(): Element | null;
  /** The scroller's padding under its rows, in px. */
  paddingBottom(area: Scroller): number;
  /** What a page key moves `area` by (`pageStep`). */
  pageStep(area: Scroller): number;
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
    if (overscroll.context !== deps.context() || !deps.offered()) {
      closeOverscroll();
      return 0;
    }
    return overscroll.px;
  }

  function footGap(area: Scroller): number {
    // The room first: asking for it can close it (`overscrollRoom`), which takes its height out of
    // the list and clamps where the reader stands — heights read before it would count the room
    // twice, and a reader in it would measure a room's height short of the foot they are past.
    const room = overscrollRoom(area) + overextendRoom(area);
    return area.scrollHeight - room - area.scrollTop - (area.clientHeight || 0);
  }

  function footTop(area: Scroller): number {
    const room = overscrollRoom(area) + overextendRoom(area);
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
    const last = deps.lastCard();
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
    if (!deps.offered() || footGap(area) > 1) return false;
    const px = overscrollFit(area);
    if (px < 1) return false;
    overscroll = { px, context: deps.context(), idle: null, reader: null };
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
    const row = area.scrollTop > footTop(area) + 0.5 ? deps.lastCard() : null;
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
    if (held.row === deps.lastCard()) {
      const box = held.row.getBoundingClientRect();
      const moved = box.top - area.getBoundingClientRect().top - held.top;
      if (Math.abs(box.height - held.height) > 0.5 && Math.abs(moved) > 0.5) area.scrollTop += moved;
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
    area.scrollTop = area.scrollHeight;
  }

  function toFoot(area: Scroller): void {
    // `#459 pack-keeps-card-still`. Out of the room a card held where it stood left, too.
    if (area === deps.side()) closeOverextend();
    if (area === deps.side()) closeOverscroll();
    area.scrollTop = area.scrollHeight;
  }

  function overextendRoom(area: Scroller): number {
    if (overextend === null || area !== deps.side()) return 0;
    if (overextend.context !== deps.context() || !deps.sideUp()) {
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
    if (overextend === null) overextend = { px, context: deps.context() };
    overextend.px = px;
    const room = deps.overextendBox();
    if (room.style.height !== `${px}px`) room.style.height = `${px}px`;
  }

  function fitOverextend(area: Scroller): void {
    if (overextendRoom(area) > 0) sizeOverextend(area, area.scrollTop, true);
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
      if (typeof area.scrollBy === "function") area.scrollBy({ top: px, behavior: "smooth" });
      else area.scrollTop += px;
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

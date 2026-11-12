// The jump's anchor (`#1224 mention-jump-in-view`): a jump to a message — a mention in the
// Documents list, a deep link, a link in a message — lands its row in view and lights it, and the
// row it landed on is the reader's place until they move. Background arrivals after the landing
// (a live message, summaries arriving or going, a reaction's row growing, the freshness line, a
// page of older history above) hold the reader by the first row on screen instead, and each one
// that changes height between that row and the landed one moves the landed row a little —
// together, off the screen the jump put it on. So the landed row itself becomes the scroll
// anchor: `preservingScroll`'s capture asks for it first (as it asks for a filter change's held
// row), the rebuilt rows are re-found by message id, and the lists' keepers put it back where
// the jump put it while sizes change under it. Nothing here scrolls on its own: it only holds
// what a jump — the reader's own act — placed, and it lets go the moment the reader scrolls,
// the list's context changes, or the next jump lands.
//
// And the lost jump's trace (`#1208 scroll-trace-layout-shift`, the coordinator's add-on): the
// auto-sent traces only fire when the view moves with no hand in it, and a jump's click is a
// hand, so a jump target pushed off screen sent nothing. For a short window after a landing
// this watches the target: left the viewport while the reader has not moved, it tells the
// trace, which posts itself as a `jump-lost` entry through its own send path.

/** How long after a landing a target leaving the viewport is the jump's loss, in milliseconds. */
export const JUMP_LOST_WINDOW_MS = 10_000;
/** How often the landed row is checked while that window runs, in milliseconds. */
export const JUMP_LOST_CHECK_MS = 250;

/** What the jump anchor asks of the page. */
export interface JumpAnchorDependencies {
  /** The page's element by id (`el`), in the column being worked on. */
  el(id: string): HTMLElement;
  /** Which list the reader is on, in one key (`channelContextKey`): channel, view, thread, pins. */
  contextKey(): string;
  /** The row now holding message `id` in the list, or null where none does. */
  rowById(id: string): HTMLElement | null;
  /** Where the list's place is measured from (`listPlaceTop`). */
  listPlaceTop(area: HTMLElement): number;
  /** The clock, in milliseconds. */
  now(): number;
  /** The timers the lost-jump watch runs on. */
  clock: {
    setTimeout(run: () => void, ms: number): unknown;
    clearTimeout(handle: unknown): void;
  };
  /** Move the list by the measured drift, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(area: HTMLElement, delta: number): void;
  /** Tell the scroll trace a jump target was lost (`#1208`): it notes and posts itself. */
  noteJumpLost(id: string): void;
}

/** The jump anchor's commands and questions. */
export interface JumpAnchor {
  /** Note `row`, holding message `id`, as where a jump just landed, at the height it now stands. */
  note(row: HTMLElement, id: string): void;
  /** The held row, re-found by its message in the rows now drawn, or null once the hold is over. */
  row(): HTMLElement | null;
  /** Put the held row back where the jump put it. True where a row is held and was placed. */
  place(): boolean;
  /** Let the anchor go: the reader moved, the mark went out, or another place was taken. */
  clear(): void;
  /**
   * Whether the landed row has left the viewport inside the lost window while the reader has
   * not moved; saying so to the trace, once, is this call's whole act. The watch below calls
   * it, and a keeper that finds the target gone may too.
   */
  checkLost(): boolean;
}

/** The one landing held: its list, its message, its row and where that row stands. */
interface JumpHold {
  readonly key: string;
  readonly id: string;
  row: HTMLElement;
  offset: number;
  at: number;
  landedAt: number;
  reported: boolean;
}

/** Hold the row a jump landed on as the reader's place. */
export function createJumpAnchor(deps: JumpAnchorDependencies): JumpAnchor {
  let hold: JumpHold | null = null;
  let timer: unknown = null;

  function stopWatch(): void {
    if (timer !== null) deps.clock.clearTimeout(timer);
    timer = null;
  }

  function clear(): void {
    hold = null;
    stopWatch();
  }

  /**
   * The hold as it stands, or null and forgotten: another list, its row gone from the rows
   * drawn (kept only while a re-draw may still bring it back — asked by id, not by node), or
   * the reader moved. A reader's own scroll ends it: the scroller and the row gone opposite
   * ways by the same amount, as a filter hold's ends (`currentFilterHold`) — the page's own
   * corrections and in-flight redraws move only one of the two, and are not the reader.
   */
  function current(): JumpHold | null {
    if (hold === null) return null;
    if (hold.key !== deps.contextKey()) {
      clear();
      return null;
    }
    const area = deps.el("scroll-area");
    const found = deps.rowById(hold.id);
    if (found === null) {
      // No row holds it in what is drawn now: a rebuild between drawings is not the reader
      // moving, so the hold stands for its row to be drawn again — but nothing can be placed
      // or measured until it is. Past the lost window, a row never drawn again is let go.
      if (deps.now() - hold.landedAt > JUMP_LOST_WINDOW_MS) {
        clear();
        return null;
      }
      return hold;
    }
    hold.row = found;
    // The reader's own scroll is the one move that ends the hold, and it is known by its
    // shape: the scroller went one way by exactly what the row went the other (the same
    // list, nothing in it changed). The page's own acts differ — a correction that kept
    // the row where it stands (the scroller moved, the row did not), a row displaced
    // mid-redraw before its correction lands (the row moved, the scroller did not), a
    // step back still in flight (the scroller reset while older rows stand in above) —
    // and none of them is the reader moving, so none ends the hold.
    const scrollDelta = area.scrollTop - hold.at;
    const rowDelta = found.getBoundingClientRect().top - deps.listPlaceTop(area) - hold.offset;
    if (Math.abs(scrollDelta) > 1 && Math.abs(rowDelta) > 1 && Math.abs(scrollDelta + rowDelta) <= 2) {
      clear();
      return null;
    }
    // Where the page kept the row at its offset, its scroller is the reader's place now:
    // the hold follows it, so a later mid-redraw measure is not read as a move either.
    if (Math.abs(rowDelta) <= 1) hold.at = area.scrollTop;
    return hold;
  }

  function row(): HTMLElement | null {
    const held = current();
    if (held === null) return null;
    const found = deps.rowById(held.id);
    return found === null || found.parentNode === null ? null : found;
  }

  function place(): boolean {
    const held = current();
    if (held === null) return false;
    const found = deps.rowById(held.id);
    if (found === null || found.parentNode === null) return false;
    const area = deps.el("scroll-area");
    // The keepers ask on every size report: already at its offset, the hold costs one
    // measurement and no scroll write.
    const drift = found.getBoundingClientRect().top - deps.listPlaceTop(area) - held.offset;
    if (Math.abs(drift) <= 1) return true;
    // A scroller elsewhere than the hold's place with the row displaced too is an act in
    // flight (a step back whose settle has not landed): its own settle places the reader,
    // and placing here would chase the row's mid-flight stand. Abstain, keeping the hold.
    if (Math.abs(area.scrollTop - held.at) > 1) return false;
    deps.nudge(area, drift);
    hold = { ...held, row: found, at: area.scrollTop };
    return true;
  }

  function checkLost(): boolean {
    if (hold === null || hold.reported) return false;
    if (deps.now() - hold.landedAt > JUMP_LOST_WINDOW_MS) return false;
    // A reader who scrolled ends the hold first: their target leaving is their own act.
    const held = current();
    if (held === null || held.reported) return false;
    const found = deps.rowById(held.id);
    if (found === null || found.parentNode === null) return false;
    const area = deps.el("scroll-area");
    const box = found.getBoundingClientRect();
    const view = area.getBoundingClientRect();
    if (box.bottom > view.top && box.top < view.bottom) return false;
    held.reported = true;
    stopWatch();
    deps.noteJumpLost(held.id);
    return true;
  }

  function watch(): void {
    stopWatch();
    const tick = (): void => {
      timer = null;
      if (hold === null || hold.reported) return;
      if (deps.now() - hold.landedAt > JUMP_LOST_WINDOW_MS) {
        // Still in view at the window's end, or already let go: the watch is over either way.
        return;
      }
      if (checkLost()) return;
      if (hold !== null) timer = deps.clock.setTimeout(tick, JUMP_LOST_CHECK_MS);
    };
    timer = deps.clock.setTimeout(tick, JUMP_LOST_CHECK_MS);
  }

  return {
    note(found, id) {
      clear();
      const area = deps.el("scroll-area");
      hold = {
        key: deps.contextKey(),
        id: String(id),
        row: found,
        offset: found.getBoundingClientRect().top - deps.listPlaceTop(area),
        at: area.scrollTop,
        landedAt: deps.now(),
        reported: false,
      };
      watch();
    },
    row,
    place,
    clear,
    checkLost,
  };
}

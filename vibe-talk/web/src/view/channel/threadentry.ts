// Entering a thread, on its first unread message (`#221 read-modes`, `#220
// view-switch-instant`, `#222 unread-replies`, `#280 scroll-stability`, `#324
// live-pill-covers-row`, `#338 column-overscroll`; `#624 voice-page-modularity`, moved from
// the legacy script into view/channel for the plan's step 17): the landing owed on entry, the
// first unread message put on screen unfolded with the root above it when both fit, and the
// landing kept across the read behind the held rows. The column's entry record, the list and
// the reads are handed in; the words are the page's.

import { childByClass } from "../dom/index.ts";
import { idsOf, shownRowFor } from "./rows.ts";
import { REPLY_LANDING_GAP_PX } from "./replyarrows.ts";
import { expandLandedRow } from "./fold.ts";
import type { GestureStart } from "./sidecontrols.ts";

/**
 * A drill-in anchor measured on the message clicked in the channel (`#1818
 * vibe-talk-thread-drill-in-keeps-anchor`): that message's id, and its top on
 * the screen (its viewport top) at the click.
 */
export interface DrillAnchor {
  id: string;
  top: number;
}

/** A column's own record of entering a view (`freshEntryState`). */
export interface EntryState {
  threadOwed: string | null;
  threadAwaitsRead: boolean;
  readOwed: string | null;
  landing: { key: string; row: HTMLElement; first: HTMLElement; at: number } | null;
  backGesture: GestureStart | null;
  /** The drill-in anchor owed by this entry, with the thread it was measured for, or null. */
  drill: (DrillAnchor & { thread: string }) | null;
}

/** A fresh record of entering a view (`freshEntryState`). */
export function freshEntryState(): EntryState {
  return { threadOwed: null, threadAwaitsRead: false, readOwed: null, landing: null, backGesture: null, drill: null };
}

/** The landing a read is about to rebuild the rows under (`entryLandingMark`). */
export interface EntryLandingMark {
  ids: string[];
  top: number;
  first: string[];
  open: boolean;
}

/** What entering a thread is handed by the page. */
export interface ThreadEntryDependencies {
  el(id: string): HTMLElement;
  activeEntry(): EntryState;
  channelContextKey(): string;
  selection(): { view: string; threadId: string | null };
  channelViewsHas(key: string): boolean;
  viewKey(): string;
  applyReadRuns(): void;
  atBottom(area: HTMLElement): boolean;
  renderScrollTools(): void;
  requestVisibleSummaries(): void;
  floatingClearance(area: Element): number;
  drawnTop(row: Element): number;
  openThreadRootId(): string | null;
  isOpenThreadRoot(row: HTMLElement, id: string | null): boolean;
  rowIsRead(row: HTMLElement): boolean;
  toFoot(area: HTMLElement): void;
  /** Move the list by a measured delta, or put it at a measured position, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(area: HTMLElement, delta: number): void;
  toPosition(area: HTMLElement, top: number): void;
  setJumpNewest(wanted: boolean): void;
  screen(): string;
  view(): string;
  pinsView(): { only: boolean };
  /**
   * Whether a composer holds the view (`#1839 vibe-talk-composer-stays-in-view-while-dictating`):
   * a message box has focus or input arrived in one within the last few seconds. While it
   * stands, no entry landing or drill anchor moves the list: the reader is dictating into the
   * box, and holding a message still must not carry that box off screen. Absent: never held.
   */
  composerHeld?(): boolean;
  /**
   * Whether Read is actively following (`#1856 vibe-talk-read-aloud-follow-not-yanked-up`):
   * a message is playing and not held. While it stands, no entry landing or drill anchor
   * moves the list either: the follow keeps the read sentence on screen, and re-placing a
   * drilled message's top on every repaint yanked the list back up over it. Absent: never.
   */
  readFollowing?(): boolean;
}

/** Entering a thread, as the page calls it. */
export interface ThreadEntry {
  entryState(): EntryState;
  payThreadEntry(): void;
  entryLandingMark(): EntryLandingMark | null;
  keepEntryLanding(mark: EntryLandingMark): boolean;
  landOnFirstUnread(): boolean;
  placeEntryLanding(row: HTMLElement, first: HTMLElement): void;
  entryLandingHolds(): boolean;
}

/** Make the thread entry from the page's column record, list and reads. */
export function createThreadEntry(deps: ThreadEntryDependencies): ThreadEntry {

  /** The column being worked on's record of entering a view (`freshEntryState`). */
  function entryState(): EntryState {
    return deps.activeEntry();
  }

  // The thread entered and still owed its landing, as its `channelContextKey`, or null. Kept per column: `entryState().threadOwed` (`freshEntryState`, `#624 voice-page-modularity`).

  /*
   * Whether the landing owed waits for the read behind rows the store held (`drawHeldRows`, `#220
   * view-switch-instant`), because none of them was unread. Those rows are the thread's newest stretch,
   * from where All's window starts: an older message the read brings may still be unread. Paid only to
   * a reader still on the newest line, where the switch left them. One who has scrolled since has
   * chosen what to read, and is not moved. Nor by a later read when that one fails: it is the read
   * that answers the entry, and a poll's, forty-five seconds on, is not.
   **/
  // Kept per column: `entryState().threadAwaitsRead` (`freshEntryState`, `#624 voice-page-modularity`).

  /*
   * The thread entered, as its `channelContextKey`, while its cover does not answer for it: its root
   * reports replies the store has not brought (`threadOutrunsStore`). `#222 unread-replies`. A tap on
   * "1 new" is the case. The thread is drawn at once from what this page holds — its rows from an
   * earlier visit, or the cover saved on the device — and read behind them, and until that read lands
   * its rows are what `drawHeldRows` draws: the newest stretch so far, not the whole. So the landing is
   * made on them only on an unread message they hold, and otherwise waits for the read, which brings the
   * reply the reader came for (`payThreadEntry`), and keeps a landing made meanwhile (`keepEntryLanding`).
   * Null otherwise.
   **/
  // Kept per column: `entryState().readOwed` (`freshEntryState`, `#624 voice-page-modularity`).

  /**
   * How far down the room under the floating line the first unread message may start with the root
   * still kept at the head of the list: half. Below that too little of the message shows to read
   * without scrolling, and the message the reader came for outranks the one it answers.
   */
  const ENTRY_ROOT_ROOM = 0.5;

  // --- the drill-in anchor (`#1818 vibe-talk-thread-drill-in-keeps-anchor`) -------------------------
  //
  // The owner (thread 158, 2026-11-17): "when I have the message X on screen and I click to the
  // thread containing it, I want X to still be on screen. IDEALLY we would be able to place X at
  // the SAME place on the screen so that what is above/below changes but not it." So a drill-in
  // from X's Thread(n) button measures X's top on the screen (its viewport top) before
  // navigating (`drillAnchorFor` in threadlist.ts) and this entry puts X back at that screen
  // place once the thread is drawn, in place of the first-unread landing: the reader clicked
  // that message, and keeping it still outranks landing on a different one. The measure is the
  // screen's, not the scroll container's: entering a thread puts its header card above the
  // scroller and pushes the whole area down, so a container-relative place lands X that much
  // lower on screen (about 270px on a phone). If X stood too near the top to stay put — the
  // header would cover it — it is kept fully visible just below the floating line instead. If X
  // is the root, the root is the anchor, as its id is what was measured. The anchor is then kept through late layout — an image loading
  // above X, a summary settling above it — by a ResizeObserver (never a timer), until the reader
  // scrolls, leaves the thread, or enters another view. Back keeps what it does today: leaving
  // the thread clears the anchor and the held view restores as before.

  /** The live keeper of a placed drill anchor: its entry, its measure, and where it left the list. */
  interface DrillKeeper {
    entry: EntryState;
    drill: DrillAnchor & { thread: string };
    at: number;
    resize: ResizeObserver | null;
    mutation: MutationObserver | null;
  }

  let drillKeeper: DrillKeeper | null = null;

  function stopDrillKeeper(): void {
    if (drillKeeper?.resize) drillKeeper.resize.disconnect();
    if (drillKeeper?.mutation) drillKeeper.mutation.disconnect();
    drillKeeper = null;
  }

  /** The drawn row standing for message `id` in the thread's list, or null while it is not drawn. */
  function drillRow(id: string): HTMLElement | null {
    const rows = [...deps.el("discord-log").children] as HTMLElement[];
    const found = rows.find((candidate) => idsOf(candidate).includes(id));
    if (!found) return null;
    const row = shownRowFor(found);
    return row && !row.hidden ? row : null;
  }

  /**
   * Put the anchor message back at its measured screen place: its viewport top as it stood at
   * the click, or just below the floating line (which stands below the thread's header card)
   * where the click's place is now under that card. False while its row is not drawn.
   */
  function placeDrillAnchor(drill: DrillAnchor & { thread: string }): boolean {
    // `#1839`: while the composer holds the view the anchor is not placed at all - the
    // measured nudge that keeps the clicked message still is exactly what carries the
    // box being dictated into off screen. Paid (true) so the entry does not retry it as
    // an unpaid landing on every read while the reader writes. `#1856`: the same while
    // Read is following — re-placing the drilled top is what yanked the list back up
    // over the sentence being read, on every repaint.
    if ((deps.composerHeld && deps.composerHeld()) || (deps.readFollowing && deps.readFollowing())) return true;
    const row = drillRow(drill.id);
    if (!row) return false;
    const area = deps.el("scroll-area");
    const target = Math.max(drill.top, deps.floatingClearance(area) + REPLY_LANDING_GAP_PX);
    // By where the row starts (`drawnTop`: its date rule above the tile included), as a landing
    // is placed: a tile put just below the floating line leaves that rule under it
    // (`#324 live-pill-covers-row`, live-pill-browser's thread-lands case).
    const delta = deps.drawnTop(row) - target;
    if (Math.abs(delta) > 0.5) deps.nudge(area, delta);
    deps.renderScrollTools();
    return true;
  }

  /**
   * Keep a placed anchor at its offset through late layout: every resize of the list's rows (an
   * image loading above it), of the summary seam above the list, or of the list's own rows coming
   * and going re-places it. The keeper lets go the moment its entry's anchor is no longer the one
   * measured, the reader scrolls away from where it left the list, or the thread is left — and it
   * never fires on a clock.
   */
  function keepDrillAnchor(drill: DrillAnchor & { thread: string }): void {
    stopDrillKeeper();
    const entry = entryState();
    const area = deps.el("scroll-area");
    const list = deps.el("discord-log");
    const keeper: DrillKeeper = { entry, drill, at: area.scrollTop, resize: null, mutation: null };
    drillKeeper = keeper;
    const correct = (): void => {
      if (drillKeeper !== keeper) return;
      // `#1839`: the keeper stands down while the composer holds the view, for good:
      // re-placing its message on a live-refresh mutation is the reported jump.
      // `#1856`: the same while Read is following — the keeper's re-place on every
      // repaint fought the follow's down-write for the whole read.
      if ((deps.composerHeld && deps.composerHeld()) || (deps.readFollowing && deps.readFollowing())) {
        stopDrillKeeper();
        return;
      }
      if (entry.drill !== drill) {
        stopDrillKeeper();
        return;
      }
      if (deps.activeEntry() !== entry) return;
      if (deps.selection().view !== "thread" || String(deps.selection().threadId ?? "") !== drill.thread) {
        stopDrillKeeper();
        return;
      }
      if (Math.abs(area.scrollTop - keeper.at) > 1) {
        stopDrillKeeper();
        return;
      }
      if (keeper.resize) {
        for (const child of [...list.children]) keeper.resize.observe(child as Element);
      }
      if (!placeDrillAnchor(drill)) return;
      keeper.at = area.scrollTop;
    };
    if (typeof ResizeObserver === "function") {
      keeper.resize = new ResizeObserver(correct);
      keeper.resize.observe(list);
      for (const child of [...list.children]) keeper.resize.observe(child as Element);
      keeper.resize.observe(deps.el("channel-summary"));
    }
    if (typeof MutationObserver === "function") {
      keeper.mutation = new MutationObserver(correct);
      keeper.mutation.observe(list, { childList: true });
    }
  }

  /**
   * Land the thread just entered on its first unread message, once its rows are drawn. Called where a
   * thread's rows first appear: from what the page holds, or when the entering read lands. A read for
   * any other view, or a thread left before its read landed, owes nothing.
   */
  function payThreadEntry(): void {
    // `#1818`: a drill-in anchor owed by this entry is paid before any first-unread landing, and
    // while its row is still on its way (the entering read has not drawn it) nothing else lands:
    // the reader is waiting on that message, not on the newest line the switch left them at.
    const drill = entryState().drill;
    if (drill) {
      if (deps.selection().view !== "thread" || String(deps.selection().threadId ?? "") !== drill.thread) {
        entryState().drill = null;
        stopDrillKeeper();
      } else if (placeDrillAnchor(drill)) {
        entryState().threadOwed = null;
        entryState().threadAwaitsRead = false;
        keepDrillAnchor(drill);
        deps.requestVisibleSummaries();
        return;
      } else if (!(deps.channelViewsHas(deps.viewKey()) && entryState().readOwed !== deps.channelContextKey())) {
        return;
      } else {
        // The thread is fully held and still has no row for the clicked message: the anchor
        // cannot be paid, so the entry lands as any other (`landOnFirstUnread` below).
        entryState().drill = null;
        stopDrillKeeper();
      }
    }
    if (entryState().threadOwed === null) return;
    if (entryState().threadOwed !== deps.channelContextKey() || deps.selection().view !== "thread") {
      entryState().threadOwed = null;
      return;
    }
    const covered = deps.channelViewsHas(deps.viewKey()) && entryState().readOwed !== deps.channelContextKey();
    if ((deps.el("discord-log").children.length === 0 || entryState().threadAwaitsRead) && !covered) return;
    const awaited = entryState().threadAwaitsRead;
    entryState().threadOwed = null;
    entryState().threadAwaitsRead = false;
    if (awaited && !deps.atBottom(deps.el("scroll-area"))) return;
    // The thread's runs were forgotten on entry; a view this page held was drawn before that.
    deps.applyReadRuns();
    // On rows drawn from the store ahead of the thread's read (`drawHeldRows`) the landing is made now,
    // and that read keeps it (`keepEntryLanding`). With nothing unread among them, the read pays it.
    if (!landOnFirstUnread() && !covered) {
      entryState().threadOwed = deps.channelContextKey();
      entryState().threadAwaitsRead = true;
    }
  }

  /**
   * The landing a read is about to rebuild the rows under, `{ ids, top, first, open }`: the messages
   * of the row the entry brought to the head and where on screen that row stands now, and the
   * messages of the first unread row it landed on and whether that row's fold is open. Null when the
   * reader has moved since the landing, or there was none.
   */
  function entryLandingMark(): EntryLandingMark | null {
    if (!entryLandingHolds()) return null;
    const { row, first } = entryState().landing as { row: HTMLElement; first: HTMLElement };
    return { ids: idsOf(row), top: row.getBoundingClientRect().top, first: idsOf(first),
      open: first.getAttribute("data-collapsed") === "false" };
  }

  /**
   * `#221 read-modes` x `#220 view-switch-instant`. A thread drawn from rows the store held lands on
   * its first unread among them at once, and the read behind those rows rebuilds the list: the root
   * and older replies arrive above the landed message, and the newest line may move below it. The read
   * keeps its reader by the row at the top of the screen, or on the newest line, and either can move
   * the landed message — a row above it that grows by a message the read brought, or a list that was
   * on its newest line because it ended just below the landing. So the landing is made again on the
   * rows the read drew, as entering the thread with all of them held would have made it:
   *
   * - THE ROOT, when it fits. An old thread is exactly one whose root precedes All's window, so the
   *   held rows never hold it, and the rule that keeps the root above the first unread could not
   *   apply until the read brought it (`entryHeadRow`). It applies now: the root goes to the head and
   *   the message moves down under it, at most into the upper half of the screen.
   * - Otherwise THE ROW AT THE HEAD STAYS where it stood: the message, or a root the landing already
   *   brought there. Opening something under that root does not scroll, so the landing still holds,
   *   and the root may no longer fit: a reader who opened the run of read messages under it is
   *   reading them, and is not sent down past them to the message.
   * - OPEN, as it was. A fold is kept by its row's first message, and the read can join an older
   *   message of the same author at the head of the landed row: the row is then kept under that
   *   message, which nobody opened. It is opened through its own control, so the record of opened
   *   messages has it, and a fold the reader makes afterwards holds as any other.
   *
   * The landing goes on holding from there. False, with nothing moved, when the read did not draw the
   * landed message.
   */
  function keepEntryLanding(mark: EntryLandingMark): boolean {
    // `#1856`: while Read is following, the landing is not re-placed at all: the follow
    // owns the reader's place, and a nudge back to the landed row is the reported yank.
    if (deps.readFollowing && deps.readFollowing()) return true;
    const rows = ([...deps.el("discord-log").children] as HTMLElement[]) as HTMLElement[];
    const rowFor = (ids: readonly string[]) => shownRowFor(rows.find((candidate) => idsOf(candidate).some((id) => ids.includes(id))));
    const first = rowFor(mark.first);
    if (!first || first.hidden) return false;
    const fold = childByClass(first, "fold");
    if (mark.open && fold && !first.hasAttribute("data-read-run") && first.getAttribute("data-collapsed") === "true") {
      fold.click();
    }
    const head = entryHeadRow(first);
    const kept = rowFor(mark.ids);
    const area = deps.el("scroll-area");
    if (head !== first && head !== kept) {
      placeEntryLanding(head, first);
    } else {
      const row = kept && !kept.hidden ? kept : first;
      deps.nudge(area, row.getBoundingClientRect().top - mark.top);
      entryState().landing = { key: deps.channelContextKey(), row, first, at: area.scrollTop };
    }
    deps.renderScrollTools();
    deps.requestVisibleSummaries();
    return true;
  }

  /** Whether `row` is drawn as a message of its own: not hidden, not behind a run's line, not filtered out. */
  function drawnOnItsOwn(row: HTMLElement): boolean {
    return !row.hidden && !row.hasAttribute("data-read-run") &&
      !String(row.className).split(/\s+/).includes("search-hidden");
  }

  /**
   * The row a landing on the first unread row `first` brings to the head of the list: the thread's
   * root when it is drawn and `first` then still starts within `ENTRY_ROOT_ROOM` of the room under the
   * floating line, else `first`. Measured with `first` open, as it is shown.
   */
  function entryHeadRow(first: HTMLElement): HTMLElement {
    const area = deps.el("scroll-area");
    const head = deps.floatingClearance(area) + REPLY_LANDING_GAP_PX;
    const tools = deps.el("scroll-tools");
    const foot = area.getBoundingClientRect().top + area.clientHeight -
      (tools.hidden ? 0 : tools.getBoundingClientRect().height);
    const rootId = deps.openThreadRootId();
    const root = ([...deps.el("discord-log").children] as HTMLElement[]).find((row) => drawnOnItsOwn(row) && deps.isOpenThreadRoot(row, rootId));
    const rootFits = root && root !== first &&
      first.getBoundingClientRect().top - root.getBoundingClientRect().top <= (foot - head) * ENTRY_ROOT_ROOM;
    return rootFits ? root : first;
  }

  /**
   * Put the first unread message of the list on screen, unfolded. False, and nothing moved, when the
   * list holds nothing unread that is drawn.
   */
  function landOnFirstUnread(): boolean {
    const first = ([...deps.el("discord-log").children] as HTMLElement[]).find((row) => drawnOnItsOwn(row) && !deps.rowIsRead(row));
    if (!first) return false;
    // Opened by its own control, as a tap opens it. (It went by the control when the page also kept a
    // list of folds, which a view held and drawn again came back missing from; the rows are the one
    // record now, `foldsOf`.)
    const fold = childByClass(first, "fold");
    if (fold && first.getAttribute("data-collapsed") === "true") fold.click();
    if (!landAtFoot(first)) placeEntryLanding(entryHeadRow(first), first);
    deps.renderScrollTools();
    deps.requestVisibleSummaries();
    return true;
  }

  /**
   * `#280 scroll-stability`. The first unread message is the thread's NEWEST, and shown whole at the foot of
   * the list: the thread opens there, scrolled all the way down, as a thread with nothing unread does — the
   * newest message, the box under it, and as much above as fits. False, with the list left where it was,
   * when the message is not the newest, or is taller than the room under the floating line, whose head
   * the landing then shows (`placeEntryLanding`).
   *
   * The owner, 2026-10-13: "the thread view also feels like it is not starting out scrolled all the way to
   * the bottom". A thread whose one unread reply was its newest was landed with the root brought to the
   * head (`entryHeadRow`), which in Chromium left the box under the reply, and the foot, 88px below the
   * screen at a desk (tests/scroll_stability_browser.py). Landed at the foot, it is followed from there as
   * any reader on the newest line is.
   *
   * @param {Element} first
   */
  function landAtFoot(first: HTMLElement): boolean {
    const rows = ([...deps.el("discord-log").children] as HTMLElement[]).filter((row) => drawnOnItsOwn(row));
    if (rows[rows.length - 1] !== first) return false;
    const area = deps.el("scroll-area");
    const was = area.scrollTop;
    // A landing: on the foot, never in the room past it (`#338 column-overscroll`).
    deps.toFoot(area);
    if (deps.drawnTop(first) < deps.floatingClearance(area) + REPLY_LANDING_GAP_PX - 1) {
      deps.toPosition(area, was);
      return false;
    }
    entryState().landing = null;
    deps.setJumpNewest(false);
    return true;
  }

  /*
   * Where the entry put the reader, as `{ key, row, first, at }`: the thread, the row it brought to
   * the head of the list, the first unread row it landed on (the same row, or the one under the root),
   * and the scroll position that did it. Null once the reader has moved.
   *
   * KEPT, because entering a thread also changes what is around the list, and that is reported after
   * the landing. On a desk the thread picker takes the thread's name and the dock wraps onto a second
   * row (`#218 desktop-dock`); opening the message's fold can put "Collapse all" up under the list.
   * The observers of both keep a reader within the slack of the newest line ON it — and a landing in a
   * short thread is within that slack on purpose, because the list ends close below the message. Read
   * as a reader following the newest line, the landing was undone as soon as it was made: on a desk in
   * Hide read the message went up under the pill and past the list's top edge.
   *
   * So until the reader scrolls, the landing outranks the newest line: those observers put the landed
   * row back at the head instead, as near as the list allows.
   **/
  // Kept per column: `entryState().landing` (`freshEntryState`, `#624 voice-page-modularity`).

  /**
   * Bring `row` to just under the floating line, as near as the list allows, and keep it there, for
   * the first unread row `first`: `row` itself, or the one under the root.
   */
  function placeEntryLanding(row: HTMLElement, first: HTMLElement): void {
    // `#1839`: no landing write while the composer holds the view; the landing is
    // still recorded, so entryLandingHolds keeps it for when the reader stops writing.
    // `#1856`: the same while Read is following.
    if ((deps.composerHeld && deps.composerHeld()) || (deps.readFollowing && deps.readFollowing())) {
      entryState().landing = { key: deps.channelContextKey(), row, first, at: deps.el("scroll-area").scrollTop };
      return;
    }
    // `#1585 linked-message-always-expanded`. The row brought to the head — the thread's
    // root, for a thread-root open — is shown in full, as the first unread below it is.
    expandLandedRow(row);
    const area = deps.el("scroll-area");
    // By where the row starts, its date rule included: `#324 live-pill-covers-row`, see `drawnTop`.
    deps.nudge(area, deps.drawnTop(row) - deps.floatingClearance(area) - REPLY_LANDING_GAP_PX);
    entryState().landing = { key: deps.channelContextKey(), row, first, at: area.scrollTop };
  }

  /**
   * Whether the reader is still where entering the thread put them: that thread's list on screen, the
   * landed row still drawn in it, and nothing scrolled since. The landing is forgotten once it is not.
   */
  function entryLandingHolds(): boolean {
    const landing = entryState().landing;
    if (landing === null) return false;
    const holds = deps.screen() === "main" && deps.view() === "discord" && deps.selection().view === "thread" &&
      !deps.pinsView().only && landing.key === deps.channelContextKey() && landing.row.parentNode === deps.el("discord-log") &&
      landing.row.getBoundingClientRect().height > 0 && Math.abs(deps.el("scroll-area").scrollTop - landing.at) <= 1;
    if (!holds) entryState().landing = null;
    return holds;
  }

  // The thread selector and older threads are web/src/view/channel's now (`createThreadList`,
  // `#624 voice-page-modularity`): what threads the store knows and what they are called, the
  // view picker and heading, opening and closing a thread, the walk back, the cards, and a row's
  // thread decoration. Changing the view and the selection stay the page's, injected; the
  // unanswered-thread group stays here until the reply-counts move.


  return { entryState, payThreadEntry, entryLandingMark, keepEntryLanding, landOnFirstUnread, placeEntryLanding, entryLandingHolds };
}

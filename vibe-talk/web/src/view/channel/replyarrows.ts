// Following a reply up to what it answers (`#204 reply-arrow`, `#214 reply-arrow`, `#215
// reply-coalesce`'s one tap, `#226 page-energy-profile`, `#228 reply-visible`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel for the plan's step 17):
// the arrow in a reply's gutter, the jump to the message it answers (on screen, in All, or
// further back), the way back, and where each adjacent arrow meets the row above. The columns'
// records, the list and everything the jump walks are handed in; the words are the page's.

import { childByClass } from "../dom/index.ts";
import { idsOf, rowShown } from "./rows.ts";
import { setArrowReach, setMarkerLift } from "./replies.ts";
import { PULL_WAIT_MS } from "./pull.ts";

/** How far below the floating line a row lands, so its top edge is not flush against the pill. */
export const REPLY_LANDING_GAP_PX = 8;

/** A reply jump's ticket: bumped by each tap, and a walk whose ticket has moved on stops. */
export interface ReplyJumpTicket {
  current(): number;
  bump(): void;
}

/** A row of the list, with the arrow this owner puts on a reply's row. */
export interface ReplyArrowRow extends HTMLElement {
  replyArrowButton?: HTMLElement;
}

/** A column's own record of its reply arrows and jumps (`freshReplyArrows`). */
export interface ReplyArrowsState {
  back: { id: string; context: string } | null;
  landed: { row: ReplyArrowRow; timer: ReturnType<typeof setTimeout> } | null;
  tap: { arrow: HTMLElement; jump: () => void; timer: ReturnType<typeof setTimeout> } | null;
  settling: ReturnType<typeof setTimeout> | null;
  held: number;
  owed: boolean;
  countsDrawn: boolean;
}

/** A fresh record of a column's reply arrows (`freshReplyArrows`). */
export function freshReplyArrows(): ReplyArrowsState {
  return { back: null, landed: null, tap: null, settling: null, held: 0, owed: false, countsDrawn: false };
}

/** What the reply arrows and the jump walk are handed by the page. */
export interface ReplyArrowsDependencies {
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  guardQuietly(fn: (...args: never[]) => unknown): (...args: never[]) => unknown;
  gatherFromReply(row: ReplyArrowRow): void;
  filterState(): { hold: unknown };
  endFilterWalk(): void;
  endSearchWalks(): void;
  selectLanded(row: ReplyArrowRow): void;
  setSearchOpen(open: boolean): void;
  openReadRun(row: ReplyArrowRow): void;
  renderScrollTools(): void;
  setStatus(text: string): void;
  channelContextKey(): string;
  heldMessage(id: string): unknown;
  inView(message: unknown, view: string, threadId: string | null): boolean;
  selection(): { view: string; threadId: string | null };
  threading(): boolean;
  olderPages(): { moreAbove: boolean; cursor: unknown };
  viewKey(): string;
  channelCanon(): { views: Map<string, { more?: boolean }> };
  selectedChannelId(): unknown;
  changeChannelView(view: string): Promise<void>;
  channelReadsBusy(): boolean;
  loadOlder(): Promise<void> | null;
  walkCursorOwed(): boolean;
  hidingRead(): boolean;
  timelineMessages(): readonly { id: unknown }[];
  hiddenEmpty(message: unknown): boolean;
  readHiding(): boolean;
  layoutColumns(): boolean;
  columnSteps(): { pause(): void; resume(): void };
  activeArrows(): ReplyArrowsState;
  replyJumpTicket(): ReplyJumpTicket;
}

/** The reply arrows, the jump and its way back, as the page calls them. */
export interface ReplyArrows {
  readonly replyJumpTicket: ReplyJumpTicket;
  replyArrows(): ReplyArrowsState;
  dropReplyArrowTap(): void;
  replyArrow(row: ReplyArrowRow): HTMLElement;
  rowHolding(id: string): ReplyArrowRow | null;
  listPlaceTop(area: Element): number;
  floatingClearance(area: Element): number;
  drawnTop(row: Element): number;
  rowOnScreen(row: Element): boolean;
  revealRow(row: ReplyArrowRow, center?: boolean): void;
  putOutLanding(row: ReplyArrowRow): void;
  replyReturnRow(): ReplyArrowRow | null;
  backToReply(): void;
  landOnAnswered(target: string, from: string): boolean;
  answeredElsewhere(id: string): boolean;
  historyReadToStart(): boolean;
  jumpToAnswered(reply: ReplyArrowRow): Promise<void>;
  seekMessage(target: string, arrive: () => unknown, openAll: () => unknown): Promise<void>;
  measureReplyArrows(): void;
  holdingReplyArrows(draw: () => void): void;
  rootFontPx(): number;
}

/** Make the reply arrows and the jump from the page's columns, list and reads. */
export function createReplyArrows(deps: ReplyArrowsDependencies): ReplyArrows {
  // arrow from the left side of the box pointing upward. And clicking on the arrow should jump to
  // the message it is a reply to."
  //
  // So a reply is inset from the left by one touch target, and that gutter holds an arrow: out of
  // the row's left side, round the corner and up. It is a BUTTON the full width of the gutter at the
  // head of the row, and a tap on it takes the reader to the message the reply answers. Where that
  // message is decides what the tap has to do, in this order:
  //
  //   1. ON SCREEN in this view, in a row of its own or as one constituent of a combined row. That
  //      row is scrolled to the head of the list, under the floating pill and glass, and lit for a
  //      moment.
  //   2. NOT A MESSAGE THIS VIEW SHOWS — a thread reply seen from Main, a main-channel message seen
  //      from inside a thread — or one the page has never read: the tap opens All. All is the one
  //      view that holds every message, so it is the one view in which "not found" means not in the
  //      channel; and the reply itself is in it too, which keeps the way back one tap away. It is
  //      opened the way the picker opens it, so it is also the view this channel is kept in
  //      (`#205 channel-view-memory`): the page reopens where the reader was left, as for any change
  //      of view. All that has to be read is waited for, behind any read already on the wire.
  //   3. FURTHER BACK THAN WHAT IS LOADED: older pages are read, at most REPLY_JUMP_PAGES of them,
  //      and the status line says so as each one is asked for. Bounded, because a message deleted
  //      upstream is never found, and an unbounded walk would read the whole channel looking for it.
  //   4. NOT FOUND: the status line says which — older than what is loaded even after the walk, or
  //      not in the channel's history at all — rather than leaving a tap that visibly did nothing.
  //      The second only when a read has said the history ends there (`historyReadToStart`).
  //
  // Every one of those sentences is short, because the status line is one line, ellipsised: on a
  // 360px phone about 38 characters of it show, and a sentence whose point came after that showed
  // only its preamble.
  //
  // THE WAY BACK is a chip, "Back to reply", over the list beside the Bookmark chip. Leaving the reader where
  // the jump put them, and letting them scroll back, is what happens when the reply is still on
  // screen after the jump, and then no chip is offered. But a reply whose question was five pages up
  // is five pages down from where the tap landed, and the standing rule of this page is that the
  // reader does not lose their place (`#47 scrollback-stability`). The chip goes once it is used,
  // once the reply is back on screen however it got there, and once the list it points into is no
  // longer the one on screen.
  //
  // NONE OF THE ROW'S OWN ACTS FIRE. The arrow is inside the row, and a tap on the row folds it or
  // reads it aloud (`tapRow`): the arrow's click stops at the arrow. A finger that comes down on the
  // arrow is not the start of a swipe or a press-and-hold on the row either (`swipeable`).
  //
  // TWO TAPS are another act: they gather the conversation the reply is in under its first message
  // (`#215 reply-coalesce`, below). So one tap waits `REPLY_DOUBLE_TAP_MS` before it jumps, long enough
  // to know a second is not coming. And since `#214 reply-arrow` the arrow is one of two, drawn by
  // whether the row above is the message it answers; see the section after this one.

  /** How many older pages one tap on a reply's arrow may read, looking for what the reply answers. */
  const REPLY_JUMP_PAGES = 5;

  /** How long the row a jump lands on stays lit: long enough to find, gone before it is clutter. */
  const REPLY_LANDED_MS = 2000;

  /** The arrow's accessible name and its tooltip. No channel data in it, for the reason Reply has none. */
  const REPLY_JUMP_NAME = "Jump to the message this answers";

  /**
   * Every tap on an arrow takes a ticket, and a walk whose ticket has moved on stops where it is: a generation,
   * bumped by each tap and by the way back from a jump (web/src/core/sync `createStreamGeneration`, `#624
   * voice-page-modularity`).
   */
  const replyJumpTicket = deps.replyJumpTicket();


  /** The column being worked on's record of its reply arrows (`freshReplyArrows`). */
  function replyArrows(): ReplyArrowsState {
    return deps.activeArrows();
  }

  // The reply the last jump left, as `{ id, context }`, while "Back to reply" is offered; else null. Kept per column: `replyArrows().back` (`freshReplyArrows`, `#624 voice-page-modularity`).

  // The row lit by the last landing, and the timer that puts it out. Kept per column: `replyArrows().landed` (`freshReplyArrows`, `#624 voice-page-modularity`).

  /**
   * How long a tap on a reply's arrow waits to learn whether a second tap follows, in milliseconds.
   * `#215 reply-coalesce`: two taps gather the conversation the reply is in under its first message,
   * and one tap still jumps to the message it answers.
   *
   * WHY A DELAY AT ALL. The first tap of two cannot be told from a single tap until the second has
   * had its chance to arrive, and the owner asked that a double tap never first jump away — a jump
   * scrolls the list, and the second tap would then land on whatever the scroll put under the
   * finger. So a single tap waits this long. A quarter of a second is about the shortest window in
   * which a deliberate double tap reliably lands, mouse or thumb, and short enough that the jump
   * still reads as the answer to the tap.
   */
  const REPLY_DOUBLE_TAP_MS = 250;

  // The first tap on an arrow, waiting for a possible second, as `{ arrow, jump, timer }`; else null. Kept per column: `replyArrows().tap` (`freshReplyArrows`, `#624 voice-page-modularity`).

  /*
   * The timer of the moment after two taps on an arrow gathered, while taps on an arrow or on the X are
   * ignored; else null. A third tap that close is the same gesture running on, not a new one — a hand
   * that taps three times, or a mouse that reports three clicks — and it lands where the gathering has
   * just changed what is under the finger: on the X that has appeared in the gutter, which would put
   * the replies straight back, or on an arrow, which would wait and then jump away from them.
   **/
  // Kept per column: `replyArrows().settling` (`freshReplyArrows`, `#624 voice-page-modularity`).

  /** Forget a tap on an arrow still waiting to jump: the list it would have jumped in is going. */
  function dropReplyArrowTap(): void {
    const tap = replyArrows().tap;
    if (tap) clearTimeout(tap.timer);
    replyArrows().tap = null;
  }

  /**
   * The control in a reply's gutter. `li` is the reply's own row.
   *
   * One tap jumps to the message the reply answers; two, within `REPLY_DOUBLE_TAP_MS`, gather the
   * conversation the reply is in under its first message (`#215 reply-coalesce`, `conversationHead`),
   * and a third that soon after is ignored (`replyArrows().settling`). Counted from the clicks themselves
   * rather than from `dblclick`, which a phone does not reliably send for two taps — and a keyboard's
   * Enter, pressed twice, is two clicks as well.
   */
  function replyArrow(li: ReplyArrowRow): HTMLElement {
    const arrow = document.createElement("button");
    arrow.className = "reply-jump";
    arrow.setAttribute("type", "button");
    arrow.setAttribute("aria-label", REPLY_JUMP_NAME);
    // The tooltip says what the second tap does, where a pointer can rest to read it. The accessible
    // name stays what one tap does.
    arrow.setAttribute("title", `${REPLY_JUMP_NAME}. Double-tap to gather its whole conversation.`);
    // Which of the two arrows `#214 reply-arrow` draws, decided by `renderReplyLinks` once the row is
    // in the list: the row above is the one it answers, or it is not.
    arrow.setAttribute("data-reply-style", "disconnected");
    // The arrow itself is drawn by web/voice.css on this empty span: a line out of the row's side,
    // round the corner and up, with a head on it. Drawn rather than typed, because a glyph is
    // whatever the phone's font makes of it — "↰" is a hairline in one font and an emoji in
    // another — and the owner asked for something he could see.
    const mark = document.createElement("span");
    mark.className = "reply-jump-mark";
    mark.setAttribute("aria-hidden", "true");
    arrow.append(mark);
    // `#228 reply-visible`. On the row, for the pass that asks which replies are joined to the row
    // above by this arrow (`renderReplyCounts`), without walking the row for it.
    li.replyArrowButton = arrow;
    arrow.addEventListener("click", deps.inThisColumn((event: Event) => {
      // The row's own tap handler would fold the row, or start reading it aloud in reading mode. It
      // already ignores a click on a button, and this does not rely on that: the click ends here.
      if (event) event.stopPropagation();
      if (replyArrows().settling !== null) return;
      const tap0 = replyArrows().tap;
      if (tap0 && tap0.arrow === arrow) {
        dropReplyArrowTap();
        replyArrows().settling = setTimeout(deps.inThisColumn(() => {
          replyArrows().settling = null;
        }), REPLY_DOUBLE_TAP_MS);
        deps.guardQuietly(() => deps.gatherFromReply(li))();
        return;
      }
      // A first tap on ANOTHER arrow still waiting is that arrow's single tap, and it is taken now:
      // the reader has moved on to this one, and dropping it would be a tap that did nothing.
      const waiting = replyArrows().tap;
      if (waiting) {
        clearTimeout(waiting.timer);
        replyArrows().tap = null;
        waiting.jump();
      }
      // Both bound to this column: the wait ends in a later task. `#229 desktop-two-column`.
      const tap = {
        arrow,
        jump: deps.inThisColumn(() => deps.guardQuietly(() => jumpToAnswered(li))()),
        timer: setTimeout(deps.inThisColumn(() => {
          if (replyArrows().tap === tap) replyArrows().tap = null;
          tap.jump();
        }), REPLY_DOUBLE_TAP_MS),
      };
      replyArrows().tap = tap;
    }));
    return arrow;
  }

  /** The row on screen that holds message `id`: its own row, or the combined row it is part of. */
  function rowHolding(id: string): ReplyArrowRow | null {
    return ([...deps.el("discord-log").children].find((row) => idsOf(row).includes(String(id))) as ReplyArrowRow | undefined) || null;
  }

  /**
   * The top of the list's PLACE, in viewport pixels: its box's top, less the room its top edge is moved
   * down by — the pill's line (`#324 live-pill-covers-row`), or what hangs under the open search bar on a
   * narrow phone (`#251 author-filter`), both in web/voice.css. Steady as that room comes and goes, which
   * opening and closing the search does, so a height in the window measured from here is the same height
   * after it. The list's own top moves with the room, and a row put back by it moved with it.
   *
   * @param {Element} area
   */
  function listPlaceTop(area: Element): number {
    const top = area.getBoundingClientRect().top;
    const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(area) : null;
    return top - (style ? parseFloat(style.marginTop) || 0 : 0);
  }

  /**
   * Where the floating line over the head of the list ends, in viewport pixels: the bottom of the
   * freshness pill or of the search glass beside it, whichever is lower, or the list's own top when
   * neither is up. A row put any higher than this is under them.
   */
  function floatingClearance(area: Element): number {
    let clear = area.getBoundingClientRect().top;
    // `#229 desktop-two-column`. The pill and the glass float over the left column only.
    if (area !== deps.leftEl("scroll-area")) return clear;
    for (const id of ["channel-freshness", "search-float"]) {
      const node = deps.el(id);
      if (node.hidden) continue;
      const box = node.getBoundingClientRect();
      if (box.height > 0) clear = Math.max(clear, box.bottom);
    }
    return clear;
  }

  /**
   * Where `row` starts on screen, in viewport pixels: the top of its tile, or of the lines it draws above
   * the tile in the room it keeps there — a day's rule or a gap's line (`#250 date-separators`, `#249
   * search-highlight`), and the bookmark's line when it is above the row (`#240 reading-bookmark`).
   *
   * `#324 live-pill-covers-row`. A landing put the TILE just under the floating line, so the lines over
   * it were put above the line: entering a thread landed on its root and drew the root's date rule under
   * "Live · updated". A row is landed by where it starts.
   *
   * @param {Element} row
   */
  function drawnTop(row: Element): number {
    let top = row.getBoundingClientRect().top;
    for (const child of row.children) {
      const classes = String(child.className).split(/\s+/);
      const above = (classes.includes("row-seam") && !classes.includes("row-seam-below")) ||
        (classes.includes("bookmark-line") && row.getAttribute("data-bookmark") === "above");
      if (!above) continue;
      const box = child.getBoundingClientRect();
      if (box.height > 0) top = Math.min(top, box.top);
    }
    return top;
  }

  /** Whether any of `row` shows between the floating line and the foot of the list. */
  function rowOnScreen(row: Element): boolean {
    const area = deps.el("scroll-area");
    const box = row.getBoundingClientRect();
    return box.bottom > floatingClearance(area) &&
      box.top < area.getBoundingClientRect().top + area.clientHeight;
  }

  /**
   * Put `row` at the head of the list, just under the floating line, and light it for a moment.
   *
   * At the HEAD rather than centred, because what follows a question is usually its answer: the most
   * room below the row is the best chance the reply is still on screen, and then there is nothing to
   * come back from. A row nearer the end of the list than a screenful stops where the list does.
   */
  function revealRow(row: ReplyArrowRow, center = false): void {
    const area = deps.el("scroll-area");
    // The reader asked for this message: a filter change's hold, and its walk back, give way. `#243`.
    deps.filterState().hold = null;
    deps.endFilterWalk();
    const clear = floatingClearance(area) + REPLY_LANDING_GAP_PX;
    // `#580 open-in-chat-jump`: centred in the room under what floats over the list, or under the top
    // as anywhere else when it is too tall to be.
    const height = row.getBoundingClientRect().bottom - drawnTop(row);
    const lead = center ? Math.max(clear, (clear + area.getBoundingClientRect().bottom - height) / 2) : clear;
    area.scrollTop += drawnTop(row) - lead;
    const prior = replyArrows().landed;
    if (prior) {
      clearTimeout(prior.timer);
      prior.row.removeAttribute("data-landed");
    }
    row.setAttribute("data-landed", "true");
    const landed = {
      row,
      timer: setTimeout(deps.inThisColumn(() => {
        row.removeAttribute("data-landed");
        if (replyArrows().landed === landed) replyArrows().landed = null;
      }), REPLY_LANDED_MS),
    };
    replyArrows().landed = landed;
    // `#239 single-selection`. The message the reader asked for is the one every key now acts on, and
    // the flash is on it: never a lit row beside a selected one.
    deps.selectLanded(row);
  }

  /** Put out the flash of the last landing, unless it is on `row`. See `drawSelection`. */
  function putOutLanding(row: ReplyArrowRow): void {
    const landed = replyArrows().landed;
    if (!landed || landed.row === row) return;
    clearTimeout(landed.timer);
    landed.row.removeAttribute("data-landed");
    replyArrows().landed = null;
  }

  /**
   * The reply "Back to reply" would return to: on screen in the list the jump left it in, or null.
   * A different channel, view or thread is a different list, and the chip goes with the list.
   */
  function replyReturnRow(): ReplyArrowRow | null {
    const back = replyArrows().back;
    if (back === null) return null;
    if (back.context !== deps.channelContextKey()) {
      replyArrows().back = null;
      return null;
    }
    return rowHolding(back.id);
  }

  /** "Back to reply": where the jump started, lit as the landing was. */
  function backToReply(): void {
    const row = replyReturnRow();
    replyArrows().back = null;
    if (row) {
      revealRow(row);
      deps.setStatus("Back at the reply.");
    }
    deps.renderScrollTools();
  }

  /**
   * Take the reader to the message `target` if a row on screen holds it, and offer the way back to
   * the reply `from` if the landing took it off the screen. False, and nothing moved, when no row
   * holds it.
   */
  function landOnAnswered(target: string, from: string): boolean {
    const row = rowHolding(target);
    if (!row) return false;
    // The reader asked for this message by name. A search that filtered it out is closed rather
    // than left standing over a row that cannot be scrolled to, because it is not drawn.
    if (String(row.className).split(/\s+/).includes("search-hidden")) deps.setSearchOpen(false);
    // ...and a run of read messages it is collapsed into is opened, for the same reason. `#221 read-modes`.
    deps.openReadRun(row);
    revealRow(row);
    const reply = rowHolding(from);
    replyArrows().back = reply && reply !== row && !rowOnScreen(reply) ? { id: from, context: deps.channelContextKey() } : null;
    deps.renderScrollTools();
    deps.setStatus("Here is the message it answers.");
    return true;
  }

  /**
   * Whether message `id` is one the view on screen cannot show: in another part of the channel, or
   * never read by this page at all, so nothing says which part it is in.
   */
  function answeredElsewhere(id: string): boolean {
    const held = deps.heldMessage(id);
    return held === null || !deps.inView(held, deps.selection().view, deps.selection().threadId);
  }

  /**
   * Whether a read has said the view on screen reaches back to the start of its history, so that a
   * message not in it is not in that part of the channel at all.
   *
   * Not the same as nothing being above the rows. A view no read has landed for yet has nothing
   * above it either: `olderPages().moreAbove` starts false. That is All, opened by a jump while another
   * read held the line, and the message the jump is for may be on the page still to come. Where the
   * provider has threads, only the store's cover for the view says which of the two it is.
   */
  function historyReadToStart(): boolean {
    if (deps.olderPages().moreAbove !== false) return false;
    if (!deps.threading()) return true;
    const cover = deps.channelCanon().views.get(deps.viewKey());
    return Boolean(cover && cover.more === false);
  }

  /** The tap on a reply's arrow. See the head of this section for the order of the four cases. */
  async function jumpToAnswered(reply: ReplyArrowRow): Promise<void> {
    const target = String(reply.getAttribute("data-reply-to") || "");
    const from = String(reply.getAttribute("data-id") || "");
    if (!target) return;
    await seekMessage(target, () => landOnAnswered(target, from),
      () => deps.threading() && deps.selection().view !== "flat" && answeredElsewhere(target));
  }

  /**
   * Bring the row holding message `target` into the list and hand it to `arrive`, which says whether
   * it was there to be acted on. The walk the arrow's jump makes, in the order the head of this
   * section gives, shared since `#215 reply-coalesce` with the gathering of a message's replies,
   * which looks for the message the same way and does something else when it finds it.
   *
   * `openAll` says whether All has to be opened before the message can be found or acted on: for the
   * jump, a message the view on screen cannot show; for the gathering, any view but All. Every call
   * takes a ticket, so a later tap, on either control, ends a walk still under way.
   */
  async function seekMessage(target: string, arrive: () => unknown, openAll: () => unknown): Promise<void> {
    // `#229 desktop-two-column`. The column the walk is in, which its work after every wait is done in,
    // given back before the walk's promise settles, whichever way it ends.
    const steps = deps.columnSteps();
    // A walk back for a filter change gives way to this one, which takes the reader somewhere else. `#243`.
    // So does a walk for the search. `#262 search-load-older`.
    deps.endFilterWalk();
    deps.endSearchWalks();
    try {
      replyJumpTicket.bump();
      const ticket = replyJumpTicket.current();
      const channel = String(deps.selectedChannelId());
      if (arrive()) return;
      // The reader's own filter, said plainly rather than walked past: the message is loaded, read,
      // and Hide read is the reason it is not drawn. No walk would find it. Asked again wherever the
      // list has just changed, because All, or an older page, may be what brings the message in.
      const hiddenAsRead = () => {
        if (!(deps.hidingRead() && deps.threading() && !rowHolding(target) &&
            deps.timelineMessages().some((message) => String(message.id) === target))) {
          return false;
        }
        deps.setStatus("Hide read is hiding that message.");
        return true;
      };
      if (hiddenAsRead()) return;
      // `#386 hide-empty-messages`. Nor would any walk find an empty message while the reader hides those:
      // the store holds it, and no list draws it.
      const held = deps.threading() ? deps.heldMessage(target) : null;
      if (held && deps.hiddenEmpty(held)) {
        deps.setStatus("Settings hides that empty message.");
        return;
      }
      // Never in two columns, whose left column is Main by the layout, and whose right column is a thread
      // a reply's arrow or a gathering does not leave. `#229 desktop-two-column`.
      if (!deps.layoutColumns() && openAll()) {
        deps.setStatus("Not in this view — opening All…");
        // The reader went somewhere else while All was being read: the tap is over.
        const stillAll = () =>
          ticket === replyJumpTicket.current() && String(deps.selectedChannelId()) === channel && deps.selection().view === "flat";
        try {
          const opening = deps.changeChannelView("flat");
          steps.pause();
          await opening;
          steps.resume();
        } catch (error) {
          steps.resume();
          // As for a step back that fails, below: reported where every failed read is, and the status
          // line no longer says All is being opened.
          if (stillAll()) deps.setStatus("Could not read All — try again.");
          throw error;
        }
        // All not held by this page nor covered by the store, and another read — a poll of Main, say —
        // on the wire when the tap came: All's read is queued behind it, and `changeChannelView`
        // returned with nothing drawn. Waited for, as a pull up waits for a read already in flight,
        // because until All has landed an empty list says nothing about where the message is.
        while (deps.channelReadsBusy() && !deps.channelCanon().views.has(deps.viewKey()) && stillAll()) {
          steps.pause();
          await new Promise((resolve) => setTimeout(resolve, PULL_WAIT_MS));
          steps.resume();
        }
        if (!stillAll()) return;
        if (arrive() || hiddenAsRead()) return;
        if (!deps.channelCanon().views.has(deps.viewKey())) {
          // All's read failed, and that is reported where every failed read is. Nothing was learned
          // about the message, so nothing is said about it either — whether or not the rows the page
          // already held for All are up (`#220 view-switch-instant`): those are not All read.
          deps.setStatus("Could not read All — try again.");
          return;
        }
      }
      let walked = 0;
      // Main drawn from All takes its first step from its own read (`walkCursorOwed`, `#220`).
      while (walked < REPLY_JUMP_PAGES && deps.olderPages().moreAbove === true && (deps.olderPages().cursor || deps.walkCursorOwed())) {
        walked += 1;
        const context = deps.channelContextKey();
        deps.setStatus(`Looking further back: page ${walked} of ${REPLY_JUMP_PAGES}…`);
        try {
          const step = deps.loadOlder();
          steps.pause();
          await step;
          steps.resume();
        } catch (error) {
          steps.resume();
          // The failure itself is reported where every failed read is; this only stops the status
          // line from going on saying a walk is under way.
          if (ticket === replyJumpTicket.current()) deps.setStatus("Could not look further back.");
          throw error;
        }
        // Another tap, another channel or another view: this walk is nobody's any more.
        if (ticket !== replyJumpTicket.current() || context !== deps.channelContextKey()) return;
        if (arrive() || hiddenAsRead()) return;
      }
      if (deps.readHiding() && !deps.threading()) {
        deps.setStatus("Not in this list — Hide read is on.");
      } else if (historyReadToStart()) {
        deps.setStatus("Not in this channel's history.");
      } else if (walked > 0) {
        deps.setStatus(`Further back than ${walked} page${walked === 1 ? "" : "s"} — scroll up.`);
      } else {
        deps.setStatus("Older than what is loaded.");
      }
    } finally {
      steps.pause();
    }
  }

  // --- two arrows: the message directly above, or one somewhere else ------------------------------
  //
  // `#214 reply-arrow`. The owner, on his desk, 2026-10-08: "the arrows make it look like the reply
  // points to the message directly above it. But that's misleading when that's not actually the reply
  // chain." Every reply drew the same arrow, out of its upper left and straight up, so every reply
  // looked like the answer to whatever happened to be above it. There are two now:
  //
  //   * ADJACENT. The row directly above holds the message the reply answers. The arrow keeps its
  //     right angle at the box's upper left, and its shaft now runs up across the gap between the two
  //     rows until its head TOUCHES that row: the arrow is a connection, not a gesture upward.
  //   * DISCONNECTED. Anything else — the message is further up, in another view, or not loaded. The
  //     arrow comes out of the MIDDLE of the box's left side and leaves at about forty-five degrees,
  //     a different shape at a different height, so it cannot be read as pointing at the row above.
  //
  // "DIRECTLY ABOVE" is the previous row the reader can SEE in this list: a row the search or Links
  // is hiding is skipped, a message Hide read leaves out is not drawn at all, and the row may be a
  // combined one, which holds the message when any of its constituents is it. Decided again by
  // `renderReplyLinks` whenever what is above a row can have changed — a redraw, a refresh, an older
  // page, a filter, the gathering below.
  //
  // WHERE THE HEAD MEETS IT is layout, so it is measured (`measureReplyArrows`). The arrow's shaft
  // sits in the reply's gutter, and the row above does not always reach over it: the owner's own rows
  // are inset from the left, by far more than a gutter on a desk, and a reply above a reply has its
  // own gutter where this one's shaft would land. Three cases, from where that row's left edge is:
  //
  //   * left of the shaft, by its rounded corner and more: the head meets its bottom edge;
  //   * inside the gutter: the shaft moves right until it is under that edge, and still meets it;
  //   * at or beyond the reply's own box: no shaft in the gutter can reach the bottom edge, so it
  //     rises past it and turns right, and the head meets the row's LEFT side, near its foot.

  /** How far left of a reply's box its arrow's shaft runs, in rem: where web/voice.css draws it. */
  const REPLY_SHAFT_REM = 1.2;

  /** How far in from a row's left edge its bottom edge is flat, past the rounded corner, in rem. */
  const REPLY_CORNER_REM = 0.6;

  /** The nearest a moved shaft may come to the reply's own box, its head clear of the box, in rem. */
  const REPLY_SHAFT_CLEAR_REM = 0.65;

  /** The shortest run to the row's side that has room for its corner and its head, in rem. */
  const REPLY_RUN_MIN_REM = 0.9;

  /**
   * `#228 reply-visible`. How far right of a row's lower-left corner the point of a head under its
   * foot must be, in rem, for the head to keep clear of a circle centred on that corner: half the
   * head's width, the disc's radius, and a gap, which also keeps a press on the head off the circle.
   */
  const REPLY_HUB_CLEAR_REM = 1.1;

  /** The rem on this page, in pixels: the arrow's drawing is in rem and its measurements in pixels. */
  function rootFontPx(): number {
    const style = typeof window.getComputedStyle === "function"
      ? window.getComputedStyle(document.documentElement) : null;
    const size = style ? parseFloat(style.fontSize) : NaN;
    return Number.isFinite(size) && size > 0 ? size : 16;
  }

  /**
   * Where each adjacent arrow meets the row above, from where the two rows are laid out: on its
   * bottom edge (`data-reply-reach="below"`, with `--reply-shift` the distance the shaft moved right
   * to get under it) or on its left side (`"side"`, with `--reply-run` the distance from the shaft to
   * that side). See the head of this section. Nothing is measured in a list that is not laid out —
   * off the channel, or behind another screen — and the next resize, which showing it is, measures.
   *
   * `#226 page-energy-profile`. EVERY RECTANGLE IS READ BEFORE ANYTHING IS WRITTEN. Reading one after
   * an arrow's attributes were written made the browser lay the list out again, so a redraw paid one
   * layout per adjacent arrow, two or three times over: in a profile of live traffic this function
   * and the row-by-row geometry it asked for were a third of the page's work. Now one layout, and only
   * what changed is written, so a second measure of an unchanged list lays nothing out again. Nothing
   * at all is asked of a list with no adjacent arrow, nor of a hidden page, which has no frame to draw
   * and is measured when it is shown again (`replyArrows().owed`); nor inside a redraw, which measures
   * once, at its end (`holdingReplyArrows`).
   */
  function measureReplyArrows(): void {
    if (replyArrows().held > 0) {
      replyArrows().owed = true;
      return;
    }
    replyArrows().owed = false;
    const list = deps.el("discord-log");
    /** @type {{row: any, arrow: any, above: any}[]} */
    const pairs = [];
    let above = null;
    for (const row of list.children) {
      const arrow = childByClass(row, "reply-jump");
      if (arrow && above && arrow.getAttribute("data-reply-style") === "adjacent") pairs.push({ row, arrow, above });
      if (rowShown(row)) above = row;
    }
    if (pairs.length === 0) return;
    if (document.hidden) {
      replyArrows().owed = true;
      return;
    }
    if (list.hidden || !(list.getBoundingClientRect().width > 0)) return;
    const rem = rootFontPx();
    // From the reply's padding edge, which is where web/voice.css hangs the arrow (`right: 100%`).
    const gaps = pairs.map(({ row, above: over }) => {
      const box = row.getBoundingClientRect();
      return over.getBoundingClientRect().left - (box.left + (row.clientLeft || 0));
    });
    pairs.forEach(({ arrow, above: over }, at) => {
      const dx = gaps[at];
      // `#228 reply-visible`. A row with a circle on its corner as well: the circle is lifted up the
      // row's side, out of the way of a head that comes up under its foot too near the corner for
      // both. A head that meets the row's side is raised instead, by web/voice.css.
      const hub = arrow.hasAttribute("data-reply-hub");
      // Where the head would have to be to meet the flat of that row's bottom edge, and how far a
      // turn to its side would have to run, both from the reply's padding edge.
      const meets = (dx as number) + REPLY_CORNER_REM * rem;
      const run = (dx as number) + REPLY_SHAFT_REM * rem;
      if (meets <= -REPLY_SHAFT_CLEAR_REM * rem || run < REPLY_RUN_MIN_REM * rem) {
        // Under it. A row whose edge is so close to the box that neither fits — closer than a turn
        // has room for, too close for the shaft to come up under its flat — is met as near its
        // corner as the shaft may go.
        const furthest = (REPLY_SHAFT_REM - REPLY_SHAFT_CLEAR_REM) * rem;
        const shift = Math.min(furthest, Math.max(0, meets + REPLY_SHAFT_REM * rem));
        setArrowReach(arrow, "below", "--reply-shift", `${shift.toFixed(2)}px`, "--reply-run");
        if (hub) setMarkerLift(over, shift - REPLY_SHAFT_REM * rem - (dx as number) < REPLY_HUB_CLEAR_REM * rem);
      } else {
        setArrowReach(arrow, "side", "--reply-run", `${run.toFixed(2)}px`, "--reply-shift");
        if (hub) setMarkerLift(over, false);
      }
    });
  }

  // How many redraws are under way that measure the arrows once, at their end. `#226`. Kept per column: `replyArrows().held` (`freshReplyArrows`, `#624 voice-page-modularity`).
  // Whether the arrows want measuring: asked inside a redraw, or of a hidden page. `#226`. Kept per column: `replyArrows().owed` (`freshReplyArrows`, `#624 voice-page-modularity`).

  /**
   * Run `draw`, a redraw of the rows, measuring the arrows once at its end rather than at every pass
   * inside it — the pass over the rows measures, and so does the search pass that every redraw ends
   * with (`renderScrollTools`). Should neither come, the end of this task measures. `#226
   * page-energy-profile`.
   */
  function holdingReplyArrows(draw: () => void): void {
    replyArrows().held += 1;
    try {
      return draw();
    } finally {
      replyArrows().held -= 1;
      if (replyArrows().held === 0 && replyArrows().owed) {
        // In the column whose rows were drawn: the end of this task is in the left one. `#229`.
        const measure = deps.inThisColumn(() => {
          if (replyArrows().held === 0 && replyArrows().owed && !document.hidden) measureReplyArrows();
        });
        Promise.resolve().then(measure);
      }
    }
  }
  return {
    replyJumpTicket, replyArrows, dropReplyArrowTap, replyArrow, rowHolding, listPlaceTop,
    floatingClearance, drawnTop, rowOnScreen, revealRow, putOutLanding, replyReturnRow, backToReply,
    landOnAnswered, answeredElsewhere, historyReadToStart, jumpToAnswered, seekMessage,
    measureReplyArrows, holdingReplyArrows, rootFontPx,
  };
}


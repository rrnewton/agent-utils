// What a finger or the mouse does on a channel row (`#624 voice-page-modularity`, moved from the legacy
// script into view/channel): a tap folds the row, or in reading mode reads it aloud or jumps the reading
// to the block tapped; a hold opens its details; a swipe archives it, or in a thread a swipe to the right
// goes back. One act per finger: a hold or a swipe that acted claims the click behind it, which the row's
// own tap, and any other control that honours the claim, then lets go. What each act does is the page's.

import { childByClass, nodeWithin } from "../dom/index.ts";

/** How far a finger moves before it is a swipe or a scroll, not a tap, in CSS pixels. */
const SWIPE_START_PX = 12;
/** How far a swipe goes before letting go acts on the row, in CSS pixels. */
const SWIPE_COMMIT_PX = 90;
/** How long a finger held still on a row opens its details, in milliseconds. */
const HOLD_MS = 450;

/** A drawn block a tap may land in, as the reading's prepared blocks say it. */
export interface TappedBlock {
  readonly element: Node;
}

/** Where in a row's body a tap landed: the part (message) and, where the reading drew one, the block. */
export interface Tapped {
  readonly part: number;
  readonly block: number | null;
}

/** The reading on now, as far as a tap reads it. */
export interface PlayingReading {
  readonly id: string | null;
  readonly segments?: readonly { readonly part?: unknown; readonly block?: unknown }[] | null;
  readonly segment?: number;
}

/** A row as the gestures read it: the element, and the controls it keeps on itself. */
export type GestureRow = HTMLElement & {
  bookmarkLine?: Node | null;
  replyMarker?: Node | null;
  threadStrip?: Node | null;
  seamLine?: Node | null;
  seamBelowLine?: Node | null;
};

/** What the gestures ask of the page. */
export interface RowGesturesDependencies {
  /** `fn` made to run in the column the row was drawn in, as every row's own listener is. */
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  /** Whether the row is showing its links (`#211 link-filter`), whose taps are theirs. */
  showingLinks(row: Element): boolean;
  /** The reading's prepared blocks of part `part` of the row, or null where it has none. */
  preparedBlocksOf(row: Element, part: number): readonly TappedBlock[] | null | undefined;
  /** Whether reading mode is on, where a tap reads a row aloud rather than folding it. */
  readingMode(): boolean;
  /** The reading playing now, or null. */
  nowPlaying(): PlayingReading | null;
  /** Jump the reading playing now to segment `index`. */
  jumpReading(index: number): void;
  /** Read the messages `ids` aloud. */
  readAloud(ids: string[]): void;
  /** The ids of the messages the row stands for. */
  idsOf(row: Element): string[];
  /** A hold: open or close the row's details. */
  toggleDetails(row: GestureRow, messages: readonly unknown[]): void;
  /** A swipe that went far enough, `dx` pixels across: act on the row, the messages `ids`. */
  swiped(row: GestureRow, ids: string[], dx: number): void;
}

/** The gestures on a row, and the one claim a gesture puts on the click behind it. */
export interface RowGestures {
  /** A tap on `row` acts on it (`act`, by default `actOnRow`), unless something else had it. */
  tapRow<F extends HTMLElement | null>(row: GestureRow, fold: F, act?: (row: GestureRow, fold: F, tapped: Tapped | null) => void): void;
  /** Where in `row`'s body `target` is: its part, and the reading's block there, or null outside the body. */
  blockTapped(row: Element, target: Node): Tapped | null;
  /** What a tap on `row` does: read it aloud, or jump the reading, in reading mode; fold it otherwise. */
  actOnRow(row: GestureRow, fold: HTMLElement | null, tapped?: Tapped | null): void;
  /** Fold or unfold `row` by its fold, unless it is showing its links. */
  foldOnTap(row: Element, fold: HTMLElement | null): void;
  /** A finger's hold and swipe on `row`, which stands for `messages`. */
  swipeable(row: GestureRow, messages: readonly { readonly id: unknown }[]): void;
  /** A gesture acted: the click behind it is claimed. */
  claimClick(): void;
  /** Whether the click now is one a gesture claimed, the claim then let go. */
  takeClaimedClick(): boolean;
}

/** The gestures, with no click claimed. */
export function createRowGestures(deps: RowGesturesDependencies): RowGestures {
  /**
   * A gesture that ENDED in an act must not also be read as a tap.
   *
   * A swipe and a press-and-hold both finish with a `pointerup`, and a browser follows that with a
   * `click` — which the row also listens for, to fold. One flag rather than one per row: only one
   * gesture is in flight at a time, and a per-row flag would have to be cleaned up on a list the
   * poll rebuilds underneath it.
   */
  let suppressNextRowClick = false;

  /**
   * `#376 read-aloud-blocks`. Which block of which message in `li` the tap at `target` landed in, as
   * `{part, block}`; `block` is null where the message was not prepared in blocks, and the answer is
   * null outside every message's body.
   */
  const blockTapped = (li: Element, target: Node): Tapped | null => {
    const body = childByClass(li, "body");
    const boxes = body && body.children ? [...body.children] : [];
    const part = boxes.findIndex((box) => nodeWithin(box, target));
    if (part < 0) return null;
    const drawn = deps.preparedBlocksOf(li, part);
    let block: number | null = null;
    // The innermost: an item nested in another comes after it. `#486 read-aloud-pieces`: and a block
    // read in pieces from its first piece.
    (drawn || []).forEach((found, index) => {
      if (nodeWithin(found.element, target) && (block === null || (drawn as readonly TappedBlock[])[block]?.element !== found.element)) block = index;
    });
    return { part, block };
  };

  /**
   * The fold half of a tap on a row: the fold button pressed for it, so the tap and the button are one
   * act. Shared by the channel's rows (`actOnRow`) and the transcript's (`#431 voice-transcript-expand`).
   * Not while the row shows its links in place of its text (`#211 link-filter`): the text is not on
   * screen to be folded, and a fold changed out of sight would surprise the reader later.
   */
  const foldOnTap = (li: Element, fold: HTMLElement | null): void => {
    if (fold && !deps.showingLinks(li)) {
      fold.click();
    }
  };

  /**
   * What a tap that is meant for the row does to it. `#224 keyboard-shortcuts`: and Enter, or o, on the
   * selected message: the same act, so the key and the tap cannot become two opinions about a row.
   */
  const actOnRow = (li: GestureRow, fold: HTMLElement | null, tapped: Tapped | null = null): void => {
    // IN READING MODE A TAP IS A REQUEST TO HEAR IT, not to fold it. One gesture, two meanings,
    // decided by a mode the reader turned on deliberately and can see in the control bar.
    if (deps.readingMode()) {
      const playing = deps.nowPlaying();
      // `#376 read-aloud-blocks`. A tap on ANOTHER block of the message being read moves the read
      // there. A tap on the block being read stops it, as a tap on the row always has.
      if (tapped && tapped.block !== null && playing !== null && playing.segments &&
          playing.id === li.getAttribute("data-id")) {
        const index = playing.segments.findIndex((segment) =>
          segment.part === tapped.part && segment.block === tapped.block);
        if (index >= 0 && index !== playing.segment) {
          deps.jumpReading(index);
          return;
        }
      }
      // EVERY message the row is showing, so a combined row is heard whole rather than half.
      deps.readAloud(deps.idsOf(li));
      return;
    }
    // A SHORT message has no fold control, and that is fine: it is already whole. The handler is
    // still attached to every row, because reading mode has to reach a short message too — gating
    // this on foldability made the shortest messages the only ones that could not be read aloud.
    foldOnTap(li, fold);
  };

  /**
   * What a tap on the message itself means.
   *
   * The owner's report: the fold control was a small target on a line already carrying four other
   * things, and the message beside it is enormous. So the message IS the control.
   *
   * It drives the fold BUTTON rather than the fold state, so there is exactly one path that folds a
   * row — the button keeps its scroll anchoring and its `aria-expanded`, and a keyboard user who
   * tabs to it gets the same act the tap performs.
   *
   * `act` is what the tap does once it is known to be meant for the row: a channel row's is
   * `actOnRow`, which reads aloud in reading mode; a transcript row's (`#431 voice-transcript-expand`)
   * is `foldOnTap` alone, since reading mode reads the CHANNEL's messages and a voice turn is not one.
   */
  const tapRow = <F extends HTMLElement | null>(li: GestureRow, fold: F,
    act: (row: GestureRow, fold: F, tapped: Tapped | null) => void = actOnRow): void => {
    // A NEW PRESS ENDS ANY OLD GESTURE'S CLAIM on the click. A swipe that commits, or a hold, sets
    // `suppressNextRowClick` for the click a browser sends after it, and a touch that ends in either
    // often sends none: the flag then waited for the next tap on any row, and swallowed it — since
    // `#431 voice-transcript-expand`, a first tap on a transcript card, after a swipe in the channel.
    // A gesture on this press sets it again after this, as its pointerdown starts it.
    li.addEventListener("pointerdown", () => {
      suppressNextRowClick = false;
    });
    li.addEventListener("click", deps.inThisColumn((event: Event) => {
      if (suppressNextRowClick) {
        suppressNextRowClick = false;
        return;
      }
      // Never steal a tap meant for a control, a link, or a selection the reader is making.
      const target = (event && event.target) as Element | null;
      if (target && typeof target.closest === "function" && target.closest("button, a, input")) {
        return;
      }
      // `#211 link-filter`. Nor one on the Links view's links, or between them: that is where the
      // reader opens a link, and a tap there that also folded the row or started reading it aloud
      // would be two acts for one touch. Walked by hand, as the reply arrow is, so it holds for any
      // element rather than only for one that has `closest`.
      const links = deps.showingLinks(li) ? childByClass(li, "row-links") : null;
      if (links && target && nodeWithin(links, target)) {
        return;
      }
      const selection = typeof getSelection === "function" ? getSelection() : null;
      if (selection && String(selection) !== "") {
        return;
      }
      act(li, fold, target ? blockTapped(li, target) : null);
    }));
  };

  const swipeable = (li: GestureRow, messages: readonly { readonly id: unknown }[]): void => {
    const ids = messages.map((m) => String(m.id));
    let startX = 0;
    let startY = 0;
    let dragging = false;
    let active = false;
    let holdTimer: ReturnType<typeof setTimeout> | null = null;
    const cancelHold = (): void => {
      if (holdTimer !== null) {
        clearTimeout(holdTimer);
        holdTimer = null;
      }
    };
    const reset = (): void => {
      cancelHold();
      li.style.transform = "";
      li.style.transition = "";
      dragging = false;
      active = false;
    };
    const within = (held: Node | null | undefined, target: EventTarget | null): boolean => Boolean(held) && nodeWithin(held, target);
    li.addEventListener("pointerdown", deps.inThisColumn((event: PointerEvent) => {
      // `#221 read-modes`. While the row is the line of a collapsed run it IS that line, and a finger
      // on it is pressing the line: neither archiving the message under it nor opening its details.
      if (li.getAttribute("data-read-run") === "head") {
        return;
      }
      if (!event || event.pointerType === "mouse") {
        return;
      }
      // `#204 reply-arrow`. A finger on a reply's arrow is pressing the arrow, not the row: the row
      // neither follows it sideways nor opens its details under it. A hold there would also leave
      // `suppressNextRowClick` set behind a click the arrow keeps to itself, and the next tap on any
      // row would be swallowed by it.
      if (within(childByClass(li, "reply-jump"), event.target)) return;
      // `#215 reply-coalesce`. Nor on the X beside the bridge of gathered replies, for the same reason.
      if (within(childByClass(li, "reply-ungather"), event.target)) return;
      // `#501 thread-bridge-gutter`. Nor on the bridge itself, whose tap glides back to what the
      // stack hangs from: a finger resting on the line, or sliding off it, is pressing that, not
      // archiving the reply or opening its details — one act per finger, as with the X.
      if (within(childByClass(li, "reply-bridge"), event.target)) return;
      // `#420 bookmark-clear`. Nor on the bookmark line's X; the line itself takes no taps.
      if (within(li.bookmarkLine, event.target)) return;
      // `#228 reply-visible`. Nor on the circle at the row's corner, the way down to a reply.
      if (within(li.replyMarker, event.target)) return;
      // `#299 all-thread-strip`. Nor on the strip at its left edge, which selects its thread: a finger
      // resting there, or sliding off it, neither archives the message nor opens its details.
      if (within(li.threadStrip, event.target)) return;
      // `#439 search-unmatched-separators`. Nor on a gap's line above or below the row, which shows
      // its run: a finger there neither archives this message nor opens its details, and a hold
      // there would leave `suppressNextRowClick` behind a click the line keeps to itself.
      if (within(li.seamLine, event.target) || within(li.seamBelowLine, event.target)) return;
      active = true;
      startX = event.clientX;
      startY = event.clientY;
      li.style.transition = "none";
      cancelHold();
      // PRESS AND HOLD, on the same pointer stream as the swipe so the two cannot both fire. The
      // row prints neither the author nor the message id any more; this is where they went.
      //
      // `#211 link-filter`. Not on a link in the Links view, where a long press is the phone's own:
      // it shows the address and offers to open or copy it, and the details sheet opening under that
      // as well would be two answers to one finger. A swipe that starts there is still a swipe.
      const links = deps.showingLinks(li) ? childByClass(li, "row-links") : null;
      if (links && nodeWithin(links, event.target)) {
        return;
      }
      // `#409 open-upstream-link`. Nor on the ⋯ menu's own link, for the same reason: a long press
      // there is how a phone copies its address, which is what the link replaced a Copy item for.
      if (within(childByClass(li, "row-open-link-button"), event.target)) return;
      holdTimer = setTimeout(() => {
        holdTimer = null;
        reset();
        suppressNextRowClick = true;
        deps.toggleDetails(li, messages);
      }, HOLD_MS);
    }));
    li.addEventListener("pointermove", deps.inThisColumn((event: PointerEvent) => {
      if (!active) {
        return;
      }
      const dx = event.clientX - startX;
      const dy = event.clientY - startY;
      // Any real travel means this is a swipe or a scroll, not a hold.
      if (Math.abs(dx) >= SWIPE_START_PX || Math.abs(dy) >= SWIPE_START_PX) {
        cancelHold();
      }
      // The axis is decided ONCE, on the first movement that is big enough to have a direction, and
      // then held. Re-deciding per event turns a diagonal flick into a row that judders sideways
      // while the list scrolls under it.
      if (!dragging) {
        if (Math.abs(dx) < SWIPE_START_PX && Math.abs(dy) < SWIPE_START_PX) {
          return;
        }
        if (Math.abs(dy) >= Math.abs(dx)) {
          active = false; // a scroll, not a swipe. Leave it entirely alone.
          return;
        }
        dragging = true;
      }
      li.style.transform = `translateX(${dx}px)`;
    }));
    const finish = (event: PointerEvent): void => {
      if (!active) {
        return;
      }
      const dx = dragging ? event.clientX - startX : 0;
      li.style.transition = "";
      li.style.transform = "";
      const committed = Math.abs(dx) >= SWIPE_COMMIT_PX;
      reset();
      if (committed) {
        suppressNextRowClick = true;
      }
      if (committed) deps.swiped(li, ids, dx);
    };
    li.addEventListener("pointerup", deps.inThisColumn(finish));
    li.addEventListener("pointercancel", deps.inThisColumn(reset));
  };

  return {
    tapRow, blockTapped, actOnRow, foldOnTap, swipeable,
    claimClick() {
      suppressNextRowClick = true;
    },
    takeClaimedClick() {
      if (!suppressNextRowClick) return false;
      suppressNextRowClick = false;
      return true;
    },
  };
}

// What the page and the tests use of it.
export { HOLD_MS, SWIPE_COMMIT_PX, SWIPE_START_PX };

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
  /** Whether the row is showing its links (`#212 link-filter`), whose taps are theirs. */
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
  let suppressNextRowClick = false;

  const blockTapped = (li: Element, target: Node): Tapped | null => {
    const body = childByClass(li, "body");
    const boxes = body && body.children ? [...body.children] : [];
    const part = boxes.findIndex((box) => nodeWithin(box, target));
    if (part < 0) return null;
    const drawn = deps.preparedBlocksOf(li, part);
    let block: number | null = null;
    (drawn || []).forEach((found, index) => {
      if (nodeWithin(found.element, target) && (block === null || (drawn as readonly TappedBlock[])[block]?.element !== found.element)) block = index;
    });
    return { part, block };
  };

  const foldOnTap = (li: Element, fold: HTMLElement | null): void => {
    if (fold && !deps.showingLinks(li)) {
      fold.click();
    }
  };

  const actOnRow = (li: GestureRow, fold: HTMLElement | null, tapped: Tapped | null = null): void => {
    if (deps.readingMode()) {
      const playing = deps.nowPlaying();
      if (tapped && tapped.block !== null && playing !== null && playing.segments &&
          playing.id === li.getAttribute("data-id")) {
        const index = playing.segments.findIndex((segment) =>
          segment.part === tapped.part && segment.block === tapped.block);
        if (index >= 0 && index !== playing.segment) {
          deps.jumpReading(index);
          return;
        }
      }
      deps.readAloud(deps.idsOf(li));
      return;
    }
    foldOnTap(li, fold);
  };

  const tapRow = <F extends HTMLElement | null>(li: GestureRow, fold: F,
    act: (row: GestureRow, fold: F, tapped: Tapped | null) => void = actOnRow): void => {
    li.addEventListener("pointerdown", () => {
      suppressNextRowClick = false;
    });
    li.addEventListener("click", deps.inThisColumn((event: Event) => {
      if (suppressNextRowClick) {
        suppressNextRowClick = false;
        return;
      }
      const target = (event && event.target) as Element | null;
      if (target && typeof target.closest === "function" && target.closest("button, a, input")) {
        return;
      }
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
      if (li.getAttribute("data-read-run") === "head") {
        return;
      }
      if (!event || event.pointerType === "mouse") {
        return;
      }
      if (within(childByClass(li, "reply-jump"), event.target)) return;
      if (within(childByClass(li, "reply-ungather"), event.target)) return;
      if (within(childByClass(li, "reply-bridge"), event.target)) return;
      if (within(li.bookmarkLine, event.target)) return;
      if (within(li.replyMarker, event.target)) return;
      if (within(li.threadStrip, event.target)) return;
      if (within(li.seamLine, event.target) || within(li.seamBelowLine, event.target)) return;
      active = true;
      startX = event.clientX;
      startY = event.clientY;
      li.style.transition = "none";
      cancelHold();
      const links = deps.showingLinks(li) ? childByClass(li, "row-links") : null;
      if (links && nodeWithin(links, event.target)) {
        return;
      }
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
      if (Math.abs(dx) >= SWIPE_START_PX || Math.abs(dy) >= SWIPE_START_PX) {
        cancelHold();
      }
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

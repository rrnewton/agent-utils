// The ways back to a place in the list (`#240 reading-bookmark`, the jump to where the reader was reading, and back
// to a reply; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/channel for the plan's
// step 17). What each does is the page's; mounted once, in the page's order, and taken down again.

import { createListening, nodeWithin } from "../dom/index.ts";

/** How long a press on the Bookmark chip is held before it offers Clear, in milliseconds (the row hold's). */
const BOOKMARK_HOLD_MS = 450;
/** How far a press may travel before it is a scroll or swipe, not a hold, in CSS pixels. */
const BOOKMARK_HOLD_SLOP_PX = 12;

export interface ReadingJumpsDependencies {
  /** An element of the page by id, and one of the left column's (where the reading chip is, as b is). */
  lookup(id: string): HTMLElement;
  leftLookup(id: string): HTMLElement;
  /** To the bookmark, to where the reader was reading, and back to the reply left. */
  jumpToBookmark(): unknown;
  jumpToUnread(): unknown;
  jumpToReading(): unknown;
  backToReply(): unknown;
  /** Take the bookmark away through the existing clear path (`#420 bookmark-clear`), Undo included. */
  clearBookmark(): unknown;
}

/** The listeners, mounted. */
export interface ReadingJumps {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the ways back in the page's order. */
export function mountReadingJumps(deps: ReadingJumpsDependencies): ReadingJumps {
  const { listen, unmount } = createListening();
  const chip = deps.lookup("jump-bookmark");
  const popover = deps.lookup("bookmark-clear-popover");
  const clearButton = deps.lookup("bookmark-clear-button");
  let holdTimer: ReturnType<typeof setTimeout> | null = null;
  let startX = 0;
  let startY = 0;
  let suppressClick = false;
  let open = false;

  const cancelHold = (): void => {
    if (holdTimer !== null) {
      clearTimeout(holdTimer);
      holdTimer = null;
    }
  };

  // Fixed against the viewport, above the chip so neither the chip stack nor the dock can cover it
  // and it never covers the chip; clamped 8px inside the viewport, flipping below only where above
  // cannot fit. Best effort where a host has no layout boxes (the page suite's fake DOM).
  const place = (): void => {
    try {
      if (typeof chip.getBoundingClientRect !== "function" || typeof popover.getBoundingClientRect !== "function") return;
      const chipBox = chip.getBoundingClientRect();
      const popBox = popover.getBoundingClientRect();
      if (!chipBox.width || !popBox.width) return;
      const viewWidth = document.documentElement?.clientWidth || chipBox.right;
      const viewHeight = window.innerHeight || chipBox.bottom;
      const gap = 8;
      const width = popBox.width;
      const height = popBox.height || 52;
      const left = Math.min(Math.max(chipBox.left, gap), Math.max(gap, viewWidth - width - gap));
      let top = chipBox.top - height - gap;
      if (top < gap) top = Math.min(chipBox.bottom + gap, Math.max(gap, viewHeight - height - gap));
      popover.style.left = `${Math.round(left)}px`;
      popover.style.top = `${Math.round(top)}px`;
    } catch {
      // A host that cannot measure keeps the stylesheet's placement.
    }
  };

  // Idempotent: Android fires contextmenu during a long press, so the hold timer and the
  // contextmenu path can both arrive for one press. The second arrival must not flash,
  // close-and-reopen, or toggle the popover shut.
  const openPopover = (): void => {
    if (open) return;
    open = true;
    popover.hidden = false;
    chip.setAttribute("aria-expanded", "true");
    place();
    if (typeof clearButton.focus === "function") clearButton.focus();
  };

  const closePopover = (restoreFocus = false): void => {
    if (!open) return;
    open = false;
    popover.hidden = true;
    chip.setAttribute("aria-expanded", "false");
    popover.style.left = "";
    popover.style.top = "";
    if (restoreFocus && typeof chip.focus === "function") chip.focus();
  };

  listen<Event>(chip, "click", () => {
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    if (open) {
      closePopover();
      return;
    }
    deps.jumpToBookmark();
  });
  listen<Event>(deps.lookup("jump-unread"), "click", () => deps.jumpToUnread());
  listen<Event>(deps.leftLookup("jump-reading"), "click", () => deps.jumpToReading());
  listen<Event>(deps.lookup("back-to-reply"), "click", () => deps.backToReply());

  listen<PointerEvent>(chip, "pointerdown", (event) => {
    // A new press ends any old hold's claim on the click behind it.
    suppressClick = false;
    cancelHold();
    if (!event || (typeof event.button === "number" && event.button > 0)) return;
    startX = event.clientX || 0;
    startY = event.clientY || 0;
    holdTimer = setTimeout(() => {
      holdTimer = null;
      suppressClick = true;
      openPopover();
    }, BOOKMARK_HOLD_MS);
  });
  listen<PointerEvent>(chip, "pointermove", (event) => {
    if (holdTimer === null || !event) return;
    if (Math.abs((event.clientX || 0) - startX) >= BOOKMARK_HOLD_SLOP_PX ||
        Math.abs((event.clientY || 0) - startY) >= BOOKMARK_HOLD_SLOP_PX) cancelHold();
  });
  const endPress = (): void => cancelHold();
  listen<PointerEvent>(chip, "pointerup", endPress);
  listen<PointerEvent>(chip, "pointercancel", endPress);
  listen<Event>(window, "scroll", () => cancelHold(), { capture: true });
  listen<MouseEvent>(chip, "contextmenu", (event) => {
    if (event && typeof event.preventDefault === "function") event.preventDefault();
    // The click an Android hold sends after its contextmenu must not also jump.
    suppressClick = true;
    cancelHold();
    openPopover();
  });
  listen<MouseEvent>(clearButton, "click", () => {
    closePopover();
    deps.clearBookmark();
  });
  listen<KeyboardEvent>(popover, "keydown", (event) => {
    if (event && event.key === "Escape") {
      if (typeof event.stopPropagation === "function") event.stopPropagation();
      closePopover(true);
    }
  });
  listen<KeyboardEvent>(chip, "keydown", (event) => {
    if (event && event.key === "Escape" && open) {
      if (typeof event.stopPropagation === "function") event.stopPropagation();
      closePopover(true);
    }
  });
  listen<KeyboardEvent>(document, "keydown", (event) => {
    if (open && event && event.key === "Escape") closePopover(true);
  });
  listen<PointerEvent>(document, "pointerdown", (event) => {
    if (!open || !event) return;
    const target = event.target as Node | null;
    if (target && (nodeWithin(popover, target) || nodeWithin(chip, target))) return;
    closePopover();
  });

  // The chip going away (bookmark cleared, line back on screen, another view) takes its popover with it.
  let watcher: MutationObserver | null = null;
  if (typeof MutationObserver === "function") {
    watcher = new MutationObserver(() => {
      if ((chip as HTMLElement).hidden) closePopover();
    });
    watcher.observe(chip, { attributes: true, attributeFilter: ["hidden"] });
  }

  return {
    unmount() {
      cancelHold();
      watcher?.disconnect();
      unmount();
    },
  };
}

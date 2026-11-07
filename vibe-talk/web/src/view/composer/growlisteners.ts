// The message boxes' grow switches and their measuring (`#340 composer-grow-box`; `#624 voice-page-modularity`, moved
// from the legacy script's wiring into view/composer for the plan's step 17): the switches drawn as kept and heard,
// three boxes fitted as they are typed in, and Send kept in view as a box grows. How a box grows is
// `createComposerGrow`'s; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";
import { COMPOSER_GROW_BOXES, COMPOSER_GROW_IDS } from "./grow.ts";

/** The boxes fitted as they are typed in. The call's box is measured as it saves its draft instead. */
const TYPED_GROW_BOXES = ["channel-compose-text", "side-compose-text", "reply-text"] as const;

export interface ComposerGrowListenersDependencies {
  /** An element of the left column by id, where the boxes and their switches are. */
  lookup(id: string): HTMLElement;
  /** Whether the boxes grow, as kept (absent means off), and the switches drawn so. */
  storedComposerGrow(): boolean;
  renderComposerGrow(on: boolean): unknown;
  /** A switch ticked or cleared, from the checkbox the change came from. */
  applyComposerGrow(on: boolean, grow: HTMLElement): unknown;
  /** A box fitted to what is written in it, and kept in view as it grows. */
  fitGrowingBox(field: HTMLElement): unknown;
  keepGrowingBoxInView(field: HTMLElement): unknown;
  /** The browser's ResizeObserver, where it has one. */
  resizeObserver(): typeof ResizeObserver | undefined;
}

/** The listeners and the observer, mounted. */
export interface ComposerGrowListeners {
  /** Take every listener down again, and stop observing. */
  unmount(): void;
}

/** Draw the grow switches as kept, and mount their listeners and the boxes' measuring. */
export function mountComposerGrowListeners(deps: ComposerGrowListenersDependencies): ComposerGrowListeners {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  // Drawn as stored, and stored only once the reader ticks one: absent means off.
  deps.renderComposerGrow(deps.storedComposerGrow());
  for (const id of COMPOSER_GROW_IDS) {
    // The checkbox the change came from, not the focus, which a click in Safari or Firefox on a Mac leaves on nothing.
    listen<Event>(el(id), "change", () => deps.applyComposerGrow((el(id) as HTMLInputElement).checked, el(id)));
  }
  // The call's box is measured as it saves its draft (`rememberComposeDraft`); these three, as they are typed in.
  for (const id of TYPED_GROW_BOXES) listen<Event>(el(id), "input", () => deps.fitGrowingBox(el(id)));
  // Send kept in view as the box grows under the reader's typing. In the observer, not after it: a scroll changes no
  // size, so it cannot come round again as the loop a browser reports, and the list is where it will be before the
  // jump to the newest is decided again after the observer (`#207 scrollback-jump`). In the right column a reader at
  // the foot is kept there already (`#280 scroll-stability`), and this then finds the composer in view and moves
  // nothing.
  const Observer = deps.resizeObserver();
  let growing: ResizeObserver | null = null;
  if (typeof Observer === "function") {
    growing = new Observer((entries) => {
      for (const entry of entries) deps.keepGrowingBoxInView(entry.target as HTMLElement);
    });
    for (const id of COMPOSER_GROW_BOXES) growing.observe(el(id));
  }
  return {
    unmount() {
      unmount();
      if (growing) growing.disconnect();
    },
  };
}

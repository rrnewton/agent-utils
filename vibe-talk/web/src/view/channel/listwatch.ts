// Watching the lists move (`#207 scrollback-jump`, `#245 done-feedback`, `#204 reply-arrow`, `#49 cached-summaries`,
// `#340 composer-grow-box`, `#261 two-column-connectors`, `#229 desktop-two-column`, `#338 column-overscroll`;
// `#624 voice-page-modularity`, moved from the legacy script's wiring into view/channel for the plan's step 17): what
// a scroll of Main's list asks for, and what a change of size of either list, or of anything directly in it, asks for
// once the browser has finished telling of it. What each asks for is the page's; mounted once, in the page's order,
// and taken down again.

import { createListening } from "../dom/index.ts";

/** A ResizeObserver, as far as watching the lists uses one. */
export interface SizeWatcher {
  observe(target: Element): void;
  disconnect(): void;
}

export interface ListWatchDependencies {
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /** A ResizeObserver calling `report`, or null where the browser has none. */
  sizeWatcher(report: () => void): SizeWatcher | null;
  /** Run `act` once the current task is done (`setTimeout(act, 0)`). */
  afterThisTask(act: () => void): void;
  /** Keep the chips off the message box (`#245 done-feedback`). */
  placeChipsOverComposer(): void;
  /** Offer the jump to the newest line, or take it away. */
  renderJumpNewest(): void;
  /** Whether a way back to a reply is offered, the row it goes back to, and taking the offer away (`#204`). */
  replyBackOffered(): boolean;
  replyReturnRow(): Element | null;
  dropReplyBack(): void;
  /** Whether `row` is on screen. */
  rowOnScreen(row: Element): boolean;
  /** Draw the chips over the list's foot again. */
  renderScrollTools(): void;
  /** Load what is above the channel's list, and above the call's transcript, where each is on screen at its top. */
  maybeLoadOlder(): void;
  maybeLoadOlderTurns(): void;
  /** Ask for the summaries of the rows now showing (`#49 cached-summaries`). */
  requestVisibleSummaries(): void;
  /** Measure the growing message boxes where the browser does not size them (`#340`). */
  fitGrowingBoxes(): void;
  /** Draw the links between the columns again, once per frame (`#261`). */
  scheduleColumnLinks(): void;
  /** Fit the room past the foot of `area` to its rows (`#338`). */
  fitOverscroll(area: HTMLElement): void;
  /** Whether the right column is showing, and offering its own jump to its newest line. */
  sideShowing(): boolean;
  renderSideJumpNewest(): void;
}

/** The watchers, mounted. */
export interface ListWatch {
  /** Take the scroll listener down and stop both size watchers. */
  unmount(): void;
}

/** Mount the watchers of Main's list and of the right column's. */
export function mountListWatch(deps: ListWatchDependencies): ListWatch {
  const { listen, unmount: unlisten } = createListening();
  const watchers: SizeWatcher[] = [];
  // The chip is an offer to go somewhere the reader may simply go themselves. Once they are there it
  // has nothing left to say, so it takes itself away rather than waiting to be tapped — and since
  // `#207 scrollback-jump` it is offered the moment they scroll far enough up the history to want it.
  // Only the jump, not every chip: this runs on every scroll event. See `renderJumpNewest`.
  listen(deps.byId("scroll-area"), "scroll", () => {
    // `#245 done-feedback`. First, so the chips keep off the message box in the frame it moved in.
    deps.placeChipsOverComposer();
    deps.renderJumpNewest();
    // `#204 reply-arrow`. The way back to a reply goes once the reply is on screen again, however
    // the reader brought it there: a chip offering to go where they already are is clutter.
    if (deps.replyBackOffered()) {
      const reply = deps.replyReturnRow();
      if (reply === null || deps.rowOnScreen(reply)) {
        deps.dropReplyBack();
        deps.renderScrollTools();
      }
    }
    // ...and the other end of the same list: arriving at the top is a request for what is above it.
    // Both lists, because both of them now have something above them: the channel has older
    // messages and the transcript has an older record. Each declines unless its own view is the one
    // on screen, so exactly one of them can act on any given scroll.
    deps.maybeLoadOlder();
    deps.maybeLoadOlderTurns();
    // `#49 cached-summaries`. Summaries are produced as the reader scrolls, so this is where they
    // are asked for. It is cheap on the ordinary event: the summaries owner's `summariesAsked` answers
    // for every row that has already been asked about, and a scroll that reveals nothing new issues
    // nothing.
    deps.requestVisibleSummaries();
  });
  // `#207 scrollback-jump`. How far the list runs below the reader also changes WITHOUT a scroll, and
  // the listener above never hears of it: the window resized or turned, the reading column dragged
  // narrower, a row or the chips' room growing or shrinking below the reader. A reader 40px
  // up whose newest row then shrank by 30 is on the newest line with a jump offering to take them
  // there; one on the newest line whose last row grew is up the history with no way back offered.
  // So the list's box and everything directly in it — the two panes, the search's empty notice — are
  // watched too, and the jump decided again from where the reader now is.
  //
  // AFTER the observer returns, not inside it: deciding can show or hide the jump, that resizes
  // #scroll-tools, and a size changed from inside a ResizeObserver callback is the loop the browser
  // reports as an error. One decision per burst of resizes; it is cheap, but a row drawn is a resize.
  let decisionDue = false;
  const geometry = deps.sizeWatcher(() => {
    if (decisionDue) return;
    decisionDue = true;
    deps.afterThisTask(() => {
      decisionDue = false;
      deps.renderJumpNewest();
      // `#245 done-feedback`. The message box moves without a scroll too: the window, a view or
      // screen changing, the box itself resized.
      deps.placeChipsOverComposer();
      // `#340 composer-grow-box`. A box shown again, by a view, a screen or a send card going, is
      // measured where the browser does not size it.
      deps.fitGrowingBoxes();
      // `#261 two-column-connectors`. A row folded or opened moves the roots below it with no scroll.
      deps.scheduleColumnLinks();
    });
  });
  if (geometry) {
    watchers.push(geometry);
    const area = deps.byId("scroll-area");
    for (const box of [area, ...Array.from(area.children)]) geometry.observe(box);
    // `#229 desktop-two-column`. The right column's list, for its own jump.
    let sideDue = false;
    const sideGeometry = deps.sizeWatcher(() => {
      if (sideDue) return;
      sideDue = true;
      deps.afterThisTask(() => {
        sideDue = false;
        // `#338 column-overscroll`. The room past the foot fitted to the rows as they now are, the cards' too.
        deps.fitOverscroll(deps.leftById("side-scroll"));
        if (deps.sideShowing()) deps.renderSideJumpNewest();
        deps.scheduleColumnLinks();
      });
    }) as SizeWatcher;
    watchers.push(sideGeometry);
    // Not the room past the foot itself, which only this page sizes: a size it is given while an observer
    // of its neighbours is being told of theirs is one the browser could not report in the same frame. Nor
    // the room held for a card (`#459 pack-keeps-card-still`), for the same reason.
    const side = deps.leftById("side-scroll");
    for (const box of [side, ...Array.from(side.children)]) {
      if (box !== deps.leftById("side-overscroll") && box !== deps.leftById("side-overextend")) sideGeometry.observe(box);
    }
  }
  return {
    unmount() {
      unlisten();
      for (const watcher of watchers.splice(0)) watcher.disconnect();
    },
  };
}

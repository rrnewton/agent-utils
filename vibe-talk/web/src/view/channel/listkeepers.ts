// The keepers of the lists' readers (`#218 desktop-dock`, `#229 desktop-two-column`, `#269 approval-card-inline`,
// `#281 view-keys`, `#280 scroll-stability`, `#338 column-overscroll`, `#459 pack-keeps-card-still`, `#214 reply-arrow`;
// `#624 voice-page-modularity`, moved from the legacy script's wiring into view/channel for the plan's step 17): a
// reader on the newest line kept on it as the dock grows under Main's list or the right column's, the right
// column's reader kept on its newest line as what is in it changes size, and the reply arrows measured again as the
// lists' layout changes. The page's acts, owners and the slack come in, under the page's own names; mounted once,
// in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";
import type { SizeWatcher, SizeWatching } from "../dom/index.ts";
import type { UnderListNote, UnderLists } from "./underlists.ts";

/** What the right column's keeper noted of its reader at its last pass or scroll. */
interface Footing {
  readonly foot: boolean;
  readonly width: number;
  readonly height: number;
  readonly context: string;
}

export interface ListKeepersDependencies<Column> extends SizeWatching {
  /** An element of the page by id, in the column the code is working in, and in the left column. */
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  /** The screen and the view up. */
  readonly screens: { screen(): string; view(): string };
  /** What the keepers noted at their last reports (`createUnderLists`). */
  readonly underLists: UnderLists;
  /** Whether the cards are being drawn as a thread opens. */
  readonly cardsDrawn: { opening(): boolean };
  /** The two columns. */
  readonly columns: { readonly main: Column; readonly side: Column & { readonly mode: string } };
  /** Whether two columns are drawn. */
  readonly layoutShown: { columns(): boolean };
  /** A reader within this many px of a list's foot is on its newest line. */
  readonly bottomSlackPx: number;
  /** How much stands under the main screen's lists: the dock, and the send card docked under the thread list. */
  heightUnderLists(): number;
  /** Whether a quick reply holds `area` still, and placing it again (`#727 vt-selection-quick-reply`). */
  quickReplyHolds(area: HTMLElement): boolean;
  placeQuickReply(): void;
  /** Whether a thread's landing still holds, where it is, and placing it again (`#221 read-modes`). */
  entryLandingHolds(): boolean;
  entryState(): { readonly landing: { readonly row: HTMLElement | null; readonly first: HTMLElement | null } };
  placeEntryLanding(row: HTMLElement | null, first: HTMLElement | null): void;
  /** Whether a filter change holds a message in place, and the row it holds (`#243 filter-keeps-selection`). */
  keepFilterHold(): boolean;
  heldFilterRow(): HTMLElement | null;
  /** Whether a walk for the search holds the step back still, in the column the code is working in (`#262`). */
  searchWalkAnchored(): boolean;
  /** The call: whether the reader is writing to the agent, and keeping its box in view (`#241 voice-composer`). */
  voiceCall(): { writingToAgent(): boolean; keepComposerInView(): void };
  /** Keep a box being written in `area` in view as the keyboard shrinks it; whether it did (`#429`). */
  keepWrittenBox(area: HTMLElement, seen: UnderListNote | null, now: UnderListNote): boolean;
  /** Whether a box in `area` is being written in (`boxBeingWritten`): its reader focused there. */
  writingIn(area: HTMLElement): boolean;
  /** The scroll position of the rows' foot of `area` (`footTop`), where the page measures it. */
  footTop?(area: HTMLElement): number;
  /** Whether `area` is within `slack` px of its foot (the page's slack by default), and taking it to its foot. */
  atBottom(area: HTMLElement, slack?: number): boolean;
  followFoot(area: HTMLElement): void;
  /** Whether the right column is drawn, and showing. */
  sideColumnUp(): boolean;
  sideShowing(): boolean;
  /** Run `act` in `column`. */
  inColumn<T>(column: Column, act: () => T): T;
  /** The room held for a card fitted to the rows, and a reader in the room past the foot held (`#459`, `#338`). */
  fitOverextend(area: HTMLElement): void;
  holdOverscrollReader(area: HTMLElement): void;
  /** Which channel's list the column shows, and whether only the pinned are. */
  channelContextKey(): string;
  pinsView(): { readonly only: boolean };
  /** Measure the reply arrows of the column the code is working in again. */
  measureReplyArrows(): void;
}

/** The keepers, mounted. */
export interface ListKeepers {
  /** Take the right column's scroll listener down and stop every watcher. */
  unmount(): void;
}

/** Mount the keepers of the lists' readers. */
export function mountListKeepers<Column>(deps: ListKeepersDependencies<Column>): ListKeepers {
  const { listen, unmount: unlisten } = createListening();
  const watchers: SizeWatcher[] = [];
  const watch = (report: () => void): SizeWatcher => {
    const watcher = deps.sizeWatcher(report);
    watchers.push(watcher);
    return watcher;
  };
  const {
    el, leftEl, screens, underLists, cardsDrawn, columns, layoutShown, heightUnderLists, quickReplyHolds, placeQuickReply,
    entryLandingHolds, entryState, placeEntryLanding, keepFilterHold, heldFilterRow, searchWalkAnchored, voiceCall,
    keepWrittenBox, writingIn, footTop, atBottom, followFoot, sideColumnUp, sideShowing, fitOverextend, holdOverscrollReader, channelContextKey,
    pinsView, measureReplyArrows,
  } = deps;
  const inColumn = <T>(column: Column, act: () => T): T => deps.inColumn(column, act);
  const BOTTOM_SLACK_PX = deps.bottomSlackPx;
  // `#218 desktop-dock`. On a desk the dock is one row of controls or two, and which depends on what
  // is in it — a call going live, a long channel name, a label changing length. When it grows it takes
  // that height from the bottom of #scroll-area, and a reader on the newest line is left that far above
  // it: a second row is 42px, past BOTTOM_SLACK_PX, so the next turn of the call they just started was
  // no longer followed. On a phone the dock is one height on the main screen and this never acts.
  //
  // So a reader who was on the newest line BEFORE the dock grew is kept on it: within the slack of it
  // once the growth is given back. Anyone further up is left exactly where they are — the list's top
  // edge did not move, and neither does what they are reading. A view switch that grows the dock puts
  // the other list back in its own place first; a place within the slack of its newest line, plus the
  // growth, is taken the rest of the way, and anywhere further up is left alone.
  //
  // THE LIST is what is watched, not the dock, because the list is what changes size whenever this can
  // matter — and also whenever the main screen comes and goes, which is the case this must stay out of:
  // the list is hidden on every other screen, so coming back is a list that was nothing a moment ago,
  // and the dock growing then (the pane returning) is not a reader losing their place. A window that
  // shrinks is not the dock growing either, and it leaves the reader where they are, as `#207`
  // decided. What is kept between reports is the list's height and the dock's.
  //
  // `#269 approval-card-inline` x `#281 view-keys`. The send card docked under the thread list counts
  // as the dock (`heightUnderLists`): it takes its height from the foot of the cards, and of the left
  // column's list behind them, exactly as a second row of the dock does, so a reader at either foot is
  // kept on it as the card comes, and as it grows, and More opens it. Not as it MOVES there from the foot
  // of Main's list, or back (`cardMovedUnderLists`): that list loses the card's height as the card takes
  // it from under the list, and a reader up it is exactly where they were. And as it COMES, the reader
  // at Main's foot is the one who was within the slack of it before (`cardCameUnderLists`): the box at
  // the list's foot shrinks to one line in the same layout, which leaves a reader up the list nearer the
  // foot than the slack plus the growth, and they are not pulled down to it.
  if (deps.canWatchSizes()) {
    watch(() => {
      const area = el("scroll-area");
      const now = { list: area.clientHeight, dock: heightUnderLists() };
      const seen = underLists.main();
      const grew = seen !== null && seen.list > 0 && screens.screen() === "main" ? now.dock - seen.dock : 0;
      const shrank = seen !== null && seen.list > 0 && now.list < seen.list;
      underLists.noteMain(now);
      // `#727 vt-selection-quick-reply`. A list held still for a quick reply is left where it is, as a
      // phone's keyboard comes up for the box and goes again.
      if (quickReplyHolds(area)) {
        placeQuickReply();
        return;
      }
      // `#221 read-modes`. Except a reader just landed on a thread's first unread who has not moved
      // since: on a desk the entry itself wrapped the dock, by putting the thread's name in the picker,
      // and the message is kept at the head however near the end of the list it is.
      if (entryLandingHolds()) {
        placeEntryLanding(entryState().landing.row, entryState().landing.first);
        return;
      }
      // `#243 filter-keeps-selection`. Nor a message a filter change holds in place, which stays put.
      if (keepFilterHold()) return;
      // `#241 voice-composer`. The list shrinking under a reader who is writing in the call's box —
      // the on-screen keyboard coming up, as the viewport asks the browser to lay it out — keeps the
      // box in view rather than letting the list's foot, where the box is, slide under the dock.
      if (shrank && voiceCall().writingToAgent()) {
        voiceCall().keepComposerInView();
        return;
      }
      // `#429 keyboard-hides-bar`: the channel's box and a reply in the stream as well, where they were in view.
      if (keepWrittenBox(area, seen, now)) return;
      if (grew > 0 && seen !== null && seen.gap !== undefined) {
        if (seen.gap <= BOTTOM_SLACK_PX) area.scrollTop = area.scrollHeight;
        return;
      }
      if (grew > 0 && atBottom(area, BOTTOM_SLACK_PX + grew)) {
        area.scrollTop = area.scrollHeight;
      }
    }).observe(el("scroll-area"));
    // `#229 desktop-two-column`. The same for the right column's reader, under the same dock.
    //
    // `#281 view-keys` x `#280 scroll-stability`. And for the cards, beside Main or across the window as the
    // thread list. There the dock is only as wide as the reading column, so at a desk it wraps where two
    // columns' dock spans the window and does not: on a reload in the thread list the cards were put at
    // their foot and the dock's last button came about 30 ms later, a second row that left the list 43px
    // above its foot for good, since nothing after the placement took it the rest of the way. This does,
    // whatever grew the dock and whenever: a reload, a view key, a channel's longer name. The growth is
    // reckoned from the last report, or from where the cards were last put back as the reader had them in
    // the thread list, in the column as it then stood, the card docked under it included
    // (`cardsPlacedUnderLists`).
    watch(() => {
      const area = leftEl("side-scroll");
      const now = { list: area.clientHeight, dock: heightUnderLists() };
      const seen = underLists.side();
      const grew = seen !== null && seen.list > 0 && screens.screen() === "main" ? now.dock - seen.dock : 0;
      // `#336 draft-return-thread-list`. Not a report from another screen, where the column is not drawn: back on
      // the main screen the growth is reckoned from the column as the reader left it, so a send card that docked
      // under the thread list while Settings or Help was up is the dock growing under them, as it is when it
      // comes while they are on the list. Taken from the hidden column instead, it was no growth at all, and a
      // reader who had left from the foot of the cards came back the card's height above it.
      if (screens.screen() === "main") underLists.noteSide(now);
      // `#429 keyboard-hides-bar`. A box being written in this list, kept in view as the keyboard shrinks it.
      if (screens.screen() === "main" && keepWrittenBox(area, seen, now)) return;
      if (sideColumnUp() && screens.view() === "discord" && columns.side.mode === "list") {
        if (!cardsDrawn.opening() && grew > 0 && atBottom(area, BOTTOM_SLACK_PX + grew)) followFoot(area);
        return;
      }
      if (!sideShowing()) return;
      inColumn(columns.side, () => {
        if (entryLandingHolds()) {
          placeEntryLanding(entryState().landing.row, entryState().landing.first);
          return;
        }
        if (keepFilterHold()) return;
        if (grew > 0 && atBottom(area, BOTTOM_SLACK_PX + grew)) followFoot(area);
      });
    }).observe(leftEl("side-scroll"));
  }
  // `#280 scroll-stability`. THE RIGHT COLUMN'S READER ON ITS NEWEST LINE STAYS ON IT while what is in
  // the list changes size under them with nothing scrolled. The cards are why: the one under the pointer
  // is drawn whole (`#257 summary-truncation`), which in Chromium left the list of threads 43px above its
  // foot the moment the mouse came to rest on its last card — "the thread view also feels like it is not
  // starting out scrolled all the way to the bottom" — and further from it than the slack, the reader had
  // stopped following the threads that moved to the foot. A thread on the right is kept the same way, and
  // so are the same cards across the window as the thread list (`#281 view-keys`).
  //
  // What the reader was doing is the list's state at its LAST SCROLL: a browser delivers scroll events
  // before it reports sizes in the same frame, so a scroll this page made — a landing, a jump, a place
  // kept across a redraw — has been noted by the time a size it changed in the same task is reported,
  // and a reader the page put somewhere else is not "on the newest line" here. Nothing else moves:
  //
  //   * A READER UP THE LIST is left exactly where they are. The browser's own scroll anchoring holds
  //     the card or row they are on through a change above it, and every redraw holds it itself
  //     (`keepingCardPlace`, `settleAfterRead`).
  //   * A LIST TOO SHORT TO SCROLL has its reader at its head as much as at its foot: growing past the
  //     screen, it opens downward under them.
  //   * NOT A RESIZE OF THE COLUMN, nor a change of what it shows — the cards or a thread, another thread,
  //     another screen — which have rules of their own (`#207`, `#218`); the state is noted afresh instead.
  //   * NOT A PLACE ANOTHER RULE HOLDS: a thread's landing on its first unread (`#221`), a message a
  //     filter change holds (`#243`), and the step back a walk for the search holds still (`#262`).
  //
  // Main is left as `#207 scrollback-jump` decided: every redraw and every read that changes its rows keeps
  // the reader itself (`preservingScroll`, `settleAfterRead`), and a newest message growing under a reader
  // on its newest line with nothing redrawn offers the jump to its end.
  if (deps.canWatchSizes()) {
    const area = leftEl("side-scroll");
    let footing: Footing | null = null;
    // What the column is showing: a change of it is not the content changing under the reader. Beside Main
    // or on its own as the thread list (`#281 view-keys`), which is a change of what it shows too.
    const contextOf = () => (sideColumnUp()
      ? `${layoutShown.columns() ? "two" : "list"}|${screens.screen()}|${screens.view()}|${columns.side.mode}|${inColumn(columns.side, () => `${channelContextKey()}|${pinsView().only}`)}`
      : "one");
    // On the newest line, and scrolled to it. What the column shows is noted only by this observer's
    // passes, which a change of it always brings: not on every scroll event.
    const note = (context = footing ? footing.context : ""): void => {
      footing = { foot: area.scrollTop > 0 && atBottom(area), width: area.clientWidth, height: area.clientHeight, context };
    };
    // Whether the reader may be kept on the newest line now: the right column on screen, and no other rule
    // holding a place in it.
    const free = () => {
      if (!sideColumnUp() || screens.screen() !== "main" || screens.view() !== "discord") return false;
      if (columns.side.mode === "list") return !cardsDrawn.opening();
      return inColumn(columns.side, () => {
        return !entryLandingHolds() && heldFilterRow() === null && !searchWalkAnchored();
      });
    };
    listen(area, "scroll", () => note(), { passive: true });
    const footKeeper = watch(() => {
      // `#459 pack-keeps-card-still`. The room held for a card fitted to the rows first: what came at the foot
      // takes its place, before anything that follows the foot measures it.
      fitOverextend(area);
      // `#338 column-overscroll`. A reader in the room past the foot, the last card changing size under them.
      holdOverscrollReader(area);
      const was = footing;
      const context = contextOf();
      if (was && was.foot && was.width === area.clientWidth && was.height === area.clientHeight &&
          was.context === context && !atBottom(area, 1) && free()) {
        followFoot(area);
      }
      note(context);
    });
    // Not the room past the foot (`#338 column-overscroll`), nor the room held for a card (`#459
    // pack-keeps-card-still`), which are the page's own and no content.
    for (const box of area.children) {
      if (box !== leftEl("side-overscroll") && box !== leftEl("side-overextend")) footKeeper.observe(box);
    }
  }
  // `#1144 scrollback-typing-jump`. MAIN'S READER AT ITS FOOT, WRITING, STAYS AT IT as what is in
  // the list changes size under them with nothing scrolled and nothing redrawn: a row at the foot
  // laid out again taller (an image, an embed, a streamed line settling) took the composer at the
  // list's foot down with it until the next key, and left its reader above the slack, no longer
  // following. The right column's keeper above is the shape, with one difference that is the
  // point: follow is ONE STATE, noted only from the reader's own scrolls (a touch, a press, a
  // wheel, as the keyboard's are) and from where the list opened — never from a scroll the
  // browser made revealing the caret in the box, nor from one this page made. Only while a box
  // in the list is being written in: a reader at the foot not writing is #207's, offered the jump.
  if (deps.canWatchSizes()) {
    const area = el("scroll-area");
    let footing: Footing | null = null;
    let readerScrolled = false;
    let typedAt = -Infinity;
    const contextOf = () => `${screens.screen()}|${screens.view()}|${channelContextKey()}|${pinsView().only}`;
    const note = (foot: boolean, context: string): void => {
      footing = { foot, width: area.clientWidth, height: area.clientHeight, context };
    };
    note(area.scrollTop > 0 && atBottom(area), contextOf());
    for (const type of ["touchstart", "pointerdown", "wheel"]) {
      listen(area, type, () => { readerScrolled = true; }, { capture: true, passive: true });
    }
    listen(area, "input", () => { typedAt = Date.now(); }, { passive: true });
    listen(area, "scroll", () => {
      // A scroll in the instant after text went in, with no reader's hand in it, is the browser
      // revealing the caret: it never turns follow on. The reader's own scrolls, and the page's
      // landings at the foot (a list opening, the Newest chip), note where the reader stands.
      const reveal = !readerScrolled && Date.now() - typedAt < 150;
      readerScrolled = false;
      if (reveal && footing && !footing.foot) return;
      note(area.scrollTop > 0 && atBottom(area), footing ? footing.context : contextOf());
    }, { passive: true });
    const free = () => screens.screen() === "main" && screens.view() === "discord" && writingIn(area)
      && !entryLandingHolds() && heldFilterRow() === null && !searchWalkAnchored() && !quickReplyHolds(area);
    const mainFootKeeper = watch(() => {
      const was = footing;
      const context = contextOf();
      if (was && was.foot && was.width === area.clientWidth && was.height === area.clientHeight
          && was.context === context && !atBottom(area, 1) && free()) {
        // To the rows' foot, not past it: room the keyboard keeps under the composer (a row
        // parked under the box, #1135) stays below the keys, where the reader left it.
        if (footTop) area.scrollTop = footTop(area);
        else followFoot(area);
      }
      if (was) note(was.foot, context);
    });
    for (const box of area.children) mainFootKeeper.observe(box);
  }
  // `#214 reply-arrow`. Where an arrow meets the row directly above it is layout, and layout changes
  // without a redraw: the window resized or turned, the reading column dragged, the reader's type
  // size, a row folding or opening above an arrow. So the channel's list is watched, and the arrows
  // measured again once the observer has returned, as the lists' watch decides the jump (`mountListWatch`).
  // Measuring only moves drawings that are out of the flow, so it cannot resize the list and come round again.
  if (deps.canWatchSizes()) {
    let measureDue = false;
    watch(() => {
      if (measureDue) return;
      measureDue = true;
      setTimeout(() => {
        measureDue = false;
        measureReplyArrows();
      }, 0);
    }).observe(el("discord-log"));
    // `#229 desktop-two-column`. And the right column's, in its column.
    let sideMeasureDue = false;
    watch(() => {
      if (sideMeasureDue) return;
      sideMeasureDue = true;
      setTimeout(() => {
        sideMeasureDue = false;
        if (sideShowing()) inColumn(columns.side, measureReplyArrows);
      }, 0);
    }).observe(leftEl("side-log"));
  }
  return {
    unmount() {
      unlisten();
      for (const watcher of watchers.splice(0)) watcher.disconnect();
    },
  };
}

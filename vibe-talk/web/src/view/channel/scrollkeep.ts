// Leaving the reader looking at the same thing across a change (`#47 scrollback-stability`, `#51
// reply-view`, `#206 pin-message`, `#207 scrollback-jump`, `#221 read-modes`, `#243
// filter-keeps-selection`, `#262 search-load-older`; `#624 voice-page-modularity`, moved from the legacy
// script into view/channel): the list on screen, the message the reader's eye is on, where they are
// captured as and put back by, and following a new turn only where they were already at the newest line.
// Explicit, rather than the browser's scroll anchoring, which a change above the viewport defeats. The
// filters' held row, the search walk's control, and the jump to the newest are the page's.

import { shownRowFor } from "./rows.ts";

/** Where the reader is: on the newest line, or which element their eye is on and how far down it sits. */
export interface ScrollMark {
  readonly pinned: boolean;
  readonly anchor: Element | null;
  readonly top: number;
}

/** A walk for the search whose control holds still while it reads, as far as keeping the reader reads it. */
interface AnchoredWalk {
  anchored: boolean;
  chipTop: number;
}

/** What keeping the reader asks of the page. */
export interface ScrollKeepingDependencies {
  /** The page's element by id (`el`), in the column being worked on. */
  el(id: string): HTMLElement;
  /** Whether the main screen is the one shown (`currentScreen === "main"`). */
  onMain(): boolean;
  /** The view the reader is on: "discord" for the channel, else the call's. */
  currentView(): string;
  /** The view the channel shows in the column being worked on (`selection().view`). */
  channelView(): string;
  /** Whether the Pinned filter stands in for the channel's list (`pinsView().only`). */
  pinnedOnly(): boolean;
  /** The row a filter change holds in place, or null (`heldFilterRow`). */
  heldFilterRow(): HTMLElement | null;
  /** The walk for the search in the column being worked on, or null. */
  searchWalk(): AnchoredWalk | null;
  /** Whether `area` is at its newest line (`atBottom`). */
  atBottom(area: HTMLElement): boolean;
  /** Take the reader to the newest line (`scrollToNewest`). */
  scrollToNewest(): void;
  /** Decide the jump to the newest from where the reader is (`renderJumpNewest`). */
  renderJumpNewest(): void;
  /** Raise the jump to the newest over `view`, counting `turns` new turns (`setJumpNewest`). */
  setJumpNewest(on: boolean, view: string, turns: number): void;
}

/** The keeper's commands and questions. */
export interface ScrollKeeping {
  /** The list the reader is actually looking at. Both panes share one scrolling element. */
  visibleList(): HTMLElement;
  /** The channel's rows as they are on screen: the Pinned filter's list while it is on, the channel's otherwise. */
  shownChannelList(): HTMLElement;
  /** The message the reader's eye is on: the first one not already scrolled off the top. */
  scrollAnchor(area: HTMLElement): Element | null;
  /** Run a mutation and leave the reader looking at the same thing afterwards. */
  preservingScroll(mutate: () => void): void;
  /** `preservingScroll` for a change that resizes the list's window or the room at its head. */
  holdingReader(mutate: () => void): void;
  /** Where the reader is: which message their eye is on, and how far down the viewport it sits. */
  captureScroll(): ScrollMark;
  /** Put the reader back where `captureScroll` found them. */
  restoreScroll(mark: ScrollMark): void;
  /** A turn arrived: follow it only if the reader had not scrolled away. */
  followIfPinned(pinned: boolean, turns?: number, view?: string): void;
}

/** Keeping the reader in the page's lists. */
export function createScrollKeeping(deps: ScrollKeepingDependencies): ScrollKeeping {
  const el = deps.el;

  function visibleList(): HTMLElement {
    return el(deps.currentView() === "discord"
      ? (deps.channelView() === "threads" && !deps.pinnedOnly() ? "thread-list" : shownChannelList().id)
      : "transcript");
  }

  function shownChannelList(): HTMLElement {
    return el(deps.pinnedOnly() ? "pinned-log" : "discord-log");
  }

  /**
   * This is the anchor everything below is measured against. A browser has its own scroll anchoring,
   * and it is exactly the wrong tool here — it is defeated by a mutation ABOVE the viewport, which
   * is precisely what collapsing a message the reader has already scrolled past is.
   */
  function scrollAnchor(area: HTMLElement): Element | null {
    const edge = area.getBoundingClientRect().top;
    const items = visibleList().children;
    for (const li of items) {
      // A row hidden in a collapsed run of read messages has no box to be looking at. `#221 read-modes`.
      if (li.getAttribute("data-read-run") === "member") continue;
      if (li.getBoundingClientRect().bottom > edge) {
        return li;
      }
    }
    return items.length > 0 ? items[items.length - 1] as Element : null;
  }

  /**
   * Capture the anchor's offset, mutate, measure it again, and move the scroll position by the
   * difference. Explicit, because the alternative is trusting browser scroll anchoring, and the
   * whole point is that it does not hold for a change made above the viewport.
   *
   * A reader who was already at the bottom is the one exception: for them "the same thing" IS the
   * newest line, so the view follows it.
   */
  function preservingScroll(mutate: () => void): void {
    const mark = captureScroll();
    mutate();
    restoreScroll(mark);
  }

  /**
   * The error panel above #scroll-area, the freshness pill's room inside it: browser scroll anchoring
   * holds for neither, so the line being read, or the newest line, would move by the change's height:
   * 74-89px for a failed refresh's panel (`#33 error-banner-scroll-shift`).
   *
   * Not for a reader at the very top, nor off the main screen. At the top the first row is the
   * anchor, and holding it would scroll the header out of view: that reader is meant to see the
   * header move down. Off the main screen there is no list on screen to hold.
   */
  function holdingReader(mutate: () => void): void {
    if (deps.onMain() && el("scroll-area").scrollTop > 0) preservingScroll(mutate);
    else mutate();
  }

  /**
   * Split out of `preservingScroll` so that `#51 reply-view` can use the same mechanism across a
   * screen change, where the two halves are separated by everything the reader does on the reply
   * screen. One mechanism, two callers: an anchor is strictly better than a saved `scrollTop` here
   * as well, because hiding an element is allowed to reset its scroll position to zero — and a
   * restore expressed as a DELTA against the anchor is correct whether the browser did that or not.
   */
  function captureScroll(): ScrollMark {
    const area = el("scroll-area");
    // `#243 filter-keeps-selection`. While a filter change holds a message in place, that message is
    // where the reader is, and the newest line is not followed: nothing but the reader moves it.
    const held = deps.heldFilterRow();
    if (held) return { pinned: false, anchor: held, top: held.getBoundingClientRect().top };
    // `#262 search-load-older`. While a walk for the search reads older pages, its control holds still and
    // the matches open under it — never the newest line followed, on a list short enough to be at its
    // foot — until the reader scrolls it away, which lets it go. At the head of the list it may have moved
    // without the reader: the sentence above it that nothing matched goes once something does, and there is
    // no room above to keep it where it stood. Measured only while a walk runs.
    const walk = deps.searchWalk();
    if (walk && walk.anchored) {
      const chip = el("load-older");
      const top = chip.getBoundingClientRect().top;
      if (!chip.hidden && (Math.abs(top - walk.chipTop) <= 1 || area.scrollTop <= 1)) {
        walk.chipTop = top;
        return { pinned: false, anchor: chip, top };
      }
      walk.anchored = false;
    }
    const anchor = scrollAnchor(area);
    return {
      pinned: deps.atBottom(area),
      anchor,
      top: anchor ? anchor.getBoundingClientRect().top : 0,
    };
  }

  function restoreScroll(mark: ScrollMark): void {
    const area = el("scroll-area");
    if (mark.pinned) {
      deps.scrollToNewest();
      return;
    }
    // A list that was replaced while we were away has taken the anchor with it, and there is nothing
    // left to measure against; leaving the position alone beats jumping somewhere arbitrary.
    if (!mark.anchor || !mark.anchor.parentNode) {
      deps.renderJumpNewest();
      return;
    }
    // An anchor that has just joined a collapsed run of read messages is held by the run's line,
    // where it went. `#221 read-modes`.
    const shown = shownRowFor(mark.anchor);
    if (shown !== mark.anchor) mark = { ...mark, anchor: shown as Element };
    area.scrollTop += (mark.anchor as Element).getBoundingClientRect().top - mark.top;
    // Wherever that put the reader decides the jump, now: up the history it is offered, on the
    // newest line it is not. `#207 scrollback-jump`.
    deps.renderJumpNewest();
  }

  /**
   * `pinned` is measured BEFORE the append, because appending is itself what changes the answer.
   * When it is false the scroll position is left exactly alone and the "Newest" chip appears —
   * silently leaving the reader in place would be the other half of the same defect, since they
   * would have no way of knowing something had arrived at all.
   *
   * AND ONLY WHILE THE TRANSCRIPT IS THE LIST ON SCREEN. `pinned` is measured on #scroll-area, which
   * both panes share, so a turn arriving while the reader is over on the channel was asking "is the
   * reader at the bottom of the CHANNEL?" and following the answer: it scrolled the channel and put
   * the channel's chip away, and it never raised the transcript's. A call goes on talking while you
   * read a backlog, so that is the ordinary case, not a corner — and its cost was silence. The turn
   * belongs to a list nobody is looking at, so the only right act is to raise that list's chip.
   */
  function followIfPinned(pinned: boolean, turns = 0, view = "voice"): void {
    if (pinned && deps.currentView() === view) {
      deps.scrollToNewest();
      return;
    }
    // Named explicitly: `line()` and `seam()` append to the transcript, so they leave `view` as the
    // transcript's. Letting it default to `currentView` would raise a chip over the channel list. The
    // send card (`#269 approval-card-inline`) is the one caller that names a view: it is the next
    // message of whichever list is on screen. `turns` is how many new turns this was, for the count
    // the jump shows: a turn growing as it is spoken, a seam or the card is a change below and not
    // another message. `#207 scrollback-jump`.
    deps.setJumpNewest(true, view, turns);
  }

  return { visibleList, shownChannelList, scrollAnchor, preservingScroll, holdingReader, captureScroll, restoreScroll, followIfPinned };
}

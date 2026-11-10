// Walking back through the channel: a column's record of its older pages, what a step back asks,
// and the channel summary at the head of the list (`#624 voice-page-modularity`, moved from the
// legacy script). Words and order are as they were in the script.

import { createOlderPager, createPlainReach } from "../../core/channels/index.ts";
import type { OlderPager, PlainReach } from "../../core/channels/index.ts";

/** A column's own record of its older pages. */
export interface OlderPagesRecord {
  moreAbove: boolean | undefined;
  cursor: string | null;
  inFlight: boolean;
  step: Promise<void> | null;
  deferred: boolean;
}

/** What the older pages ask of the page. */
export interface OlderPagesDependencies {
  activeColumn(): { older: OlderPagesRecord };
  el(id: string): HTMLElement;
  transcript(): { seam(label: unknown, detail: string): Node };
}

/** The older pages: the record, the pager questions, the seam and the plain reach. */
export interface OlderPages {
  olderPages(): OlderPagesRecord;
  olderPager: OlderPager;
  plainReach: PlainReach;
  renderChannelSeam(label: unknown): void;
}

/*
 * Is anything older than the OLDEST ROW ON SCREEN still out there? A column's `olderPages().moreAbove`.
 *
 * THREE values, not two, and one variable rather than two that can disagree: `true` there is more
 * above, `false` the reader has reached the beginning, `undefined` the server did not say (a
 * server predating `has_more`). It is the single source for both things the page tells the reader
 * about that question — whether #load-older is offered, and whether the summary may state a total
 * — because when they were derived separately they disagreed: a background poll rewrote the
 * summary from the NEWEST page's `has_more` while the control stayed hidden, so the page claimed
 * more existed and offered no way to reach it. `#62 message-count-accuracy`.
 */
/**
 * A column's own record of its older pages (`#624 voice-page-modularity`, plan step 15: a column's
 * fields by group, on the column rather than swapped through the module's): `moreAbove`, the three
 * values just above; `cursor`, the cursor a step back walks from; `inFlight`, whether a step back is
 * being read; `step`, the step back under way (see below); `deferred`, a step back refused
 * while a finger was down and owed the reader.
 *
 * @returns {OlderPagesRecord}
 */
export function freshOlderPages(): OlderPagesRecord {
  return { moreAbove: false, cursor: null, inFlight: false, step: null, deferred: false };
}

/** The older pages over `deps`. */
export function createOlderPages(deps: OlderPagesDependencies): OlderPages {
  // --- walking back through the channel -----------------------------------------------------------
  //
  // `#65 scrollback-paging`. The server half of this landed with `#53 stepped-retrieval` and had no
  // caller at all: `GET /api/v1/channels/{id}/page` takes a `limit` and a `before` cursor, answers
  // with the messages oldest-first, and says whether more exist beyond them and which id to step back
  // from. This page used to read `/messages`, which is a WINDOW — the oldest message on screen was
  // simply the end of what this interface could ever show you, with nothing saying so.
  //
  // So the read moves onto the cursored route, and the reader can walk further back. Two ways in, on
  // purpose: scrolling to the top takes the next step by itself, which is the gesture people already
  // have; and #load-older is the control that SAYS more exists, that a keyboard can reach, and that
  // reports a step in flight. Neither is the master.
  //
  // Older messages arrive ABOVE the viewport, which is exactly the mutation a browser's own scroll
  // anchoring does not cover — the same case as collapsing a message the reader has scrolled past. So
  // it goes through `preservingScroll`, the mechanism `#47 scrollback-stability` already built, and
  // not a second one beside it.

  /*
   * Is anything older than the OLDEST ROW ON SCREEN still out there? A column's `olderPages().moreAbove`.
   *
   * THREE values, not two, and one variable rather than two that can disagree: `true` there is more
   * above, `false` the reader has reached the beginning, `undefined` the server did not say (a
   * server predating `has_more`). It is the single source for both things the page tells the reader
   * about that question — whether #load-older is offered, and whether the summary may state a total
   * — because when they were derived separately they disagreed: a background poll rewrote the
   * summary from the NEWEST page's `has_more` while the control stayed hidden, so the page claimed
   * more existed and offered no way to reach it. `#62 message-count-accuracy`.
   */
  
  /** The column being worked on's record of its older pages (`freshOlderPages`). */
  function olderPages() {
    return deps.activeColumn().older;
  }
  // Whether the last live page said more exists above but carried no cursor to walk back on, so that the
  // first step back owes the view a read of its own newest page first (`readWalkCursor`), is the channel
  // reads' (`channelReads.cursorOwed`, web/src/core/sync, `#624 voice-page-modularity`).

  /**
   * What a step back asks and what a page said about what is above (web/src/core/channels
   * `createOlderPager`, `#624 voice-page-modularity`). The column's own record stays above
   * (`freshOlderPages`/`olderPages`); this owns only the pure questions over it.
   *
   * @type {import("./src/core/channels/index.ts").OlderPager}
   */
  const olderPager = createOlderPager();

  // `#63 status-line-placement`. The channel's own summary — how much is loaded, and whether that is
  // the whole channel — used to be a line on the status strip, and the strip is transient now: a
  // message that is true for six seconds is the wrong home for a standing fact about what you are
  // looking at. So it is an entry at the HEAD OF THE LOG, in the same idiom the transcript uses for
  // a conversation boundary, and it scrolls away as the reader moves down instead of holding a strip
  // of the screen.
  //
  // The disclosure inside it answers the question the label raises and cannot answer on its own:
  // why there might be more than this. Kept to a couple of clauses, and measured by the suite
  // alongside the other two seams.
  const CHANNEL_SEAM_DETAIL =
    "Older messages load as you scroll up. A total appears only once everything back to the first message is loaded.";

  /**
   * Put the channel's summary at the head of the channel view, replacing any that is already there.
   *
   * In a list of its OWN, immediately above the log, rather than as the log's first child. That is a
   * deliberate departure from the issue's wording: `#discord-log`'s children are the channel's
   * messages, everywhere in this page and in its suite — `applyNewestPage` filters them by snowflake,
   * `scrollAnchor` walks them, and a couple of dozen assertions count them — and putting something
   * that is not a message among them redefines all of that for a placement. It is inside the
   * scrolling element either way, which is what the issue actually asks for: it scrolls off as the
   * reader moves down, exactly like the boundary in the transcript.
   *
   * REPLACING, not appending: `loadDiscord` and `loadOlder` both call this, and a summary that
   * stacked would grow one line per refresh — a background poll runs every forty-five seconds.
   */
  function renderChannelSeam(label: unknown) {
    deps.el("channel-summary").replaceChildren(deps.transcript().seam(label, CHANNEL_SEAM_DETAIL));
  }

  /**
   * Without threads: the instant below which the rows loaded are scattered, and whether the rows on screen
   * were drawn from this device with no read landed since (web/src/core/channels `createPlainReach`, `#624
   * voice-page-modularity`).
   */
  const plainReach = createPlainReach();

  /** @typedef {import("./src/present/notices/index.ts").HistoryReach} HistoryReach */

  return { olderPages, olderPager, plainReach, renderChannelSeam };
}

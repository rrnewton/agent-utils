// The card moving up as its thread opens, and the right column's thread life (`#298
// thread-card-header`, `#231 two-column-polish`; `#624 voice-page-modularity`, moved from the
// legacy script): where the card at the top starts from, selecting a thread on the right,
// closing it, forgetting the column for another channel, and reading it behind its rows. The
// flight's length is the stylesheet's, read as it was; entering a thread in one column and the
// scroll acts are the page's, handed in. Words, order and timing are as they were in the script.

import { threadOf } from "../../core/channels/index.ts";

/** The card at the top moving up while it is, as its owner hands it on (view/channel `createCardFlight`). */
export interface SideFlight {
  flyThreadCard(card: HTMLElement, from: { readonly left: number; readonly top: number } | null, pane: HTMLElement): void;
  endCardFlightPart(node: unknown, event: unknown): void;
  endCardFlight(): void;
}

/** A summary of a thread, as the right column holds it. */
export interface SideThreadSummary {
  readonly id: unknown;
}

/** What the side thread acts ask of the page. */
export interface SideThreadActsDependencies {
  leftById(id: string): HTMLElement;
  /** The flight owner, made by cardflight's factory, handed in as a maker (its deps are the page's). */
  makeFlight(ms: () => number, drawn: (node: HTMLElement) => boolean): SideFlight;
  cardRowOnRight(id: string | null): HTMLElement | null;
  rowShown(row: unknown): boolean;
  rowInView(pane: unknown, row: HTMLElement): boolean;
  sidePane(): unknown;
  openSideCards(): void;
  configApplied(): boolean;
  guardQuietly(run: () => unknown): () => void;
  refreshThreadDirectory(): unknown;
  // Selecting and closing the right column's thread.
  noteDeepLinkReaderMove(): void;
  selectedChannelId(): unknown;
  columnsShown(): boolean;
  threadSummary(id: string): SideThreadSummary | null;
  sideThreads(): {
    get(channel: string): { id: string; summary: unknown } | undefined;
    set(channel: string, held: { id: string; summary: unknown }): void;
    delete(channel: string): void;
  };
  sideMode(): string;
  setSideMode(mode: string): void;
  setSideFromCards(on: boolean): void;
  replyDockedAway(): boolean;
  replyInSide(): boolean;
  putDockedReplyAway(): void;
  renderSideMode(): void;
  inSideColumn<T>(act: () => T): T;
  selectionView(): string;
  selectionThreadId(): unknown;
  rowsMissDone(): boolean;
  renderCachedTimeline(): void;
  changeChannelView(view: string, thread: string, known: unknown): unknown;
  // Closing and forgetting.
  endSearchWalk(): void;
  dropReplyArrowTap(): void;
  putRepliesBack(): void;
  rememberChannelDraft(): void;
  rememberChannelContext(): void;
  bumpLoadGeneration(): void;
  clearGathered(): void;
  forgetUnpackReads(): void;
  stopUnpackWatch(): void;
  endRepliesWalk(): void;
  forgetRepliesNote(): void;
  clearSideSearchNote(): void;
  rememberCardPlace(): void;
  activeIsSide(): boolean;
  resetSideRecords(): void;
  forgetCards(): void;
  sideColumnUp(): boolean;
  threadListShown(): boolean;
  // Reading the right column behind its rows.
  loadSideDiscord(options: unknown): Promise<unknown>;
  channelContextKey(): string;
  activeColumn(): unknown;
  forwardCursor(reason: string): unknown;
  sideShowing(): boolean;
}

/** The side thread acts. */
export interface SideThreadActs {
  /** The newest timeline read each column sent, shared with the timeline reader. */
  readonly columnReadsSent: Map<unknown, { context: string; read: number; full: boolean }>;
  cardFlightMs(): number;
  cardFlightStart(id: string): { readonly left: number; readonly top: number } | null;
  readonly flight: SideFlight;
  drawnOnPage(node: HTMLElement): boolean;
  showSideList(): void;
  selectSideThread(id: string, summary?: unknown): unknown;
  closeSideThread(): void;
  resetSideColumn(reopen?: boolean): void;
  readSideColumn(reason: string, since?: number | null): Promise<void>;
  answeredSince(since: number, reason: string): boolean;
  inSideThread(message: unknown): boolean;
}

/** Make the side thread acts. */
export function createSideThreadActs(deps: SideThreadActsDependencies): SideThreadActs {
  /**
   * How long the card at the top of a thread takes to move up from its card in the list, in ms: the
   * stylesheet's `--card-flight` on #screen-main, which is 0 for a reader whose system asks for less
   * motion, as `--column-glide` is "auto" then. 0 too where the stylesheet says nothing.
   */
  function cardFlightMs(): number {
    const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(deps.leftById("screen-main")) : null;
    const said = style ? String(style.getPropertyValue("--card-flight")).trim() : "";
    const parsed = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(said);
    return parsed ? Number(parsed[1]) * (parsed[2] === "s" ? 1000 : 1) : 0;
  }

  /**
   * Where thread `id`'s card is in the cards on screen — on the right, or in the thread list — while any
   * of it shows: where the card at the top of its thread starts from. Else null.
   */
  function cardFlightStart(id: string): { readonly left: number; readonly top: number } | null {
    const row = deps.cardRowOnRight(id);
    if (!row || !deps.rowShown(row) || !deps.rowInView(deps.sidePane(), row)) return null;
    return row.getBoundingClientRect();
  }

  /** Whether `node` and everything it is in are drawn: none of them `hidden`. */
  function drawnOnPage(node: HTMLElement): boolean {
    for (let at: HTMLElement | null = node; at; at = at.parentNode as HTMLElement | null) if (at.hidden) return false;
    return true;
  }

  /**
   * The card moving up while it is, moving it, and ending it by its animations or a timer: one owner in
   * web/src/view/channel/cardflight.ts (`#624 voice-page-modularity`). The page destructures its commands
   * under their old names.
   */
  const flight = deps.makeFlight(() => cardFlightMs(), (node) => drawnOnPage(node));

  /** The right column with no thread selected: its cards, and the thread list read behind them. */
  function showSideList(): void {
    deps.openSideCards();
    // Not before sign-in has answered: the reopen's read reads the list behind the cards then.
    if (deps.configApplied()) deps.guardQuietly(() => deps.refreshThreadDirectory())();
  }

  /**
   * Select thread `id` on the right, from wherever the reader asked for it: its tag on a row, a count
   * of unread replies, a card, the view picker. Drawn at once from what the page holds and read
   * behind, landing on its first unread, exactly as a thread opens in one column.
   */
  function selectSideThread(id: string, summary: unknown = null): unknown {
    deps.noteDeepLinkReaderMove();
    const thread = String(id);
    const channel = String(deps.selectedChannelId());
    if (!deps.columnsShown() || !channel) return undefined;
    // Entered from its card, `enterCard` says so once this returns. `#277 thread-card-focus`.
    deps.setSideFromCards(false);
    const known = summary || deps.threadSummary(thread) || null;
    deps.sideThreads().set(channel, { id: thread, summary: known });
    const was = deps.sideMode();
    // `#298 thread-card-header`. Where its card is, while the cards show it, for the card at the top to
    // move up from.
    const from = was === "list" ? cardFlightStart(thread) : null;
    // The card's place is remembered by the card-place acts, through the page's act as it was called.
    if (was === "list") deps.rememberCardPlace();
    // `#410 inline-reply-editor`. A reply open under a row unpacked among the cards goes with them, its
    // words kept as a draft: it is the left column's reply (`openReply`), drawn in a list now hidden.
    if (deps.replyDockedAway()) deps.putDockedReplyAway();
    deps.setSideMode("thread");
    deps.renderSideMode();
    const opened = deps.inSideColumn(() => {
      if (was === "thread" && deps.selectionView() === "thread" && String(deps.selectionThreadId()) === thread) {
        // Shown again as it was put away, two columns coming back to it: drawn again where a Done or an
        // Undo made meanwhile in one column left its rows behind its copy. `#306 done-stale-across-views`.
        if (deps.rowsMissDone()) deps.renderCachedTimeline();
        return undefined;
      }
      return deps.changeChannelView("thread", thread, known);
    });
    flight.flyThreadCard(deps.leftById("side-thread-card"), from, deps.leftById("side-pane"));
    return opened;
  }

  /** The X: the thread goes, and the cards come back. Its rows are held, so selecting it again is instant. */
  function closeSideThread(): void {
    // `#277 reply-in-column`. A reply open under the thread goes with it, its words kept as a draft.
    if (deps.replyInSide()) deps.putDockedReplyAway();
    // `#262 search-load-older`. The thread's walk for the search goes with it: nothing else it asks
    // about changes when the cards come back.
    deps.endSearchWalk();
    const channel = String(deps.selectedChannelId());
    deps.sideThreads().delete(channel);
    if (deps.sideMode() === "thread") {
      deps.inSideColumn(() => {
        deps.dropReplyArrowTap();
        deps.putRepliesBack();
        deps.rememberChannelDraft();
        deps.rememberChannelContext();
        deps.bumpLoadGeneration();
      });
    }
    showSideList();
  }

  /**
   * Forget what the right column shows, for another channel or another credential. `reopen`, for a
   * channel the reader changed to: show that channel's thread selected there earlier in this session,
   * or its cards and the thread list read behind them.
   */
  function resetSideColumn(reopen = false): void {
    // `#404 threads-unpack-all`. Unpack reads were the last channel's.
    deps.forgetUnpackReads();
    deps.stopUnpackWatch();
    deps.endSearchWalk();
    deps.clearSideSearchNote();
    // `#262 search-load-older`. And the cards' walk, and what the last one found: they were this channel's.
    deps.endRepliesWalk();
    deps.forgetRepliesNote();
    deps.inSideColumn(() => {
      deps.dropReplyArrowTap();
      deps.clearGathered();
      deps.bumpLoadGeneration();
    });
    // Made afresh, as when it was first used: only while it is not the column worked on, whose records
    // are live.
    if (!deps.activeIsSide()) deps.resetSideRecords();
    for (const id of ["side-log", "side-thread-list", "side-summary", "side-pinned-log", "side-outgoing-log"]) {
      deps.leftById(id).replaceChildren();
    }
    deps.leftById("side-notice").hidden = true;
    deps.leftById("side-older").hidden = true;
    deps.leftById("side-loading").hidden = true;
    deps.forgetCards();
    deps.leftById("side-cards").replaceChildren();
    if (!deps.sideColumnUp() || !reopen) {
      deps.setSideMode("list");
      if (deps.sideColumnUp()) deps.openSideCards();
      return;
    }
    // `#281 view-keys`. The thread list shows cards, whatever two columns had selected on the right.
    if (deps.threadListShown()) {
      deps.setSideMode("list");
      showSideList();
      return;
    }
    const open = deps.sideThreads().get(String(deps.selectedChannelId()));
    if (open) {
      deps.setSideMode("list");
      deps.guardQuietly(() => selectSideThread(open.id, open.summary))();
    } else {
      showSideList();
    }
  }

  /**
   * Read the right column's thread behind its rows, or the thread list behind its cards: the poll's
   * second read, and coming back to the page. Nothing while one column is up.
   *
   * `since` is `replayLedger.started()` as it stood when the refresh asking for this began, before its
   * read of Main. `#231 two-column-polish`. A read of the thread on the right that went on the wire
   * after that is this read already: a thread selected while the poll's read of Main was out queued its
   * read behind it, and that read ran before the poll got here. Reading it again brought nothing the
   * first had not, a second request for the same rows. See `answeredSince` for which reads count.
   */
  async function readSideColumn(reason: string, since: number | null = null): Promise<void> {
    if (!deps.sideColumnUp()) return;
    if (deps.sideMode() === "thread") {
      await deps.inSideColumn(() => (since !== null && answeredSince(since, reason)
        ? undefined : deps.loadSideDiscord({ keepPosition: true, reason, readAs: "side" })));
    } else {
      await deps.refreshThreadDirectory();
    }
  }

  /**
   * The newest timeline read each column sent and that has not failed before landing a page: the view
   * it was for (`channelContextKey`), its number among `replayLedger.started()`, and whether it read the
   * view in full rather than as what changed. `#231 two-column-polish`. One still on the wire counts,
   * as the read a refresh would send would only queue behind it; if it then fails, the right column's
   * line says so (`renderSideFreshness`) and the next poll reads the view again. A chain of what
   * changed that fails after its first page keeps its record, which no refresh begun before it can
   * act on: the chain ran queued behind that refresh's read of Main, and its failure is that read's.
   */
  const columnReadsSent = new Map<unknown, { context: string; read: number; full: boolean }>();

  /**
   * Whether a read this column sent of the view it shows now, after read number `since`, answers for
   * a read for `reason`. Only one sent after: one that left BEFORE the refresh asking may have left
   * before what that refresh reads for — an edit the stream announced, say — had happened. And only
   * one that read as much as `reason` needs: a read in full answers anything, and a read of what
   * changed answers a reason only where that reason may be read so too (`forwardCursor`) — on a
   * channel whose changes carry additions alone, an edit's read is in full, and a delta read for the
   * poll would leave the edit unread.
   */
  function answeredSince(since: number, reason: string): boolean {
    const sent = columnReadsSent.get(deps.activeColumn());
    return Boolean(sent) && (sent as { read: number }).read > since && (sent as { context: string }).context === deps.channelContextKey() &&
      ((sent as { full: boolean }).full || deps.forwardCursor(reason) !== null);
  }

  /** Whether a message belongs to the thread the right column is showing. */
  function inSideThread(message: unknown): boolean {
    if (!deps.sideShowing()) return false;
    return deps.inSideColumn(() => deps.selectionView() === "thread" && threadOf(message) === String(deps.selectionThreadId()));
  }

  return {
    columnReadsSent,
    cardFlightMs,
    cardFlightStart,
    flight,
    drawnOnPage,
    showSideList,
    selectSideThread,
    closeSideThread,
    resetSideColumn,
    readSideColumn,
    answeredSince,
    inSideThread,
  };
}

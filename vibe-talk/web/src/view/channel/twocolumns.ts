// Two columns on a desk (`#229 desktop-two-column`, `#281 view-keys`, `#261
// two-column-connectors`, `#269 approval-card-inline`, `#280 scroll-stability`, `#298
// thread-card-header`, `#304 back-gesture-in-app`, `#314 reply-focus-flake`, `#338
// column-overscroll`, `#410 inline-reply-editor`, `#419 threads-view-main-messages`, `#262
// search-load-older`; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel for the plan's step 17): which column the code is working on, what layout is
// on screen (one column, the thread list, or two), going into and out of each, and what the
// right column shows. Esc on the thread list, opening a thread from it and leaving it
// (threadListBack, openThreadFromList, leaveThreadList) stay the page's until entering a
// thread's move takes them. The columns, the store, the cards and everything the layout
// walks are handed in; the words are the page's.

/** What the layout last chose was chosen from, for Esc on the thread list (`ListBefore`). */
export interface ListBefore {
  readonly layout: string;
  readonly columns?: unknown;
  readonly channel?: string;
  readonly view?: string;
  readonly thread?: string | null;
  readonly origin?: string | null;
}

/** The layout choice, as the two columns read it (`createLayoutChoice`). */
export interface LayoutChoiceLike {
  chosen(): LayoutName;
  beforeColumns(): LayoutName;
  choose(layout: LayoutName): void;
  noteListBefore(fn: () => { layout: "one"; channel: string; view: string; thread: string | null; origin: string | null } | null): void;
  listBefore(): ListBefore;
  restoreBeforeColumns(columns: unknown): void;
}

/** What the layout shows, as the two columns read it (`createLayoutShown`). */
export interface LayoutShownLike {
  sideUp(): boolean;
  threadList(): boolean;
  columns(): boolean;
  layout(): LayoutName;
  noteFit(fit: boolean): boolean;
  takeListCards(): CardsPlace | null;
  takeLeftColumn(): LeftPlace | null;
  holdListCards(place: CardsPlace): void;
  holdLeftColumn(place: LeftPlace): void;
  holdListPlace(place: ListPlace | null): void;
  askOneColumn(target: OneColumnView): void;
  takeOneColumn(): OneColumnView | null;
  whileRestoring(fn: () => void): void;
  setColumns(on: boolean): void;
  setThreadList(on: boolean): void;
}

/** One column, the thread list, or two (`#281 view-keys`). */
export type LayoutName = "one" | "list" | "two";

/** The one-column view asked for with a layout. */
export interface OneColumnView {
  view: string;
  thread: string | null;
  summary: unknown;
  origin: string | null;
}

/** Where the reader of the left column is (`readerPlace`), as the layout keeps it. */
export interface ReaderPlace {
  pinned: boolean;
  seen: readonly { ids: readonly string[]; top: number }[];
  card?: unknown;
}

/** Where the reader of the left column is, and the view it was read in. */
export interface LeftColumnPlace {
  place: ReaderPlace;
  view: string;
  channel: string;
}

type ListPlace = { place: ReaderPlace; view: string; channel: string };
type CardsPlace = { place: { newest: boolean; id: string | null; offset: number }; placed: { newest: boolean; id: string | null; offset: number } | null; channel: string };
type LeftPlace = { left: LeftColumnPlace; placed: LeftColumnPlace; from: "one" | "two" };

/** A column of the two, as the layout works on it. */
export interface LayoutColumn {
  name: string;
  mode?: string;
}

/** The reply open in a column, as leaving two columns reads it. */
export interface LayoutReplyState {
  docked: boolean;
  target: { id: unknown } | null;
  pending: { id?: string; ready: boolean; typing: boolean } | null;
  focusTaken: boolean;
}

/** What the two-columns layout is handed by the page. */
export interface TwoColumnsDependencies {
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  mainColumn(): LayoutColumn;
  sideColumn(): LayoutColumn;
  activeColumn(): LayoutColumn;
  columnSide(): boolean;
  useColumn(column: LayoutColumn): void;
  inColumn<T>(column: unknown, act: () => T): T;
  screen(): string;
  view(): string;
  threading(): boolean;
  selectedChannelId(): unknown;
  selection(): { view: string; threadId: string | null; thread: unknown; origin: string | null };
  channelCanon(): { channel: string; scope: unknown; threads: readonly { id: unknown }[] };
  layoutChoice: LayoutChoiceLike;
  layoutShown: LayoutShownLike;
  settleFocus(): void;
  setStatus(text: string): void;
  noteDeepLinkReaderMove(): void;
  guardQuietly(fn: (...args: never[]) => unknown): (...args: never[]) => unknown;
  renderThreadSelect(): void;
  scheduleColumnLinks(): void;
  scheduleBackSync(): void;
  cardAnchor(): CardsPlace["place"] | null;
  renderSideCards(): void;
  readerPlace(): ReaderPlace;
  keepReaderPlace(place: ReaderPlace): boolean;
  changeChannelView(view: string, thread?: string | null, summary?: unknown, origin?: string | null, place?: ReaderPlace | null): void;
  viewOnScreen(): OneColumnView | null;
  rememberView(view: OneColumnView): void;
  settleUiState(): void;
  saveUiState(): void;
  configApplied(): boolean;
  hydrateChannelScope(): void;
  restoreChannelComposer(): void;
  selectSideThread(id: string, summary: unknown): void;
  showSideList(): void;
  renderSearchInAll(): void;
  refreshSearchCount(): void;
  closeReply(options?: unknown): void;
  replyState(): LayoutReplyState;
  nodeWithin(area: HTMLElement, node: unknown): boolean;
  focusLost(): boolean;
  endSearchWalk(column: LayoutColumn): void;
  endRepliesWalk(): void;
  dropReplyArrowTap(): void;
  putRepliesBack(): void;
  rememberChannelDraft(): void;
  rememberChannelContext(): void;
  reopenPendingReply(): void;
  renderMainToggle(): void;
  renderOrderToggle(): void;
  placeMainCard(): void;
  renderUnpackToggle(): void;
  renderRepliesSearch(): void;
  noteLinkedSelection(): void;
  closeOverscroll(): void;
  renderSideFreshness(): void;
  setTextIfChanged(el: HTMLElement, text: string): void;
  nameSideRegion(name: string): void;
  paintThreadCard(el: HTMLElement, choice: unknown, tally: unknown): void;
  replyTally(): unknown;
  knownThreadChoice(id: string, fallback: unknown): unknown;
  openThreadChoice(): unknown;
  threadName(choice: unknown, limit: number): string;
  threadStillKnown(id: string): boolean;
  defaultChannelView(): string;
  openingChannelView(): OneColumnView;
  followIfPinned(pinned: boolean, slack: number, view: string): void;
  scrollToNewest(): void;
  pinsView(): { only: boolean };
  leaveThreadList(next: LayoutName): void;
  catchUpHeldView(): boolean;
  readFilterKeeps(): (message: { id: unknown }) => boolean;
  settleAfterRead(messages: readonly { id: unknown }[], options: unknown): void;
  timelineRows(): { messages: readonly { id: unknown }[] };
  hiddenEmpty(message: { id: unknown }): boolean;
  hidingRead(): boolean;
  channelReadPosition(area: HTMLElement): object;
  syncUnreadReplies(): void;
  placeCardAnchor(place: CardsPlace["place"] | null, follow: boolean): boolean;
  cardsPlacedUnderLists(): void;
  sameCardPlace(a: CardsPlace["place"], b: CardsPlace["place"]): boolean;
  liveFollowersMain(): boolean;
  transcriptPlacePostCard(): void;
  transcriptCardShown(): boolean;
  reopenedViewsIncludes(view: string): boolean;
  channelOnScreen(): boolean;
}

/** The two-columns layout, as the page calls it. */
export interface TwoColumns {
  readonly layoutChoice: LayoutChoiceLike;
  readonly layoutShown: LayoutShownLike;
  readonly sideColumnUp: () => boolean;
  threadListOnScreen(): boolean;
  readonly sideThreads: Map<string, { id: string; summary: unknown }>;
  sideShowing(): boolean;
  readonly COLUMNS_TOO_NARROW: string;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => unknown): (...args: A) => unknown;
  columnSteps(): { column: LayoutColumn; resume(): void; pause(): void };
  eachColumn(fn: () => unknown): void;
  inOtherColumn(fn: () => unknown): void;
  followInOtherColumn(): void;
  threadListOffered(): boolean;
  columnsOffered(): boolean;
  renderColumnToggle(): void;
  sideHasBox(): boolean;
  toggleHasBox(): boolean;
  renderColumns(): void;
  moveLayout(now: LayoutName, next: LayoutName, cards?: CardsPlace["place"] | null, left?: LeftColumnPlace | null): void;
  leftColumnPlace(): LeftColumnPlace | null;
  keepLeftColumnPlace(left: LeftColumnPlace, back?: LeftColumnPlace | null): void;
  sameLeftPlace(now: LeftColumnPlace, placed: LeftColumnPlace): boolean;
  setLayout(layout: LayoutName, back?: boolean): void;
  noteListBefore(): void;
  oneColumnBeforeList(before: ListBefore): OneColumnView;
  columnsFromKeyboard(): boolean;
  toggleColumns(): void;
  chooseView(which: string): void;
  chooseOneColumn(target: OneColumnView): void;
  showOneColumnView(target: OneColumnView, record: boolean, place?: ReaderPlace | null): void;
  recordViewChoice(): void;
  leftColumnToMain(place?: ReaderPlace | null): void;
  enterColumns(from?: LayoutName, left?: LeftColumnPlace | null): void;
  leaveColumns(next?: LayoutName, left?: LeftColumnPlace | null): (() => void) | null;
  showOneColumnLeft(place?: ReaderPlace | null): void;
  enterThreadList(from?: LayoutName, left?: LeftColumnPlace | null): void;
  renderSideMode(): void;
  renderSideColumn(): void;
  sideThreadChoice(): unknown;
}

/** Make the two-columns layout from the page's columns, store and cards. */
export function createTwoColumns(deps: TwoColumnsDependencies): TwoColumns {
  const layoutChoice = deps.layoutChoice;
  const layoutShown = deps.layoutShown;

  // --- two columns on a desk ------------------------------------------------------------------------
  //
  // `#229 desktop-two-column`. The owner, 2026-10-09: he likes the two-column view other chat clients'
  // desktop apps give and uses it a lot, and asked for "a layout button that toggles between single
  // column and double column. When in double column the thread selector greys out because it has a
  // fixed view where main is in the left column and the right column contains the messages from the
  // selected thread. When no thread is selected it contains a list of threads with fixed sized ~3
  // line previews of the 1st message text in EACH thread OR (even better) the 'descriptive-slug: short
  // description from summarization' ... We already have a button for clicking on a message to go into
  // the thread containing it. So we can use that to 'select' different threads from the left column
  // view. We need a button on the right hand side thread column to 'exit' (deselect) the thread and go
  // back to the list."
  //
  // WHERE. Only on a desk (the desktop block of web/voice.css: a fine pointer) in a window at least
  // 1000px wide. At 1000px each column is about 490px, which after a row's insets is about 45
  // characters: the narrowest reading width the width setting itself allows. Narrower than that, one
  // of the two columns would be narrower than the page lets a reader make its only column, so the
  // button is not offered and a window narrowed below it shows one column, keeping the choice for
  // when it is wide again. A phone never sees any of it. The stylesheet decides all of that, and the
  // script asks it, by whether the right column has a box (`renderColumns`): `voice.js` reads no media
  // query and no user agent, which the page suite holds it to.
  //
  // WHAT EACH COLUMN SHOWS. The left is Main, always. (The view picker greyed out here until `#281
  // view-keys` made two columns one of its four views.) The right is the thread selected, read and drawn exactly as a thread is in
  // one column (read modes, the landing on its first unread, reply arrows, reactions, Done, pins, the
  // walk back and its own composer at its foot), with a heading naming it and an X that deselects it.
  // With none selected it is a card per thread: "display_name: summary" where the summariser has named the thread and summed it up, else the
  // text of its first message, three lines at most.
  //
  // SELECTING. Every way into a thread selects it on the right instead of leaving Main: its Thread(N)
  // tag, the "N unread" on a root, the root's "N replies" (which in one column gathers the replies
  // under the root, and here would show them twice, side by side), the view picker, and a card. A
  // plain tap on a message does not: a tap folds and unfolds it, and replacing the right column on
  // every fold would throw away the reader's place in the thread they are reading.
  //
  // THE FILTERS. One search bar, so the search and the Links filter apply to both columns and the
  // count speaks for both: a filter that silently skipped half the screen is the defect `#129` names
  // for the Threads tab. The Pinned filter is a list of the channel's pins, and stands in for the left
  // column's list, as it does in one column. One read mode, for both.
  //
  // THE COST. No timer of its own. The poll that reads Main reads the thread on the right behind it:
  // two different views, one delta each, never the same view twice; with the cards up it asks for the
  // thread list, at most once a minute (`refreshThreadDirectory`). A refresh that brings nothing draws
  // nothing in either column. A live reply in the thread on the right is read once, as the thread.
  //
  // LEAVING. One column again shows the view last chosen in one column, or the one Ctrl+1 or Ctrl+2 or
  // the picker asked for: in two, the left column is Main by the layout, so nothing here touches that
  // choice. The thread selected on the right is kept per channel for the session, and selected again
  // when two columns come back.
  //
  // THE THREAD LIST. `#281 view-keys`. The owner, 2026-10-13: "Recall I wanted thread list view to be
  // converged with the two-column rendering of threads on the right. TBH I think two column view should
  // just be Cmd-4 as the 4th view." So the thread list (Ctrl+3) IS the right column, on its own across
  // the window — its heading, Unread only, the freshness line, "Older threads…" and the cards, one
  // rendering — and two columns (Ctrl+4) are Main beside it. It is drawn at every width, a phone's
  // included (`data-columns="list"` in web/voice.css), over the left column, which stays Main behind it
  // as it is in two columns, so going from one to the other moves nothing but the left column. A card
  // chosen there opens its thread across the window, one column, and Back from that thread is the list
  // again (`openThreadFromList`); its cards never show a thread themselves.
  //
  // HOW. See `COLUMN_IDS` at the head of this file: the code works on one column at a time, the left
  // one except inside `inColumn`. Everything that starts in the right column gets there first: its
  // rows' controls and its own buttons are bound to it when they are made (`inThisColumn`), the keys
  // by the pane the focus is in, and a read, a step back or a seek that waits comes back to it after
  // each wait (`columnSteps`). What changes in one column and shows in both — a Done, a pin, a mode,
  // a filter — is drawn in each (`eachColumn`).



  /** Whether the right column is on screen: beside Main, or on its own as the thread list. */
  const sideColumnUp = layoutShown.sideUp;

  /**
   * Whether the thread list is what is on screen now: the layout, on the channel of the main screen. It
   * waits behind the call's view and the other screens, as two columns do, and the left column's call
   * pane, its box included, is not covered there.
   */
  const threadListOnScreen = () => layoutShown.threadList() && deps.screen() === "main" && deps.view() === "discord";

  /**
   * By channel, the thread the right column shows, as `{id, summary}`, for this session: two columns
   * left and come back to, or a channel left and come back to, find it selected again.
   *
   * @type {Map<string, {id: string, summary: any}>}
   */
  const sideThreads = new Map();

  // Working on a column (`useColumn`) and running in one (`inColumn`) are the column work's (above, `#624
  // voice-page-modularity`): the column put aside has been worked on and `left`, the one taken up has not.

  /**
   * `fn`, bound to the column being worked on now: a control made while a column is drawn acts in that
   * column whenever it is pressed. The left column's controls work as they always did.
   *
   * @param {(...args: any[]) => any} fn
   * @returns {(...args: any[]) => any}
   */
  function inThisColumn<A extends unknown[]>(fn: (...args: A) => unknown): (...args: A) => unknown {
    const column = deps.activeColumn();
    return (...args) => deps.inColumn(column, () => fn(...args));
  }

  /**
   * The column an asynchronous step was asked for in, kept across its waits.
   *
   * A wait ends in a later task, and every task starts in the left column, since `inColumn` always puts
   * it back. So a step asked for in the right column that waits — a read, a step back, the seek a
   * reply's arrow makes — would go on in the left one, and draw a thread's page into Main's list. Such a
   * step says `resume()` as the first thing after each wait, which brings its own column back, and
   * `pause()` just before each wait after that and on its way out, which gives whatever runs next the
   * left column again. Before its first wait it is still in the code that called it, in that code's
   * column, and `pause()` leaves it there. Only the `await` of an `async` function's promise, which
   * always waits, may stand between the two.
   */
  function columnSteps(): { column: LayoutColumn; resume(): void; pause(): void } {
    const column = deps.activeColumn();
    let resumed = false;
    return {
      column,
      resume() {
        resumed = true;
        deps.useColumn(column);
      },
      pause() {
        if (resumed) deps.useColumn(deps.mainColumn());
      },
    };
  }

  /** Whether the right column is showing a thread now. */
  const sideShowing = () => layoutShown.columns() && deps.sideColumn().mode === "thread";

  /**
   * Run `fn` in every column showing the channel's messages: the left, and the right while it shows a
   * thread. For what is the same in both — a mode, a filter, a Done, a pin — drawn in each.
   *
   * @param {() => unknown} fn
   */
  function eachColumn(fn: () => unknown): void {
    deps.inColumn(deps.mainColumn(), fn);
    if (sideShowing()) deps.inColumn(deps.sideColumn(), fn);
  }

  /**
   * Run `fn` in the column that is not the one being worked on, when both are showing messages: what
   * one column changed in the channel, caught up in the other.
   *
   * @param {() => unknown} fn
   */
  function inOtherColumn(fn: () => unknown): void {
    if (!sideShowing()) return;
    deps.inColumn(deps.columnSide() ? deps.mainColumn() : deps.sideColumn(), fn);
  }

  /**
   * Bring the other column up to date with the store after something in this one changed it — a read
   * that landed, a reply that arrived — as a refresh of its own would: its rows caught up, the newest
   * line followed or the reader's place kept, and on Main the roots' counts of unread replies redrawn.
   */
  function followInOtherColumn(): void {
    inOtherColumn(() => {
      if (deps.catchUpHeldView()) {
        const keeps = deps.readFilterKeeps();
        deps.settleAfterRead(deps.timelineRows().messages.filter((message) => !deps.hiddenEmpty(message) && (!deps.hidingRead() || keeps(message))),
          { keepPosition: true, area: deps.el("scroll-area"), ...deps.channelReadPosition(deps.el("scroll-area")) });
      } else {
        deps.syncUnreadReplies();
      }
    });
  }

  /**
   * Whether the channel has a list of threads to show: a provider with threads, and a channel that is
   * not itself one thread, which has no others. `#281 view-keys`: what the thread list and two columns
   * are offered by, in the picker and to the keys, on any screen.
   */
  function threadListOffered(): boolean {
    const channel = String(deps.selectedChannelId() || "");
    return deps.threading() && channel !== "" && !(deps.channelCanon().channel === channel && deps.channelCanon().scope);
  }

  /**
   * Whether two columns, or the thread list, are offered now: the channel on the main screen, with a
   * list of threads to show (`threadListOffered`). How wide the window is, and whether this is a desk,
   * the stylesheet decides (`#column-toggle` is drawn only in the desktop block at 1000px and wider).
   */
  function columnsOffered(): boolean {
    return deps.screen() === "main" && deps.view() === "discord" && threadListOffered();
  }

  /** The layout button: whether it is offered, and whether it is pressed. */
  function renderColumnToggle(): void {
    const button = deps.leftEl("column-toggle");
    const offered = columnsOffered();
    if (button.hidden === offered) button.hidden = !offered;
    const chosen = layoutChoice.chosen() === "two";
    button.setAttribute("aria-pressed", chosen ? "true" : "false");
    const title = !chosen ? "Show Main and a thread side by side"
      : layoutChoice.beforeColumns() === "list" ? "Back to the thread list" : "Back to one column";
    if (button.title !== title) button.title = title;
  }

  /** Whether the right column has a box, which only web/voice.css can give it. */
  function sideHasBox(): boolean {
    const side = deps.leftEl("side-column");
    return typeof side.getClientRects === "function" && side.getClientRects().length > 0;
  }

  /** Whether the layout button has a box: whether this window is a desk wide enough for two columns. */
  function toggleHasBox(): boolean {
    const button = deps.leftEl("column-toggle");
    return !button.hidden && typeof button.getClientRects === "function" && button.getClientRects().length > 0;
  }

  /**
   * Decide again what is on screen — one column, the thread list, or two columns — and go into it or
   * out of what was. Asked on the changes that can move the answer — the layout chosen, the window
   * resized, the view, the channel, the screen — and never per scroll: asking whether the right column
   * has a box lays the page out. Off the main screen nothing is decided, so a visit to Settings or
   * Reply comes back to the layout as it was.
   */
  function renderColumns(): void {
    if (deps.screen() !== "main") return;
    renderColumnToggle();
    const screen = deps.leftEl("screen-main");
    const fit = toggleHasBox();
    const fitChanged = layoutShown.noteFit(fit);
    let want = columnsOffered() && layoutChoice.chosen() !== "one" ? layoutChoice.chosen() : null;
    // Two columns in a window too narrow for them: the layout they were chosen from — the thread list, or
    // one column — until it is wide enough, the choice kept for then. `#281 view-keys`.
    if (want === "two" && !fit) want = layoutChoice.beforeColumns() === "list" ? "list" : null;
    // `#280 scroll-stability`. Where the reader is in the cards, read before the layout changes the width
    // they are drawn at: between the thread list and two columns they are kept, and put back there.
    const cards = deps.view() === "discord" && screen.getAttribute("data-columns") !== want ? deps.cardAnchor() : null;
    // `#269 approval-card-inline` x `#280 scroll-stability`. And where the reader of the left column is,
    // read before the layout draws that column at another width (`leftColumnPlace`).
    const left = screen.getAttribute("data-columns") !== want ? leftColumnPlace() : null;
    if (screen.getAttribute("data-columns") !== want) {
      if (want) screen.setAttribute("data-columns", want);
      else screen.removeAttribute("data-columns");
    }
    // The call's view is one list. Two columns, or the thread list, wait behind it, as they were.
    if (deps.view() === "discord") {
      const now = layoutShown.layout();
      const next = want !== null && sideHasBox() ? want : "one";
      if (next !== now) moveLayout(now, next, cards, left);
      else if (fitChanged) deps.renderThreadSelect();
    }
    // `#261 two-column-connectors`. The links between the columns, drawn or taken away with them.
    deps.scheduleColumnLinks();
    // `#304 back-gesture-in-app`. The thread list is a level, and two columns are not.
    deps.scheduleBackSync();
  }

  /**
   * From layout `now` to layout `next`: out of the one, then into the other. `#281 view-keys`. Between
   * the thread list and two columns the right column stays as it is, and only the left one comes or goes.
   *
   * `#280 scroll-stability`. THE CARDS ARE PLACED ONCE THE LAYOUT IS WHOLE. The picker names the layout
   * that is up, and a <select> is as wide as what it says: "Thread list" is wider than "Main", and at a
   * desk that wrapped the dock onto a second row after the cards had been opened at their foot, which left
   * the thread list above it in Chromium, 43px from Main and 86px from two columns. So the cards are placed
   * again once the picker says the new layout: where the reader was in them (`cards`, read by
   * `renderColumns` before the layout changed) between the thread list and two columns, which keep them;
   * else where they were just opened, their foot unless a thread was selected from them up the list
   * (`placeOpenedCards`). The dock growing later than that, by something no placement here can wait for —
   * its last button coming in after a reload — is for the right column's dock-growth keeper (`#229`), which
   * takes a reader at the foot of the cards back to it.
   *
   * `#280 scroll-stability` x `#269 approval-card-inline`. Back from two columns to the thread list they
   * were brought from, the reader still where two columns put them in the cards: where they were in the
   * list (`layoutShown.holdListCards`). A send card docked under the list makes its column of cards shorter than two
   * columns' right column, so a reader near the foot of the list was at the foot in two columns, and that
   * is no reason to be at the foot of the list.
   *
   * `#269 approval-card-inline` x `#280 scroll-stability`. THE LEFT COLUMN'S READER IS WHERE THEY WERE
   * BEFORE THE LAYOUT CHANGED (`left`, read by `renderColumns` before it did): in the view the left column
   * goes to, Main into two columns and the view chosen out of them, or in the view it stays on, put back
   * once the picker says the new layout (`keepLeftColumnPlace`). See `leftColumnPlace`. And back to the
   * layout the last change came from, where they were there, unless they moved since (`layoutShown.holdLeftColumn`).
   *
   * @param {"one" | "list" | "two"} now
   * @param {"one" | "list" | "two"} next
   * @param {{newest: boolean, id: string | null, offset: number} | null} [cards]
   * @param {ReturnType<typeof leftColumnPlace>} [left]
   */
  function moveLayout(now: LayoutName, next: LayoutName, cards: CardsPlace["place"] | null = null, left: LeftColumnPlace | null = null): void {
    // Spent by this change of layout, whatever it is.
    const held = layoutShown.takeListCards();
    const heldLeft = layoutShown.takeLeftColumn();
    // `#269 approval-card-inline` x `#280 scroll-stability`. Back to the layout the last change came from,
    // the left column's reader where that change left them: where they were there.
    const leftBack = left && heldLeft && heldLeft.from === next && sameLeftPlace(left, heldLeft.placed) ? heldLeft.left : null;
    // `#410 inline-reply-editor`. A reply open in a column of the two that go is opened again in the stream
    // once the next layout is drawn under it, where its message is drawn (`leaveColumns`).
    const handOff = now === "two" ? leaveColumns(next, left) : null;
    if (now === "list") deps.leaveThreadList(next);
    if (next === "two") enterColumns(now, left);
    else if (next === "list") enterThreadList(now, left);
    const kept = now !== "one" && next !== "one" ? cards : null;
    const place = sideColumnUp() && deps.sideColumn().mode === "list" ? kept || deps.cardAnchor() : null;
    const channel = String(deps.selectedChannelId());
    const back = place && kept && now === "two" && held && held.placed && held.channel === channel && deps.sameCardPlace(kept, held.placed)
      ? held.place : null;
    deps.renderThreadSelect();
    if (left && next !== "list") keepLeftColumnPlace(left, leftBack);
    if (back && deps.placeCardAnchor(back, true)) {
      // The card docked again under the list is not the dock growing under a reader two columns had at the
      // foot of their cards (`cardsPlacedUnderLists`).
      deps.cardsPlacedUnderLists();
    } else {
      // Where the card the list's place was on has gone since, the place two columns had.
      deps.placeCardAnchor(place, true);
    }
    if (place && kept && now === "list" && next === "two") layoutShown.holdListCards({ place: kept, placed: deps.cardAnchor(), channel });
    if (left && now !== "list" && next !== "list") {
      const placed = leftColumnPlace();
      if (placed) layoutShown.holdLeftColumn({ left, placed, from: now });
    }
    if (handOff) handOff();
  }

  /**
   * Where the reader of the left column is, read by `renderColumns` before a change of layout draws that
   * column at another width: the place `readerPlace` reads there, and the view of the channel it was read
   * in. Null where the reader cannot see that column, behind the thread list (which holds its own place,
   * `layoutShown.holdListPlace`), or behind the call's view. `#269 approval-card-inline` x `#280 scroll-stability`.
   *
   * At a desk the left column is about the window's width in one column and half of it in two, so the
   * moment `data-columns` changes every row of its list is drawn at another width, and so is the send card
   * at its foot. A long draft opened by More was 519px tall in All at 1280x800 in Chromium and 580px beside
   * a thread. The change of layout read the reader's place only after that: Ctrl+4 at the foot of All,
   * card open, read a reader 61px above the foot, on two replies Main lacks, and so not on its newest line.
   * Main was put back where it was last left, 326px above its foot, with Send off screen and the jump
   * offered, and Ctrl+2 then left All 436px above its foot. A reader at the foot of Main itself, where no
   * view changes, was left 61px above it by the card growing under them, and one 12px up was left 109px up.
   * Read before the layout changes, the place is the one the reader had, and is kept where the left column
   * goes or stays (`changeChannelView`, `keepLeftColumnPlace`): at the foot, or on the row they were on.
   *
   * @returns {{place: ReturnType<typeof readerPlace>, view: string, channel: string} | null}
   */
  function leftColumnPlace(): LeftColumnPlace | null {
    if (deps.screen() !== "main" || deps.view() !== "discord" || layoutShown.threadList()) return null;
    return deps.inColumn(deps.mainColumn(), () => ({
      place: deps.readerPlace(),
      view: deps.selection().view,
      channel: String(deps.selectedChannelId()),
    }));
  }

  /**
   * Put the reader of the left column back where they were before the change of layout, `left`, where the
   * column is still on the view and channel it was read in, Main or All, and still on screen: into two
   * columns from Main, and out of them to Main. A view the change switched the column to has kept the place
   * itself (`changeChannelView`). `#269 approval-card-inline` x `#280 scroll-stability`.
   *
   * ...or, back to the layout the last change came from, where the reader was there (`back`,
   * `layoutShown.holdLeftColumn`), where the column is on the view that was read in: Main or All, whichever the way
   * back shows.
   *
   * @param {NonNullable<ReturnType<typeof leftColumnPlace>>} left
   * @param {ReturnType<typeof leftColumnPlace>} [back]
   */
  function keepLeftColumnPlace(left: LeftColumnPlace, back: LeftColumnPlace | null = null): void {
    deps.inColumn(deps.mainColumn(), () => {
      if (deps.screen() !== "main" || deps.view() !== "discord" || layoutShown.threadList()) return;
      if (deps.selection().view !== "main" && deps.selection().view !== "flat") return;
      const channel = String(deps.selectedChannelId());
      const kept = [back, left].find((one) => one !== null && one.view === deps.selection().view && one.channel === channel);
      if (kept) deps.keepReaderPlace(kept.place);
    });
  }

  /**
   * Whether the reader of the left column is where a change of layout left them (`placed`, read as it ended):
   * the same view of the same channel, and on its newest line now and then, or on the same row within a
   * pixel, or on the send card alone at the same height. `#269 approval-card-inline` x `#280 scroll-stability`.
   *
   * ...or on the same row lower down and now at the foot. What stands at the foot of the list settles after
   * the change — the jump offered there goes once the reader is near the foot, and its room with it — and a
   * list grown shorter under a reader is clamped to its foot, moving every row down. In Chromium at a desk a
   * reader 200px up All was at Main's foot in two columns, their row 11px lower than when `placed` was read,
   * and was taken to All's foot by Ctrl+2. Nothing the reader does moves the rows down and leaves them at
   * the foot.
   *
   * @param {NonNullable<ReturnType<typeof leftColumnPlace>>} now
   * @param {NonNullable<ReturnType<typeof leftColumnPlace>>} placed
   */
  function sameLeftPlace(now: LeftColumnPlace, placed: LeftColumnPlace): boolean {
    if (now.view !== placed.view || now.channel !== placed.channel) return false;
    const [a, b] = [now.place, placed.place];
    if (a.pinned && b.pinned) return true;
    const [x, y] = [a.seen[0], b.seen[0]];
    if (x && y) return x.ids.join(" ") === y.ids.join(" ") && (Math.abs(x.top - y.top) <= 1 || (a.pinned && x.top > y.top));
    return !x && !y && typeof a.card === "number" && typeof b.card === "number" && Math.abs(a.card - b.card) <= 1;
  }

  /**
   * Choose layout `layout`, keep the choice, and show it where it can be shown. Two columns chosen in a
   * window too narrow for them are kept for when it is wider, as the layout button always kept them.
   * `back` for a way back to the thread list — out of two columns by their button, out of a thread
   * opened from it — rather than a choice of it: what it was chosen from stays what Esc goes back to.
   *
   * @param {"one" | "list" | "two"} layout
   * @param {boolean} [back]
   */
  function setLayout(layout: LayoutName, back = false): void {
    // From the left column, whichever column asked: the layout changes what the left column shows.
    if (deps.columnSide()) return deps.inColumn(deps.mainColumn(), () => setLayout(layout, back));
    deps.noteDeepLinkReaderMove();
    if (layout === "list" && layoutChoice.chosen() !== "list" && !back) noteListBefore();
    layoutChoice.choose(layout);
    renderColumns();
    deps.renderThreadSelect();
  }

  /**
   * The thread list is being chosen: note what from, for Esc on it to go back to (`listBefore`). `#281
   * view-keys`. Two columns, and the layout their button went back to then; or one column and the view it
   * shows. Never a thread opened from the list itself, which the list is being come back to: the list's
   * Esc would be that thread, and the thread's the list, round and round.
   */
  function noteListBefore(): void {
    layoutChoice.noteListBefore(() => {
      if (deps.selection().view === "thread" && deps.selection().origin === "list") return null;
      const thread = deps.selection().view === "thread" && deps.selection().threadId ? String(deps.selection().threadId) : null;
      return {
        layout: "one", channel: String(deps.selectedChannelId() || ""),
        view: thread ? "thread" : deps.selection().view, thread, origin: thread ? deps.selection().origin : null,
      };
    });
  }


  /**
   * The one-column view Esc on the thread list goes back to: the one it was chosen from, in the channel
   * it was chosen in, while that view can still be shown; else the view the channel opens in — but never
   * a thread opened from the list, whose Esc is the list again, and in its place the channel's default.
   *
   * @param {ListBefore} before
   * @returns {{view: string, thread: string | null, summary: any, origin: string | null}}
   */
  function oneColumnBeforeList(before: ListBefore): OneColumnView {
    const channel = String(deps.selectedChannelId() || "");
    if (before.layout === "one" && before.channel === channel && deps.reopenedViewsIncludes(String(before.view))) {
      if (before.view !== "thread") return { view: String(before.view), thread: null, summary: null, origin: null };
      const id = String(before.thread || "");
      if (id !== "" && deps.threadStillKnown(id)) {
        const summary = deps.channelCanon().threads.find((held) => String(held.id) === id) || null;
        return { view: "thread", thread: id, summary, origin: before.origin || deps.defaultChannelView() };
      }
    }
    const opening = deps.openingChannelView();
    if (opening.view === "thread" && opening.origin === "list") {
      return { view: deps.defaultChannelView(), thread: null, summary: null, origin: null };
    }
    return opening;
  }

  /** Ctrl/Cmd+\: the layout button. */
  function columnsFromKeyboard(): boolean {
    if (!deps.channelOnScreen()) return false;
    toggleColumns();
    // The choice is kept as the button keeps it; a window too narrow to show it says so.
    if (layoutChoice.chosen() === "two" && !layoutShown.columns()) deps.setStatus(COLUMNS_TOO_NARROW);
    return true;
  }

  /** What a choice of two columns says in a window too narrow for them. */
  const COLUMNS_TOO_NARROW = "Two columns need a wider window; they come back when it is wider.";

  /** The reader pressed the layout button: two columns, or back to the layout they were chosen from. */
  function toggleColumns(): void {
    // Back to the thread list, out of two columns, is not a choice of it: its Esc still goes where it did.
    if (layoutChoice.chosen() === "two") setLayout(layoutChoice.beforeColumns(), true);
    else setLayout("two");
    deps.settleFocus();
  }

  /**
   * Show view `which` — "main", "flat", "threads" (the thread list) or "columns" — as Ctrl+1 to Ctrl+4
   * and the picker choose them. `#281 view-keys`. Main and All are one column, the left; the thread list
   * and two columns are layouts, kept as the layout button keeps two columns. Says why where the view
   * cannot be shown: no threads, a channel that is one thread, a window too narrow for two columns.
   *
   * @param {"main" | "flat" | "threads" | "columns"} which
   */
  function chooseView(which: string): void {
    deps.noteDeepLinkReaderMove();
    if (which !== "main" && !deps.threading()) {
      deps.setStatus("No threads here: Main is all of it.");
      deps.renderThreadSelect();
      return;
    }
    if ((which === "threads" || which === "columns") && !threadListOffered()) {
      deps.setStatus("This channel is a single thread: there is no list of threads to show.");
      deps.renderThreadSelect();
      return;
    }
    // `#277 reply-in-column`. A reply open in a column is put down as its Cancel puts it down, its words
    // kept as a draft, by any view but two columns, which keep the column it is in: the others take that
    // column away, or show the right one's cards. Not opened again by itself, as two columns going by the
    // layout button or a narrowed window open it (`leaveColumns`): the reader chose a view to read.
    if (deps.replyState().docked && which !== "columns") deps.closeReply();
    if (which === "threads") {
      setLayout("list");
      return;
    }
    if (which === "columns") {
      setLayout("two");
      if (!layoutShown.columns()) deps.setStatus(COLUMNS_TOO_NARROW);
      return;
    }
    chooseOneColumn({ view: which, thread: null, summary: null, origin: null });
  }

  /**
   * One column, showing `target`: the layout left for it, and the view asked for shown and kept as the
   * channel's choice, as a choice made in one column always was.
   *
   * @param {{view: string, thread: string | null, summary: any, origin: string | null}} target
   */
  function chooseOneColumn(target: OneColumnView): void {
    if (deps.columnSide()) return deps.inColumn(deps.mainColumn(), () => chooseOneColumn(target));
    deps.noteDeepLinkReaderMove();
    layoutShown.askOneColumn(target);
    setLayout("one");
    // Already one column: no change of layout showed it.
    const left = layoutShown.takeOneColumn();
    if (left) showOneColumnView(left, true);
  }


  /**
   * Put the left column on `target`, now that it is one column again. `record` for a view the reader
   * asked for, which becomes the channel's choice; otherwise the view the channel opens in, put back
   * without recording it again, which would turn a channel's default into a choice nobody made.
   *
   * `place`: where the reader of the left column was before the layout changed, for the view it goes to
   * (`changeChannelView`); else read there as it goes. `#269 approval-card-inline` x `#280 scroll-stability`.
   *
   * @param {{view: string, thread: string | null, summary: any, origin: string | null}} target
   * @param {boolean} record
   * @param {ReturnType<typeof readerPlace> | null} [place]
   */
  function showOneColumnView(target: OneColumnView, record: boolean, place: ReaderPlace | null = null): void {
    const thread = target.thread === null ? null : String(target.thread);
    const same = target.view === deps.selection().view && thread === (deps.selection().threadId ? String(deps.selection().threadId) : null);
    if (same && !(record && deps.pinsView().only)) {
      if (target.origin) deps.selection().origin = target.origin;
      if (record) recordViewChoice();
      // `#304 back-gesture-in-app`. A thread's way back can be the thread list now, a level more.
      deps.scheduleBackSync();
      return;
    }
    const show = () => deps.changeChannelView(target.view, thread, target.summary, target.origin, place);
    if (record) {
      deps.guardQuietly(show)();
      return;
    }
    layoutShown.whileRestoring(deps.guardQuietly(show));
  }

  /** Keep the view on screen as its channel's choice, in one column, where nothing switched views to do it. */
  function recordViewChoice(): void {
    if (sideColumnUp() || deps.columnSide()) return;
    const chosen = deps.viewOnScreen();
    if (chosen) deps.rememberView(chosen);
    deps.settleUiState();
    deps.saveUiState();
  }

  /**
   * The left column on Main, as it is whenever the right column is up — beside it, or behind the thread
   * list on its own. Not recorded as the channel's choice: it is the layout's, not the reader's.
   *
   * `place`: where its reader was before the layout changed, for Main to keep (`leftColumnPlace`); else
   * read as Main comes. `#269 approval-card-inline` x `#280 scroll-stability`.
   *
   * @param {ReturnType<typeof readerPlace> | null} [place]
   */
  function leftColumnToMain(place: ReaderPlace | null = null): void {
    if (deps.selection().view === "main") return;
    if (deps.configApplied()) {
      deps.guardQuietly(() => deps.changeChannelView("main", null, null, null, place))();
    } else {
      // A reopen, before sign-in has answered: Main drawn from what this device saved, with no read.
      // Nothing is read on a token nobody has proved; the reopen's own read comes once it is.
      deps.selection().view = "main";
      deps.selection().threadId = null;
      deps.selection().thread = null;
      deps.hydrateChannelScope();
      deps.restoreChannelComposer();
    }
  }

  /**
   * Into two columns: Main on the left, drawn from what the page holds, and on the right the thread the
   * left column was showing, else the one selected there earlier in this session, else the cards — as
   * the thread list left them, coming from it (`#281 view-keys`).
   *
   * `left`: where the reader of the left column was in one column (`leftColumnPlace`), which Main keeps.
   *
   * @param {"one" | "list" | "two"} from
   * @param {ReturnType<typeof leftColumnPlace>} [left]
   */
  function enterColumns(from: LayoutName = "one", left: LeftColumnPlace | null = null): void {
    layoutShown.setColumns(true);
    const channel = String(deps.selectedChannelId());
    const open = deps.selection().view === "thread" && deps.selection().threadId
      ? { id: String(deps.selection().threadId), summary: deps.selection().thread }
      : sideThreads.get(channel) || null;
    leftColumnToMain(left ? left.place : null);
    deps.renderThreadSelect();
    if (open) deps.guardQuietly(() => deps.selectSideThread(open.id, open.summary))();
    else if (from === "list") renderSideMode();
    else deps.showSideList();
    // `#262 search-load-older`. Not one column any more: Search All goes, even where the left column was
    // Main already and nothing above searched it again.
    deps.renderSearchInAll();
  }

  /**
   * Out of two columns: to the view last chosen in one column, or the one asked for (`layoutShown.askOneColumn`);
   * or to the thread list, the left column staying Main behind it. The right column keeps what it shows,
   * held, so two columns come back to it at once.
   *
   * `left`: where the reader of the left column was in two columns (`leftColumnPlace`), which the view
   * shown in one keeps.
   *
   * @param {"one" | "list" | "two"} next
   * @param {ReturnType<typeof leftColumnPlace>} [left]
   * @returns {(() => void) | null} The hand-off of a reply that was open in a column, to run once the
   *   next layout is drawn; else null.
   */
  function leaveColumns(next: LayoutName = "one", left: LeftColumnPlace | null = null): (() => void) | null {
    // `#410 inline-reply-editor`. A reply open in the stream is put away, as a draft, before the views
    // change under it, and opened again in the stream once the layout that follows is drawn, where its
    // message is drawn (`reopenPendingReply`): what this returns, which `moveLayout` runs then — over one
    // column, or over the thread list (`#281 view-keys`). Where its message is not drawn, it waits for it.
    const replying = deps.replyState().docked;
    // `#314 reply-focus-flake`. Or had it until the browser took it, the column no longer drawn.
    const typing = replying && (deps.nodeWithin(deps.el("reply-compose"), document.activeElement) || (deps.replyState().focusTaken && deps.focusLost()));
    const target = deps.replyState().target;
    const waiting = replying && target ? { id: String(target.id), typing, ready: false } : null;
    if (replying) deps.closeReply({ restore: false, refocus: false });
    if (waiting) deps.replyState().pending = waiting;
    // `#262 search-load-older`. The right column's walk for the search ends: its list is put aside, held.
    deps.endSearchWalk(deps.sideColumn());
    layoutShown.setColumns(false);
    // ...and the cards' walk, unless the cards stay, across the window as the thread list.
    if (next !== "list") deps.endRepliesWalk();
    if (deps.sideColumn().mode === "thread") {
      deps.inColumn(deps.sideColumn(), () => {
        deps.dropReplyArrowTap();
        deps.putRepliesBack();
        deps.rememberChannelDraft();
        deps.rememberChannelContext();
      });
    }
    deps.renderThreadSelect();
    if (next === "one") showOneColumnLeft(left ? left.place : null);
    // `#262 search-load-older`. One column again: on Main, where the search still matches nothing, Search
    // All is offered at once, not at the next keystroke. Back to Main, nothing above searched it again.
    deps.renderSearchInAll();
    return replying ? () => {
      const pending = deps.replyState().pending;
      if (pending) pending.ready = true;
      deps.reopenPendingReply();
      // Still waiting: when it opens later, the keys stay where the reader has them by then.
      const still = deps.replyState().pending;
      if (still) still.typing = false;
    } : null;
  }

  /**
   * The left column, one column again: on the view asked for with the layout, else the one the channel
   * opens in. `place`: where its reader was before the layout changed (`showOneColumnView`).
   *
   * @param {ReturnType<typeof readerPlace> | null} [place]
   */
  function showOneColumnLeft(place: ReaderPlace | null = null): void {
    const wanted = layoutShown.takeOneColumn();
    if (wanted) {
      showOneColumnView(wanted, true, place);
      return;
    }
    const opening = deps.openingChannelView();
    showOneColumnView({ view: opening.view, thread: opening.thread, summary: opening.summary, origin: null }, false, place);
  }

  /**
   * Into the thread list: the right column on its own, its cards across the window, over the left
   * column, which goes to Main behind it as it does for two columns. `#281 view-keys`. A thread the right
   * column showed in two columns is put aside, held, for two columns to come back to; the list shows
   * cards, always.
   *
   * @param {"one" | "list" | "two"} from
   * @param {ReturnType<typeof leftColumnPlace>} [left]
   */
  function enterThreadList(from: LayoutName = "one", left: LeftColumnPlace | null = null): void {
    layoutShown.setThreadList(true);
    // `#280 scroll-stability`. Where the reader of the left column is, read while a card is still at the
    // foot of its list, for the way out of the list (`layoutShown.holdListPlace`). `#269 approval-card-inline`: read
    // before the layout changed (`left`, `leftColumnPlace`), where it did: from two columns, Main is drawn
    // across the window behind the list, its rows and the card at another width than the reader saw.
    layoutShown.holdListPlace(deps.inColumn(deps.mainColumn(), () => (deps.selection().view === "main" || deps.selection().view === "flat"
      ? { place: left ? left.place : deps.readerPlace(), view: deps.selection().view, channel: String(deps.selectedChannelId()) } : null)));
    // `#269 approval-card-inline`. A send card waiting at the foot of the left column, behind the list
    // now, is docked under the cards (`placePostCard`) BEFORE they are opened or put back, so that the
    // foot they are placed at is the one above it.
    deps.transcriptPlacePostCard();
    leftColumnToMain();
    if (from === "two" && deps.sideColumn().mode === "list") {
      // The same cards, where the reader was in them.
      renderSideMode();
    } else {
      deps.showSideList();
    }
    deps.renderSearchInAll();
    deps.refreshSearchCount();
  }


  /** Show what the right column is showing — the cards or the thread — and say so in its heading. */
  function renderSideMode(): void {
    // `#338 column-overscroll`. The room past the foot was the rows that were shown: what comes lands on its foot.
    deps.closeOverscroll();
    const thread = deps.sideColumn().mode === "thread";
    if (deps.leftEl("side-list").hidden === !thread) deps.leftEl("side-list").hidden = thread;
    if (deps.leftEl("side-pane").hidden === thread) deps.leftEl("side-pane").hidden = !thread;
    if (deps.leftEl("side-close").hidden === thread) deps.leftEl("side-close").hidden = !thread;
    // `#298 thread-card-header`. Over a thread, the heading is the thread's card, and the words "Thread
    // list" go. Shown here, before the thread is drawn (`selectSideThread` lands it after this returns),
    // so the list under it is already the height the landing measures; the card is a fixed height, so
    // nothing it is filled with later changes that.
    const card = deps.leftEl("side-thread-card");
    if (card.hidden === thread) card.hidden = !thread;
    if (deps.leftEl("side-title").hidden !== thread) deps.leftEl("side-title").hidden = thread;
    // The jump to a thread's newest message is the thread's: over the cards it would jump in a list that
    // is not on screen. A thread selected again decides it afresh (`renderJumpNewest`).
    if (!thread && !deps.leftEl("side-jump-newest").hidden) deps.leftEl("side-jump-newest").hidden = true;
    deps.renderUnpackToggle();
    deps.renderMainToggle();
    deps.renderOrderToggle();
    // `#262 search-load-older`. A thread selected over the cards ends their walk for the search, aborting
    // its read; the cards back are offered it again (`renderSideCards`).
    if (thread) deps.renderRepliesSearch();
    renderSideColumn();
    // `#419 threads-view-main-messages`. The main channel's box at the foot of the cards, or back in its place.
    deps.placeMainCard();
    // What the links join, and what the glide follows, is the card at the top over a thread and the
    // cards in the list otherwise: a change of what the column shows is a new selection for both.
    deps.noteLinkedSelection();
    // `#304 back-gesture-in-app`. A thread on the right is a level; the cards are not.
    deps.scheduleBackSync();
  }

  /**
   * The right column's heading, and with no thread selected its cards. Drawn only where they changed.
   *
   * `#231 two-column-polish`. The scroller is the column's one named region, and its name is the
   * heading's: "Thread list" (`#281 view-keys`: what the picker and the keys call it), or "Thread: " and
   * the thread's name, so a screen reader landing in it says which thread it is in, once.
   *
   * `#298 thread-card-header`. Over a thread the heading is the thread's card (`paintThreadCard`), saying
   * what its card in the list said; the region keeps the thread's name, in the picker's words.
   */
  function renderSideColumn(): void {
    if (!sideColumnUp()) return;
    if (deps.sideColumn().mode === "list") {
      deps.renderSideFreshness();
      deps.setTextIfChanged(deps.leftEl("side-title"), "Thread list");
      deps.nameSideRegion("Thread list");
      deps.renderSideCards();
      return;
    }
    deps.inColumn(deps.sideColumn(), () => {
      deps.renderSideFreshness();
      const open = sideThreadChoice();
      const name = open ? deps.threadName(open, Infinity) : "Thread";
      deps.nameSideRegion(open ? `Thread: ${name}` : "Thread");
      deps.paintThreadCard(deps.leftEl("side-thread-card"), open, deps.replyTally());
    });
  }

  /**
   * The thread the right column shows, as the store names it: the one selected there (`sideThreads`),
   * whether or not its rows are drawn yet — `renderSideMode` asks before `selectSideThread` draws them —
   * else the one its rows are of. Null with neither.
   */
  function sideThreadChoice(): unknown {
    const chosen = sideThreads.get(String(deps.selectedChannelId()));
    return deps.inColumn(deps.sideColumn(), () => {
      if (!chosen) return deps.openThreadChoice();
      const drawn = deps.selection().view === "thread" && String(deps.selection().threadId) === chosen.id;
      return deps.knownThreadChoice(chosen.id, drawn && deps.selection().thread ? deps.selection().thread : chosen.summary);
    });
  }

  /** Name the right column's region `name`, unless it is already called that. */


  return {
    layoutChoice, layoutShown, sideColumnUp, threadListOnScreen, sideThreads, sideShowing,
    COLUMNS_TOO_NARROW, inThisColumn, columnSteps, eachColumn, inOtherColumn, followInOtherColumn,
    threadListOffered, columnsOffered, renderColumnToggle, sideHasBox, toggleHasBox, renderColumns,
    moveLayout, leftColumnPlace, keepLeftColumnPlace, sameLeftPlace, setLayout, noteListBefore,
    oneColumnBeforeList, columnsFromKeyboard, toggleColumns, chooseView, chooseOneColumn,
    showOneColumnView, recordViewChoice, leftColumnToMain, enterColumns, leaveColumns,
    showOneColumnLeft, enterThreadList, renderSideMode, renderSideColumn, sideThreadChoice,
  };
}

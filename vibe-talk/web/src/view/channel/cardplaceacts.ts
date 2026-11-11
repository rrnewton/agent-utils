// Leaving the thread list, and the card the reader was held by (`#281 view-keys`, `#612
// view-stability`, `#261 two-column-connectors`; `#624 voice-page-modularity`, moved from the
// legacy script): Esc on the list, opening a thread from it, leaving it, and the reader's place
// in the cards kept across the cards being drawn again. The places themselves are
// `createCardPlaces`' (cardplace.ts, untouched); every scroll write is the page's, handed in as
// the exact acts the script called, so a later scroll owner adopts them unchanged. Words, order
// and timing are as they were in the script.

import type { CardPlace, CardPlaces } from "./cardplace.ts";

/** A place held while the thread list is up, as `leaveThreadList` reads it. */
export interface HeldListPlace {
  readonly channel: string;
  readonly view: string;
  readonly place: { readonly pinned: boolean };
}

/** What the card-place acts ask of the page. */
export interface CardPlaceActsDependencies {
  // The thread list's way back and in.
  inSideColumn(): boolean;
  inMainColumn<T>(act: () => T): T;
  listBefore(): { layout: string; columns?: unknown };
  columnsFit(): boolean;
  restoreBeforeColumns(columns: unknown): void;
  setLayout(layout: string): void;
  chooseOneColumn(place: unknown): void;
  oneColumnBeforeList(before: { layout: string; columns?: unknown }): unknown;
  settleFocus(): void;
  threadSummary(id: string): unknown;
  threadListOnScreen(): boolean;
  cardFlightStart(id: string): { readonly left: number; readonly top: number } | null;
  flyThreadCard(card: HTMLElement, from: { readonly left: number; readonly top: number } | null, pane: HTMLElement): void;
  leftById(id: string): HTMLElement;
  // Leaving the list.
  postCardShown(): boolean;
  mainFollows(): boolean;
  selectionKey(): string;
  takeListPlace(): HeldListPlace | null;
  setThreadList(on: boolean): void;
  renderMainToggle(): void;
  renderOrderToggle(): void;
  placeMainCard(): void;
  endRepliesWalk(): void;
  placePostCard(): void;
  placeHolds(held: HeldListPlace, view: string): boolean;
  keepReaderPlace(place: { readonly pinned: boolean }): boolean;
  scrollToNewest(): void;
  showOneColumnLeft(): void;
  followIfPinned(): void;
  renderSearchInAll(): void;
  refreshSearchCount(): void;
  // The reader's place in the cards.
  makeCardPlaces(): CardPlaces;
  selectedChannelId(): unknown;
  takeListCards(): void;
  closeOverextend(): void;
  setSideListMode(): void;
  whileOpeningCards(act: () => void): void;
  renderSideMode(): void;
}

/** The card-place acts and the state they own. */
export interface CardPlaceActs {
  threadListBack(): undefined;
  openThreadFromList(id: string, summary?: unknown): void;
  leaveThreadList(next?: string): void;
  cardAnchor(): CardPlace | null;
  placeCardAnchor(anchor: CardPlace | null, follow?: boolean): boolean;
  keepingCardPlace(change: () => void, follow?: boolean): void;
  rememberCardPlace(): void;
  openSideCards(): void;
  placeOpenedCards(): void;
}

/** Make the card-place acts. */
export function createCardPlaceActs(deps: CardPlaceActsDependencies): CardPlaceActs {
  /**
   * Esc on the thread list, with nothing smaller open: back to what the list was chosen from
   * (`listBefore`). `#281 view-keys`. The owner, of the Threads screen it replaced: "rather ESC works".
   * Two columns, where the window still draws them, their button going back to what it went back to
   * then; else one column, on the view it showed (`oneColumnBeforeList`).
   */
  function threadListBack(): undefined {
    if (deps.inSideColumn()) return deps.inMainColumn(() => threadListBack());
    const before = deps.listBefore();
    if (before.layout === "two" && deps.columnsFit()) {
      deps.restoreBeforeColumns(before.columns);
      deps.setLayout("two");
    } else {
      deps.chooseOneColumn(deps.oneColumnBeforeList(before));
    }
    deps.settleFocus();
    return undefined;
  }

  /**
   * Open thread `id` from the thread list: across the window, one column, and Back from it is the list
   * again (`closeThread`). `#281 view-keys`. A card, the picker, and Enter on a card all come here.
   */
  function openThreadFromList(id: string, summary: unknown = null): void {
    const known = summary || deps.threadSummary(id) || null;
    // `#298 thread-card-header`. Its card moves up from the list to the top of the window.
    const from = deps.threadListOnScreen() ? deps.cardFlightStart(String(id)) : null;
    deps.chooseOneColumn({ view: "thread", thread: String(id), summary: known, origin: "list" });
    deps.flyThreadCard(deps.leftById("thread-card"), from, deps.leftById("pane-discord"));
  }

  /**
   * Out of the thread list: to one column — the view asked for, or the one the channel opens in — or to
   * two columns, Main coming in beside the cards as they are. `#281 view-keys`.
   */
  function leaveThreadList(next = "one"): void {
    // `#269 approval-card-inline`. A send card docked under the cards goes back to the foot of the left
    // column's list, its next message, BEFORE that column's view is decided, so that a view drawn afresh
    // (All, a thread) is placed with it in. Main kept behind the list, which nothing redraws, is told as a
    // message arriving tells it (`#280 scroll-stability`): a reader on its newest line follows the card
    // down, and anyone else stays on their row and is offered the jump to it.
    //
    // ...and told BEFORE that view is decided, since a switch between Main and All keeps the place Main's
    // reader is at (`changeChannelView`). Told only after, and only where the view stayed, a reader at
    // All's foot was drawn in All on the rows they could see above the card: in Chromium 369px above All's
    // foot on a phone and 337px at a desk, Send off screen. The place read as the list came up, with the
    // card still in Main's list, is what is put back, where it still holds (`layoutShown.holdListPlace`): the card's
    // going moved a reader near the foot.
    //
    // ...and a place read in All is put back in All as well, card or no card, once the way out has shown
    // All, where every row the reader could see is. Main lacks All's replies, so a place near All's foot
    // put back in Main is at Main's foot, and the switch to All took that for a reader on the newest line
    // and went to All's foot. A reader up All is offered the jump to a card come back below them, as one
    // up Main is.
    const carded = deps.postCardShown();
    const follows = carded && deps.mainFollows();
    const shown = deps.selectionKey();
    const held = deps.takeListPlace();
    deps.setThreadList(false);
    // `#419 threads-view-main-messages`. The main channel's box back under the left column's list.
    deps.renderMainToggle();
    deps.renderOrderToggle();
    deps.placeMainCard();
    // `#262 search-load-older`. The cards' walk for the search ends with the list, unless two columns
    // keep the cards beside Main.
    if (next !== "two") deps.endRepliesWalk();
    deps.placePostCard();
    const followed = deps.inMainColumn(() => {
      const holds = held !== null && deps.placeHolds(held, "main");
      if (holds && held && deps.keepReaderPlace(held.place)) return held.place.pinned;
      if (follows) deps.scrollToNewest();
      return follows;
    });
    if (next === "one") deps.showOneColumnLeft();
    const backInAll = next === "one" && held !== null && held.view === "flat" && deps.inMainColumn(() =>
      deps.placeHolds(held, "flat") && deps.keepReaderPlace(held.place));
    if (carded && (backInAll ? !(held as HeldListPlace).place.pinned : !followed && deps.selectionKey() === shown)) {
      deps.inMainColumn(() => deps.followIfPinned());
    }
    deps.renderSearchInAll();
    deps.refreshSearchCount();
  }

  /** Where the reader is in the cards, and putting them back once the cards are drawn again. */
  const cardPlaces = deps.makeCardPlaces();

  /**
   * Where the reader is in the cards, or null with the cards not on screen, or being opened.
   */
  function cardAnchor(): CardPlace | null {
    return cardPlaces.anchor();
  }

  /**
   * Put the reader back on `anchor` in the cards; whether the reader was placed. With `follow`, a place the
   * reader chose to come back to, the card wherever it now is.
   */
  function placeCardAnchor(anchor: CardPlace | null, follow = false): boolean {
    return cardPlaces.place(anchor, follow);
  }

  /**
   * Run `change` to the cards, keeping the reader's place in them. With `follow`, a change the reader asked
   * for that re-sorts the cards: the card they are on is kept wherever it goes.
   */
  function keepingCardPlace(change: () => void, follow = false): void {
    cardPlaces.keeping(change, follow);
  }

  /**
   * By channel, where the reader was in the cards when a thread was selected from them, for this
   * session: closing the thread comes back there. `#261 two-column-connectors`.
   */
  const sideCardPlaces = new Map<string, { newest: boolean; id: string | null; offset: number }>();

  /** Note where the reader is in the cards, before the right column shows something else. */
  function rememberCardPlace(): void {
    const anchor = cardAnchor();
    if (anchor) sideCardPlaces.set(String(deps.selectedChannelId()), anchor);
  }

  /** Show the cards in the right column, the reader placed in them as `placeOpenedCards` says. */
  function openSideCards(): void {
    // A place held for the way back to the thread list is not one in cards opened afresh (`layoutShown.holdListCards`).
    deps.takeListCards();
    // Nor the room past the foot held for a card packed in them (`#459 pack-keeps-card-still`).
    deps.closeOverextend();
    deps.setSideListMode();
    deps.whileOpeningCards(() => deps.renderSideMode());
    placeOpenedCards();
  }

  /**
   * The cards just opened: at the foot, as a list of messages opens at its newest, unless the reader was
   * up the list when a thread was selected from it — then back on the card they were on.
   */
  function placeOpenedCards(): void {
    const anchor = sideCardPlaces.get(String(deps.selectedChannelId()));
    sideCardPlaces.delete(String(deps.selectedChannelId()));
    // The foot, too, when the card the reader was on has gone.
    if (!(anchor && !anchor.newest && placeCardAnchor(anchor, true))) placeCardAnchor({ newest: true, id: null, offset: 0 });
  }

  return {
    threadListBack,
    openThreadFromList,
    leaveThreadList,
    cardAnchor,
    placeCardAnchor,
    keepingCardPlace,
    rememberCardPlace,
    openSideCards,
    placeOpenedCards,
  };
}

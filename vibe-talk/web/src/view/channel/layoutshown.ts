// Which layout of the channel is on screen (`#229 desktop-two-column`, `#281 view-keys`, `#280
// scroll-stability` x `#269 approval-card-inline`; `#624 voice-page-modularity`, moved from the legacy script
// into view/channel): one column, two columns, or the thread list on its own over the left column; whether
// this window draws two columns; the one-column view asked for with the layout; whether the page itself is
// putting a view back; and the reader's places held across a change of layout for the way back. Each place
// is held until the change that spends it takes it. Drawing the layouts, and reading and putting back the
// places, are the page's (`renderColumns`, `moveLayout`).

/** The channel's layouts: one column, the thread list over it, or two columns. */
export type Layout = "one" | "list" | "two";

/** The layout's commands and questions, over the page's records of the view asked for and of each place. */
export interface LayoutShown<Wanted, ListPlace, CardsPlace, LeftPlace> {
  /** Whether two columns are on screen now. */
  columns(): boolean;
  setColumns(on: boolean): void;
  /** Whether the thread list is on screen now, on its own over the left column. `#281 view-keys`. */
  threadList(): boolean;
  setThreadList(on: boolean): void;
  /** Whether the right column is on screen: beside Main, or on its own as the thread list. */
  sideUp(): boolean;
  /** The layout on screen now. */
  layout(): Layout;
  /**
   * Whether this window draws two columns: the layout button has a box, which web/voice.css gives it only on
   * a desk at least 1000px wide. Asked where `renderColumns` decides, never per redraw, because asking lays
   * the page out; the picker offers two columns by it.
   */
  fit(): boolean;
  /** The window was found to draw two columns or not: whether that changed the answer. */
  noteFit(fit: boolean): boolean;
  /**
   * The one-column view a key or the picker asked for with the layout, for the change of layout to show
   * instead of the view the channel opens in.
   */
  askOneColumn(target: Wanted): void;
  /** That view, taken by whatever shows it, or null where none was asked for or it was shown already. */
  takeOneColumn(): Wanted | null;
  /**
   * True while leaving two columns puts back the view last chosen in one: that is the reader's choice
   * already, and recording it again would turn a channel's default into a choice nobody made.
   */
  restoring(): boolean;
  /** Run `fn` as the page putting a view back (`restoring`), and stop however it returns. */
  whileRestoring(fn: () => void): void;
  /**
   * Where the reader of the left column was as the thread list came up, read by `readerPlace` on Main or
   * All, with a send card at the foot of that column's list still in it, before it is docked; in which of
   * the two views, and on which channel. Null when the left column showed neither.
   *
   * Main, behind the list, cannot keep that place itself. In Chromium the card stands 396px tall at the
   * foot of a phone's list and 246px docked, so as it goes, a reader nearer the foot than the difference
   * is left at the foot of a list now too short to keep them where they were; and the view keys'
   * switches between Main and All keep the place they measure, with the card gone from the list on the
   * way in and back in it on the way out. So the place is put back as the card comes back
   * (`leaveThreadList`), before the left column's view is decided.
   *
   * ...and put back in All as well, card or no card, when it was read in All and All is what the way out
   * shows: Esc, the list having been chosen from All, or Ctrl+2. All is Main behind the list, and Main
   * lacks All's replies, so a place near All's foot was clamped at Main's, and the way back read Main's
   * reader as on its newest line and took All to its foot. In Chromium every reader 30 to 175px up All
   * came back at its foot without a card, and with one, 200 and 225px up too.
   */
  holdListPlace(place: ListPlace | null): void;
  /** That place, taken by the way out of the thread list. */
  takeListPlace(): ListPlace | null;
  /**
   * Where the reader was in the cards of the thread list, held while two columns brought from it are up,
   * for the way back: `place`, read in the list, and `placed`, where two columns' cards then put them; on
   * which channel. Set as two columns come from the list, spent by the next change of layout, and let go
   * as the cards are opened afresh.
   *
   * A send card docked under the list takes its height from the foot of the cards, so their column is
   * that much shorter there than beside Main, where the card is back at the foot of Main's list. A reader
   * up the cards nearer their foot than that is at the foot in two columns, which is only geometry, as
   * Main behind the cards is too short to keep a reader near its foot. But the way back read them as at
   * the foot, and put them at the list's foot too. In Chromium at a desk, the long draft's card 246px tall,
   * every reader 60 to 300px up the cards came back at the list's foot by Ctrl+4 and Ctrl+3: one 300px up
   * kept their card in two columns, 10px from the foot, inside the slack, and moved about 290px. So the
   * place read in the list is put back on the way back, where the reader left the cards where two columns
   * put them; one who moved them is placed where they are, as before.
   */
  holdListCards(held: CardsPlace): void;
  /** That place, taken by the next change of layout, or let go (the cards opened afresh). */
  takeListCards(): CardsPlace | null;
  /**
   * Where the reader of the left column was before the last change between one column and two (`left`,
   * read by `renderColumns`), and where that change left them (`placed`), for the way back: the change
   * back to layout `from` puts them where they were there, unless they have moved since. Set by each
   * change between one column and two, spent by the next change of layout.
   *
   * A change of layout keeps the place it reads, but not always where it goes, and a place it could not
   * keep was lost on the way back. Main lacks All's replies, so a reader up All near its foot is at Main's
   * foot in two columns, which is only geometry; but Ctrl+2 then read them as on Main's newest line and
   * took All to its foot: in Chromium at a desk a reader 40px up All with a long draft opened by More, and
   * one 30px up with no draft at all. And the card at the foot is taller in two columns' narrower left
   * column, so a reader 40px up Main there was at Main's foot in one column by Ctrl+1, and Ctrl+4 kept them
   * at the foot. So the place read before is put back on the way back where the reader left the column as
   * the change left it (`keepLeftColumnPlace`); one who moved it is placed where they are now.
   */
  holdLeftColumn(held: LeftPlace): void;
  /** That place, taken by the next change of layout. */
  takeLeftColumn(): LeftPlace | null;
}

/** One column, in a window not yet asked whether it draws two, nothing asked for or held. */
export function createLayoutShown<Wanted, ListPlace, CardsPlace, LeftPlace>(): LayoutShown<Wanted, ListPlace, CardsPlace, LeftPlace> {
  let columnsShown = false;
  let threadListShown = false;
  let columnsFit = false;
  let oneColumnWanted: Wanted | null = null;
  let columnsRestoring = false;
  let listHeldPlace: ListPlace | null = null;
  let listCardsHeld: CardsPlace | null = null;
  let leftColumnHeld: LeftPlace | null = null;

  return {
    columns: () => columnsShown,
    setColumns(on) {
      columnsShown = on;
    },
    threadList: () => threadListShown,
    setThreadList(on) {
      threadListShown = on;
    },
    sideUp: () => columnsShown || threadListShown,
    layout: () => (columnsShown ? "two" : threadListShown ? "list" : "one"),
    fit: () => columnsFit,
    noteFit(fit) {
      const changed = fit !== columnsFit;
      columnsFit = fit;
      return changed;
    },
    askOneColumn(target) {
      oneColumnWanted = target;
    },
    takeOneColumn() {
      const wanted = oneColumnWanted;
      oneColumnWanted = null;
      return wanted;
    },
    restoring: () => columnsRestoring,
    whileRestoring(fn) {
      columnsRestoring = true;
      try {
        fn();
      } finally {
        columnsRestoring = false;
      }
    },
    holdListPlace(place) {
      listHeldPlace = place;
    },
    takeListPlace() {
      const held = listHeldPlace;
      listHeldPlace = null;
      return held;
    },
    holdListCards(held) {
      listCardsHeld = held;
    },
    takeListCards() {
      const held = listCardsHeld;
      listCardsHeld = null;
      return held;
    },
    holdLeftColumn(held) {
      leftColumnHeld = held;
    },
    takeLeftColumn() {
      const held = leftColumnHeld;
      leftColumnHeld = null;
      return held;
    },
  };
}

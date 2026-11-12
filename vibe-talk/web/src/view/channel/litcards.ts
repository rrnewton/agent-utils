// One card highlighted (`#277 thread-card-focus`; `#624 voice-page-modularity`, moved from the
// legacy script): by channel, the thread whose card is highlighted for this session, how a card
// is drawn as it stands, and the cards' own acts — a press, the way in, and Esc putting it out.
// Entering a thread is not this owner's: `enterCard` only calls the page's entry acts, handed
// in, and marks the side column as entered from the cards through a handed-in act. Words, order
// and timing are as they were in the script.

import { childByClass } from "../dom/index.ts";

/** A list the keys move through, as the lit cards read it (view/shell `KeyPane`). */
export interface LitCardPane {
  scroller(): HTMLElement;
}

/** What the lit cards ask of the page. */
export interface LitCardsDependencies {
  /** Whether the Threads view is up, where a press packs and unpacks instead of highlighting. */
  cardsCycle(): boolean;
  selectedChannelId(): unknown;
  leftById(id: string): HTMLElement;
  noteLinkedSelection(): void;
  rowShown(row: unknown): boolean;
  sidePane(): LitCardPane;
  channelPane(): LitCardPane;
  fitOverscroll(scroller: HTMLElement): void;
  /** Move the scroller by a measured delta, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(scroller: HTMLElement, delta: number): void;
  bringIntoView(pane: LitCardPane, row: HTMLElement): void;
  noteOverscrollReader(scroller: HTMLElement): void;
  sayRowShortcut(button: HTMLElement, className: string): void;
  // Entering a thread, the page's acts (`#1082` holds the entry itself).
  openThread(id: string, summary: unknown): unknown;
  openThreadFromList(id: string, summary: unknown): unknown;
  selectSideThread(id: string, summary: unknown): unknown;
  markSideFromCards(): void;
  threadListShown(): boolean;
  sideShowing(): boolean;
  focusList(pane: LitCardPane): void;
  columnLinksShown(): boolean;
  sideMode(): string;
  channelOnScreen(): boolean;
  sideColumnUp(): boolean;
  inMainColumn<T>(act: () => T): T;
  selectionView(): string;
  pinsOnly(): boolean;
  paneFor(target: unknown): LitCardPane | null;
  nodeWithin(root: unknown, node: unknown): boolean;
  keyRow(target: unknown): HTMLElement | null;
  rowInView(pane: LitCardPane, row: HTMLElement): boolean;
}

/** The lit cards' questions and acts. */
export interface LitCards {
  litCard(): string | null;
  cardThread(row: HTMLElement): string;
  paintLitCard(row: HTMLElement, id: string): void;
  paintLitCards(): void;
  toggleLitCard(id: string, row: HTMLElement): void;
  cardOpener(prefix: string, replies: string, unread: string, open: () => void): HTMLButtonElement;
  cardCountParts(prefix: string, replies: string, unread: string, empty?: string): Node[];
  enterCard(id: string, summary: unknown, row: HTMLElement): unknown;
  cardRowOnRight(id: string | null): HTMLElement | null;
  litCardOnRight(): string | null;
  cardsOnScreen(): HTMLElement | null;
  cardForArrow(target: HTMLElement): HTMLElement | null;
  putOutLitCard(): boolean;
}

/** Make the lit cards: no card highlighted in any channel yet. */
export function createLitCards(deps: LitCardsDependencies): LitCards {
  /**
   * By channel, the thread whose card is highlighted, for this session.
   */
  const litCards = new Map<string, string>();

  /** The thread whose card is highlighted in the channel on screen, or null. */
  function litCard(): string | null {
    // `#440 thread-card-pack-toggle`. The Threads view highlights no card: a press there packs and unpacks.
    if (deps.cardsCycle()) return null;
    return litCards.get(String(deps.selectedChannelId())) || null;
  }

  /** The thread a card of either list stands for. */
  const cardThread = (row: HTMLElement): string =>
    String(row.getAttribute(row.hasAttribute("data-card") ? "data-id" : "data-context-id"));

  /**
   * Draw `row`, the card of thread `id`, highlighted or not: `data-highlighted` on it, which the stylesheet
   * draws, and its button saying whether the card is expanded. Written only where it changes.
   */
  function paintLitCard(row: HTMLElement, id: string): void {
    const lit = litCard() === id;
    if (row.hasAttribute("data-highlighted") !== lit) {
      if (lit) row.setAttribute("data-highlighted", "true");
      else row.removeAttribute("data-highlighted");
    }
    const button = childByClass(row, row.hasAttribute("data-card") ? "side-card" : "thread-open");
    // In the Threads view, whether the card is shown whole, unpacked or not (`#440 thread-card-pack-toggle`).
    const whole = deps.cardsCycle() && row.hasAttribute("data-card") ? row.getAttribute("data-card-state") !== "collapsed" : lit;
    const expanded = whole ? "true" : "false";
    if (button && button.getAttribute("aria-expanded") !== expanded) button.setAttribute("aria-expanded", expanded);
  }

  /** Draw every card of both lists as the highlight now stands, and the links between the columns with it. */
  function paintLitCards(): void {
    for (const list of ["side-cards", "thread-list"]) {
      for (const row of [...deps.leftById(list).children] as HTMLElement[]) {
        if (row.hasAttribute("data-unpack-thread") || row.hasAttribute("data-main-row")) continue;
        paintLitCard(row, cardThread(row));
      }
    }
    deps.noteLinkedSelection();
  }

  /**
   * A press on card `row`, of thread `id`: highlighted, or put out when it is the one highlighted. The card
   * stays where it was on the screen, and is then shown whole where it fits: a card grown past the foot of
   * its list comes up as far as shows it, and one taking the highlight from a card above it does not jump
   * up as that one shrinks.
   */
  function toggleLitCard(id: string, row: HTMLElement): void {
    const channel = String(deps.selectedChannelId());
    if (litCards.get(channel) === id) litCards.delete(channel);
    else litCards.set(channel, id);
    const pane = row.parentNode === deps.leftById("side-cards") ? deps.sidePane() : deps.channelPane();
    const before = deps.rowShown(row) ? row.getBoundingClientRect().top : null;
    paintLitCards();
    if (before === null || !row.parentNode) return;
    // `#338 column-overscroll`. In the room past the right column's foot, the room is fitted to the card as it
    // now is before the card is put back: the last card put out would otherwise be held by the end of the list
    // the browser clamped to, and dropped by what it lost.
    deps.fitOverscroll(pane.scroller());
    deps.nudge(pane.scroller(), row.getBoundingClientRect().top - before);
    deps.bringIntoView(pane, row);
    deps.noteOverscrollReader(pane.scroller());
  }

  /**
   * A card's way into its thread: "3 replies · 2 unread", the owner's "N reply M unread" bit, a button of
   * its own beside the card's, which only highlights. Its parts are named for its list, by `prefix`:
   * "side-card" on the right, "thread-card" in the Threads list. With no count and nothing unread it says
   * "Open thread". A screen reader hears what it does, in the words it shows.
   */
  function cardOpener(prefix: string, replies: string, unread: string, open: () => void): HTMLButtonElement {
    const button = document.createElement("button") as HTMLButtonElement;
    button.className = `${prefix}-open`;
    button.setAttribute("type", "button");
    button.append(...cardCountParts(prefix, replies, unread, "Open thread"));
    const said = [replies, unread].filter(Boolean).join(" · ");
    button.setAttribute("aria-label", said ? `Open this thread, ${said}` : "Open thread");
    button.setAttribute("title", "Open this thread");
    deps.sayRowShortcut(button, `${prefix}-open`);
    button.addEventListener("click", open);
    return button;
  }

  /**
   * What a card says of its thread's replies, "3 replies · 2 unread", as the parts it is drawn in: the
   * count, and the unread in the accent (`${prefix}-replies`, `${prefix}-unread`). The one place those
   * words and that accent are made, for the button into a card's thread (`cardOpener`) and for the card
   * at the top of an open thread (`paintThreadCard`, `#298 thread-card-header`). With neither, `empty`
   * alone, or nothing for "".
   */
  function cardCountParts(prefix: string, replies: string, unread: string, empty = ""): Node[] {
    const parts: Node[] = [];
    if (replies || (!unread && empty)) {
      const count = document.createElement("span");
      count.className = `${prefix}-replies`;
      count.textContent = replies || empty;
      parts.push(count);
    }
    if (unread) {
      const fresh = document.createElement("span");
      fresh.className = `${prefix}-unread`;
      fresh.textContent = unread;
      if (parts.length) parts.push(document.createTextNode(" · "));
      parts.push(fresh);
    }
    return parts;
  }

  /**
   * Into the thread of card `row`, by its replies button or →. In two columns on the right, noted as
   * entered from the cards so that ← goes back out to them (`threadOutFromKeyboard`); in one column as
   * any thread opens, its Back the way to the Threads list. From the thread list, across the window, its
   * Back, Esc and ← the way back to the list (`openThreadFromList`, `#281 view-keys`).
   *
   * `#298 thread-card-header`. On the right the keys go into the thread with it, from a click as from →:
   * the button pressed was in the cards, which have just gone, and a browser drops the focus of a control
   * that goes on the page's body, where the next key and the next redraw of Main would not find the
   * right column (`#284 redraw-keeps-pane`).
   */
  function enterCard(id: string, summary: unknown, row: HTMLElement): unknown {
    if (row.parentNode !== deps.leftById("side-cards")) return deps.openThread(id, summary);
    if (deps.threadListShown()) return deps.openThreadFromList(id, summary);
    const opened = deps.selectSideThread(id, summary);
    deps.markSideFromCards();
    if (deps.sideShowing()) deps.focusList(deps.sidePane());
    return opened;
  }

  /** The right column's card of thread `id`, or null. */
  function cardRowOnRight(id: string | null): HTMLElement | null {
    if (id === null) return null;
    return ([...deps.leftById("side-cards").children] as HTMLElement[]).find((row) => row.getAttribute("data-id") === id) || null;
  }

  /** The thread whose card is highlighted while that card shows on the right beside Main, which the links follow; else null. */
  function litCardOnRight(): string | null {
    if (!deps.columnLinksShown() || deps.sideMode() !== "list") return null;
    const row = cardRowOnRight(litCard());
    return row && deps.rowShown(row) ? String(row.getAttribute("data-id")) : null;
  }

  /**
   * The thread cards on screen: the right column's while two columns show them, or the thread list does
   * (`#281 view-keys`: the same cards, across the window), or the Threads list in one column. Null
   * otherwise.
   */
  function cardsOnScreen(): HTMLElement | null {
    if (!deps.channelOnScreen()) return null;
    if (deps.sideColumnUp()) return deps.sideMode() === "list" ? deps.leftById("side-cards") : null;
    return deps.inMainColumn(() => (deps.selectionView() === "threads" && !deps.pinsOnly() ? deps.leftById("thread-list") : null));
  }

  /**
   * The card → goes into, on the cards on screen — the right column's while the key is for that column,
   * or the Threads list in one column: the card whose own button holds the focus, as Tab leaves it, which
   * is the card the key was pressed on; else, with the keys on a message unpacked under a card, that card,
   * as → on a message of All goes into its thread (`#407 unpack-row-parity`); else the highlighted card
   * while it shows; else, on the right, the card the keys are on, while it is on screen. Null anywhere
   * else.
   */
  function cardForArrow(target: HTMLElement): HTMLElement | null {
    const list = cardsOnScreen();
    const right = list === deps.leftById("side-cards");
    if (!list || (right && deps.paneFor(target) !== deps.sidePane())) return null;
    const rows = ([...list.children] as HTMLElement[])
      .filter((row) => !row.hasAttribute("data-unpack-thread") && !row.hasAttribute("data-main-row") && deps.rowShown(row));
    const held = rows.find((row) => row !== target && deps.nodeWithin(row, target));
    if (held) return held;
    const keyed = right ? deps.keyRow(target) : null;
    const under = keyed ? keyed.getAttribute("data-unpack-thread") : null;
    if (under !== null) return rows.find((row) => cardThread(row) === under) || null;
    const lit = litCard();
    const chosen = lit === null ? undefined : rows.find((row) => cardThread(row) === lit);
    if (chosen) return chosen;
    if (!right) return null;
    const row = deps.keyRow(target);
    return row && row.hasAttribute("data-card") && deps.rowInView(deps.sidePane(), row) ? row : null;
  }

  /** Esc: the highlighted card on screen put out. Whether there was one. */
  function putOutLitCard(): boolean {
    const list = cardsOnScreen();
    const lit = litCard();
    const row = list && lit !== null
      ? ([...list.children] as HTMLElement[]).find((one) => cardThread(one) === lit && deps.rowShown(one)) : undefined;
    if (!row || lit === null) return false;
    toggleLitCard(lit, row);
    return true;
  }

  return {
    litCard,
    cardThread,
    paintLitCard,
    paintLitCards,
    toggleLitCard,
    cardOpener,
    cardCountParts,
    enterCard,
    cardRowOnRight,
    litCardOnRight,
    cardsOnScreen,
    cardForArrow,
    putOutLitCard,
  };
}

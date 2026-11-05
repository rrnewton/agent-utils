// The keys that act on the selected row (`#224 keyboard-shortcuts`, `#229 desktop-two-column`, `#277
// thread-card-focus`, `#408 copy-upstream-link`, `#540 read-aloud-desktop`; `#624 voice-page-modularity`, moved
// from the legacy script into view/shell for the plan's step 17): Enter and o, → and ←, [ and ], r, ".", s, i and
// l, and pressing one of the row's own buttons. Which row the keys act on, and every act a press on a row or
// card performs, are the page's.

import { answersEnter, childByClass, focusMinimally, nodeWithin } from "../dom/index.ts";

/** The row a key acts on: its pane and row, and whether the key was taken even with no row to act on. */
export interface ActedRow<Pane> {
  readonly pane: Pane | null;
  readonly row: HTMLElement | null;
  readonly took: boolean;
}

/** A row's open ⋯ menu, as far as the keys read it. */
export interface OpenRowMenu {
  readonly root: unknown;
  close(): void;
}

export interface RowKeysDependencies<Pane, Message> {
  /** The row a key acts on from `target`; `runLine` lets it be the line of a collapsed run. */
  rowForAct(target: HTMLElement, runLine?: boolean): ActedRow<Pane>;
  /** The pane and row the keys are on from `target`, in sight or not. */
  keyContext(target: HTMLElement): { readonly pane: Pane | null; readonly row: HTMLElement | null };
  /** Open the collapsed run of read messages whose line is `row`. */
  openReadRun(row: HTMLElement): void;
  /** The row drawn anew for `row` in an unpacked block, or over the cards, or null. */
  liveUnpackedRow(row: HTMLElement): HTMLElement | null;
  liveCardRow(row: HTMLElement): HTMLElement | null;
  /** Select `row` of `pane`; `bring` false leaves the scroll where it is (its default is true). */
  selectRow(pane: Pane, row: HTMLElement, bring?: boolean): void;
  /** The channel's list, and the right column's. */
  channelPane(): Pane;
  sidePane(): Pane;
  /** Give the focus to the list of `pane`. */
  focusList(pane: Pane): void;
  /** Settle the focus on the list on screen. */
  settleFocus(): void;
  /** What a tap on `row` does, given its fold control. */
  actOnRow(row: HTMLElement, fold: HTMLElement | null): void;
  /** The card → goes into, for `target`, or null. */
  cardForArrow(target: HTMLElement): HTMLElement | null;
  /** Whether two columns are shown. */
  columnsShown(): boolean;
  /** Whether `node` is a message box. */
  isComposer(node: unknown): boolean;
  /** Close the thread on screen, its failure reported without ending a call. */
  closeThread(): void;
  /** Whether the code is working in the right column. */
  inSide(): boolean;
  /** Which screen is up, and the view of the main screen. */
  screen(): string;
  view(): string;
  /** What the right column shows ("thread", "list"), and whether its thread was entered from its card. */
  sideMode(): string;
  sideFromCards(): boolean;
  /** The id of the thread the right column shows. */
  sideThreadId(): string;
  /** The card on the right for the thread `id`, or null. */
  cardRowOnRight(id: string): HTMLElement | null;
  /** Close the right column's thread, back to its cards. */
  closeSideThread(): void;
  /** Whether `row` is shown. */
  rowShown(row: HTMLElement): boolean;
  /** Whether the thread list covers the left column. */
  threadListOnScreen(): boolean;
  /** The view the channel's list shows ("thread" in a thread). */
  selectionView(): string;
  /** Whether the list of the channel is the one on screen. */
  channelOnScreen(): boolean;
  /** Whether the reply's box is on screen. */
  replyOnScreen(): boolean;
  /** An element of the page by id, in the column the code is working in. */
  byId(id: string): HTMLElement;
  /** The items of the open ⋯ menu. */
  menuItems(): HTMLElement[];
  /** Read the card `card` aloud, as a press on it does. */
  readCardAloud(card: HTMLElement): void;
  /** Read the messages of `row` aloud, its failure reported without ending a call. */
  readRowAloud(row: HTMLElement): void;
  /** Open or close the details sheet on `row`. */
  toggleDetails(row: HTMLElement): void;
  /** The first message `row` stands for. */
  firstMessage(row: HTMLElement): Message;
  /** `message`'s own address in its service, or null. */
  upstreamUrlOf(message: Message): string | null;
  /** The ⋯ menu open on a row, or null. */
  openRowMenu(): OpenRowMenu | null;
  /** Copy `url`, `message`'s address, saying so in its service's name; its failure reported without ending a call. */
  copyUpstreamLink(url: string, message: Message): void;
}

/** The keys. Each returns whether it did anything, as a binding of the keymap does, but `pressRowButton`. */
export interface RowKeys {
  /**
   * Enter and o: what a tap on the selected message does (fold or unfold it, or in Read read it aloud:
   * `actOnRow`) and, on the line of a collapsed run of read messages, open the run. Enter on a button
   * or a link inside a row is that control's own press, and is left to it.
   */
  openFromKeyboard(event: { readonly key?: string }, target: HTMLElement): boolean;
  /**
   * Press one of the selected row's own buttons, found by its class: the act a click there performs. The
   * button pressed; "took" when the key was taken with no button to press; null when it was not.
   */
  pressRowButton(target: HTMLElement, className: string): HTMLElement | "took" | null;
  /**
   * →: into the selected message's thread, by its own thread tag (question 3); on the thread cards, into
   * the thread of the highlighted card, or of the card the keys are on, by its replies button (`#277
   * thread-card-focus`).
   */
  threadFromKeyboard(target: HTMLElement): boolean;
  /** Back out of a thread to the view it was opened from, the focus on the list. */
  leaveThread(): void;
  /** ←: out of a thread. Elsewhere it is nobody's. */
  threadOutFromKeyboard(): boolean;
  /**
   * [ and ]: the keys to the left column or to the right one, in two columns. `#229 desktop-two-column`.
   * F6, the desktop convention for moving between panes, is the browser's own in a window that has an
   * address bar, so it is not taken.
   */
  columnFromKeyboard(right: boolean): boolean;
  /**
   * r: reply to the selected message, its box open in the list and ready to type in. With words of
   * that message selected, its Reply is Quote reply, and r presses that (`#267 quote-reply`).
   */
  replyFromKeyboard(target: HTMLElement): boolean;
  /** .: the ⋯ menu of the selected message, the focus on its first item; ↓ and ↑ move through it. */
  menuFromKeyboard(target: HTMLElement): boolean;
  /** s: read the selected message aloud, or stop it: the act of a tap in Read. */
  readFromKeyboard(target: HTMLElement): boolean;
  /** i: the details sheet a press-and-hold opens (author, time and id) on the selected message. */
  detailsFromKeyboard(target: HTMLElement): boolean;
  /**
   * l: copy the selected message's own address in its service — the address its ⋯ menu's
   * "Open in …" link opens (`upstreamUrlOf`, the row's first constituent's, as the link is).
   * `#408 copy-upstream-link`. The menu holds the link and its Copy item (`#409`, `#461`);
   * this key is how a keyboard does the same copy. On a row with no
   * address the key is not acted on, as the menu offers nothing there: left to the browser
   * like any unbound key rather than swallowed in silence.
   */
  copyLinkFromKeyboard(target: HTMLElement): boolean;
}

export function createRowKeys<Pane, Message>(deps: RowKeysDependencies<Pane, Message>): RowKeys {
  function pressRowButton(target: HTMLElement, className: string): HTMLElement | "took" | null {
    const { row, took } = deps.rowForAct(target);
    const button = row ? childByClass(row, className) : null;
    if (!button) return took ? "took" : null;
    button.click();
    return button;
  }

  function leaveThread(): void {
    // Its drawing is done before its first wait: the focus can be settled on what it drew.
    deps.closeThread();
    deps.settleFocus();
  }

  return {
    openFromKeyboard(event, target) {
      if (event.key === "Enter" && answersEnter(target)) return false;
      const { pane, row, took } = deps.rowForAct(target, true);
      if (!pane || !row) return took;
      if (row.getAttribute("data-read-run") === "head") {
        deps.openReadRun(row);
        // `#407 unpack-row-parity`. In an unpacked block the run's rows are drawn anew: its first row's; and
        // over the cards, the run's first card or row (`#432 read-mode-unify`).
        const now = row.parentNode ? row : deps.liveUnpackedRow(row) || deps.liveCardRow(row);
        if (now) deps.selectRow(pane, now, false);
        return true;
      }
      // A card: a press on it, which highlights it and shows it whole, or puts that out. The keys stay on
      // it; → goes into its thread. `#277 thread-card-focus`, and `#229 desktop-two-column` before it. The
      // same in the thread list, whose cards are the right column's (`#281 view-keys`).
      if (row.hasAttribute("data-card")) {
        const card = childByClass(row, "side-card");
        if (card) card.click();
        // `#440 thread-card-pack-toggle`. In the Threads view the press drew the card anew: the keys stay on it.
        const now = row.parentNode ? null : deps.liveCardRow(row);
        if (now) deps.selectRow(deps.sidePane(), now, false);
        else deps.focusList(deps.sidePane());
        return true;
      }
      deps.actOnRow(row, childByClass(row, "fold"));
      return true;
    },
    pressRowButton,
    threadFromKeyboard(target) {
      const card = deps.cardForArrow(target);
      const opener = card ? childByClass(card, card.hasAttribute("data-card") ? "side-card-open" : "thread-card-open") : null;
      if (opener) {
        opener.click();
        // In two columns the keys go into the thread with it, as from a message's tag; in one column the
        // rows the focus was on have been replaced by the rows of the thread.
        if (deps.columnsShown()) deps.focusList(deps.sidePane());
        else deps.settleFocus();
        return true;
      }
      const { row, took } = deps.rowForAct(target);
      const badge = row ? childByClass(row, "thread-badge") : null;
      if (!badge || badge.hidden) return took;
      const starting = badge.getAttribute("data-thread-state") === "start";
      badge.click();
      // `#252 thread-button-state`. A thread nobody has answered opens with the focus in its message box,
      // for its first reply, and the keys leave it there.
      if (starting && deps.isComposer(document.activeElement)) return true;
      // In two columns the thread is selected on the right, and the keys go with it. `#229`.
      if (deps.columnsShown()) {
        deps.focusList(deps.sidePane());
        return true;
      }
      // The rows the focus was on have been replaced by the rows of the thread.
      deps.settleFocus();
      return true;
    },
    leaveThread,
    threadOutFromKeyboard() {
      // In two columns, from the right column back to Main on the left; the thread stays where it is.
      // `#229 desktop-two-column`.
      if (deps.columnsShown()) {
        if (!deps.inSide() || deps.screen() !== "main" || deps.view() !== "discord") return false;
        // `#277 thread-card-focus`. A thread entered from its card goes back out to the cards, as → came
        // in: the X, and the keys on the card it was entered from.
        if (deps.sideMode() === "thread" && deps.sideFromCards()) {
          const card = deps.cardRowOnRight(deps.sideThreadId());
          deps.closeSideThread();
          if (card && card.parentNode && deps.rowShown(card)) deps.selectRow(deps.sidePane(), card);
          else deps.focusList(deps.sidePane());
          return true;
        }
        deps.focusList(deps.channelPane());
        return true;
      }
      // The thread list shows cards, not a thread: there is none on screen to leave. `#281 view-keys`.
      if (deps.threadListOnScreen()) return false;
      if (deps.screen() !== "main" || deps.view() !== "discord" || deps.selectionView() !== "thread") return false;
      leaveThread();
      return true;
    },
    columnFromKeyboard(right) {
      if (!deps.columnsShown() || !deps.channelOnScreen()) return false;
      deps.focusList(right ? deps.sidePane() : deps.channelPane());
      return true;
    },
    replyFromKeyboard(target) {
      const pressed = pressRowButton(target, "reply-button");
      if (pressed === null) return false;
      if (pressed !== "took" && deps.replyOnScreen()) deps.byId("reply-text").focus();
      return true;
    },
    menuFromKeyboard(target) {
      const pressed = pressRowButton(target, "row-more-button");
      if (pressed === null) return false;
      if (pressed === "took") return true;
      const [first] = deps.menuItems();
      if (first) focusMinimally(first);
      return true;
    },
    readFromKeyboard(target) {
      // `#540 read-aloud-desktop`: a card is read as a press on it reads it, by its first message.
      // Not a card standing as the line of a run of read threads, which Enter opens.
      const { row: card } = deps.keyContext(target);
      if (card && card.hasAttribute("data-card") && card.getAttribute("data-read-run") !== "head") {
        deps.readCardAloud(card);
        return true;
      }
      const { row, took } = deps.rowForAct(target);
      if (!row) return took;
      deps.readRowAloud(row);
      return true;
    },
    detailsFromKeyboard(target) {
      const { row, took } = deps.rowForAct(target);
      if (!row) return took;
      deps.toggleDetails(row);
      return true;
    },
    copyLinkFromKeyboard(target) {
      const { row } = deps.rowForAct(target);
      if (!row) return false;
      const message = deps.firstMessage(row);
      const url = deps.upstreamUrlOf(message);
      if (url === null) return false;
      // The act the menu's item did when it was the copy, menu shut included: a copy asked with
      // this row's menu open leaves the reader with the said status, not the menu over it.
      const menu = deps.openRowMenu();
      if (menu && nodeWithin(row, menu.root)) menu.close();
      deps.copyUpstreamLink(url, message);
      return true;
    },
  };
}

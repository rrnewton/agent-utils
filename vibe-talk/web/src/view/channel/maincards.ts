// The main channel's messages among the cards, and the order of the Threads view's cards
// (`#419 threads-view-main-messages`, `#426 threads-sort-toggle`; `#624 voice-page-modularity`,
// moved from the legacy script): the device's Main-channel choice and the cards' order, both
// kept as they were — their setting objects and storage handed in untouched, so a saved choice
// survives this move byte for byte — the two chips that say them, and Main's rows among the
// cards. What the read modes are is only ever read here, never chosen. Words, order and timing
// are as they were in the script.

import { inTimeOrder, threadOf } from "../../core/channels/index.ts";
import type { ReplyTally } from "../../core/channels/index.ts";
import { THREAD_ORDERS, THREAD_ORDER_NAMES, createThreadOrder, threadOrderAfter } from "../../present/rows/index.ts";
import type { ThreadOrderChoice } from "../../present/rows/index.ts";
import type { StoragePort } from "../../ports/index.ts";
import { setTextIfChanged } from "../dom/index.ts";
import type { Message } from "../../contract/index.ts";

/** A kept on/off choice, as this owner uses it (core/settings `createKeptSwitch`). */
export interface MainCardsChoice {
  on(): boolean;
  set(on: boolean): void;
  keep(on: boolean): void;
  stored(): boolean;
}

/** A message of Main's, as the rows among the cards read it. */
export type MainCardsMessage = Message;

/** The device's storage, as the thread order keeps itself in (ports `StoragePort`). */
export type MainCardsStorage = StoragePort;

/** What the main cards ask of the page. */
export interface MainCardsDependencies {
  /** The Main-channel choice, made by core/settings' factory, handed in as a maker (that layer is not this package's to import). */
  makeMainChoice(): MainCardsChoice;
  /** The Main-channel setting's own key (`THREADS_MAIN_KEY`), as the page names it for the settings sync. */
  threadsMainKey: string;
  /** The device's storage, for the thread order's own kept copy. */
  storage(): MainCardsStorage;
  leftById(id: string): HTMLElement;
  threadListOnScreen(): boolean;
  sideMode(): string;
  threadListShown(): boolean;
  forgetCards(): void;
  sideColumnUp(): boolean;
  keepingCardPlace(change: () => void, follow?: boolean): void;
  renderSideColumn(): void;
  setStatus(text: string): void;
  selectedCardId(): string | null;
  paneRowForId(id: string): HTMLElement | null;
  rowShown(row: unknown): boolean;
  // Main's rows among the cards.
  storeChannel(): string;
  storeMessages(): readonly MainCardsMessage[];
  storeRemoved(): { has(id: string): boolean };
  storeDismissed(): unknown;
  selectedChannelId(): unknown;
  inView(message: MainCardsMessage, view: string, threadId: null): boolean;
  hiddenEmpty(message: MainCardsMessage): boolean;
  hidingRead(): boolean;
  stillToDo(message: MainCardsMessage, dismissed: unknown): boolean;
  sentFromThisPage(message: MainCardsMessage): boolean;
  whileMainRows<T>(act: () => T): T;
  glom(messages: readonly MainCardsMessage[]): unknown[];
  discordNode(group: unknown): HTMLElement;
  answeredBy(rows: readonly HTMLElement[], viewing: string): unknown;
  dressChannelRow(row: HTMLElement, pass: unknown): void;
}

/** The main cards' acts and the choices they own. */
export interface MainCards {
  /** The device's choice of Main channel among the cards: "on", or anything else for cards alone. */
  readonly THREADS_MAIN_KEY: string;
  readonly mainRowsChoice: MainCardsChoice;
  /** The order the Threads view's cards are in. */
  readonly threadOrders: ThreadOrderChoice;
  mainRowsOnScreen(): boolean;
  renderMainToggle(): void;
  setMainRowsShown(on: boolean): void;
  mainRowsFromKeyboard(): boolean;
  cardOrderOnScreen(): string;
  renderOrderToggle(): void;
  setThreadOrder(order: string): void;
  nextThreadOrder(): string;
  orderFromKeyboard(): boolean;
  mainRowMessages(carded: ReadonlySet<string>): MainCardsMessage[];
  mainRows(messages: readonly MainCardsMessage[], tally: ReplyTally): HTMLElement[];
  placeMainCard(): void;
}

/** Make the main cards, in the page's construction order. */
export function createMainCards(deps: MainCardsDependencies): MainCards {
  /** The device's choice of Main channel among the cards: "on", or anything else for cards alone. */
  const THREADS_MAIN_KEY = deps.threadsMainKey;

  /**
   * Whether the main channel's messages stand among the cards: Main channel pressed, kept on the server
   * (`#457 settings-server-side`) and read from the device here, as the page loads; a browser refusing storage
   * reads as cards alone (`DEVICE_SETTINGS.threadsMain.refused`). `#419 threads-view-main-messages`. One owner
   * in web/src/core/settings/switches.ts (`#624 voice-page-modularity`).
   */
  const mainRowsChoice = deps.makeMainChoice();

  /** Whether Main's messages belong on screen among the cards: pressed, and the Threads view showing cards. */
  const mainRowsOnScreen = (): boolean => mainRowsChoice.on() && deps.threadListOnScreen() && deps.sideMode() === "list";

  /** Main channel: offered over the Threads view's cards, as Unpack msgs is, and pressed as the reader left it. */
  function renderMainToggle(): void {
    const button = deps.leftById("side-main");
    const offered = deps.sideMode() === "list" && deps.threadListShown();
    if (button.hidden === offered) button.hidden = !offered;
    const pressed = mainRowsChoice.on() ? "true" : "false";
    if (button.getAttribute("aria-pressed") !== pressed) button.setAttribute("aria-pressed", pressed);
  }

  /** The reader pressed Main channel, or a: kept, and the cards drawn again. */
  function setMainRowsShown(on: boolean): void {
    mainRowsChoice.set(on);
    mainRowsChoice.keep(on);
    renderMainToggle();
    deps.forgetCards();
    if (deps.sideColumnUp()) deps.keepingCardPlace(deps.renderSideColumn);
    placeMainCard();
  }

  /** a, over the Threads view: Main's messages among the cards, or the cards alone again. Elsewhere, nothing. */
  function mainRowsFromKeyboard(): boolean {
    if (!deps.threadListOnScreen() || deps.sideMode() !== "list") return false;
    setMainRowsShown(!mainRowsChoice.on());
    deps.setStatus(mainRowsChoice.on() ? "The main channel's messages among the threads." : "The threads alone.");
    return true;
  }

  // The orders, their words and times, the cards put in one, and the order chosen, kept on this device,
  // are web/src/present/rows's (`createThreadOrder`, `#624 voice-page-modularity`).

  /** The order the Threads view's cards are in. */
  const threadOrders = createThreadOrder({ storage: deps.storage });

  /** The order the cards on screen are drawn in: the one chosen over the Threads view's cards, else the default. */
  const cardOrderOnScreen = (): string => (deps.threadListOnScreen() && deps.sideMode() === "list" ? threadOrders.current() : THREAD_ORDER_NAMES[0] as string);

  /** The order chip: offered over the Threads view's cards, as Main channel is, saying the order they are in. */
  function renderOrderToggle(): void {
    const button = deps.leftById("side-order");
    const offered = deps.sideMode() === "list" && deps.threadListShown();
    if (button.hidden === offered) button.hidden = !offered;
    const order = THREAD_ORDERS[threadOrders.current()] as { name: string; short: string; says: string };
    const next = THREAD_ORDERS[threadOrderAfter(threadOrders.current())] as { name: string };
    // Pressed in any order but the default, as the other chips are pressed when they change the list.
    const pressed = threadOrders.current() === THREAD_ORDER_NAMES[0] ? "false" : "true";
    if (button.getAttribute("aria-pressed") !== pressed) button.setAttribute("aria-pressed", pressed);
    const label = `Order: ${order.name}`;
    if (button.getAttribute("aria-label") !== label) button.setAttribute("aria-label", label);
    const title = `In order of ${order.name.toLowerCase()}: ${order.says}. Press for ${next.name.toLowerCase()}`;
    if (button.title !== title) button.title = title;
    // The whole name, and on a phone the short one (web/voice.css).
    setTextIfChanged(deps.leftById("side-order-whole"), order.name);
    setTextIfChanged(deps.leftById("side-order-short"), order.short);
  }

  /**
   * The reader pressed the order chip, or d: the next order, kept, and the cards drawn in it,
   * the selected card kept where it is on screen (`keepingCardPlace` keeps it where it shows; otherwise
   * it is brought into view).
   */
  function setThreadOrder(order: string): void {
    threadOrders.set(order);
    renderOrderToggle();
    deps.forgetCards();
    if (!deps.sideColumnUp()) return;
    const selected = deps.selectedCardId();
    deps.keepingCardPlace(deps.renderSideColumn, true);
    const row = selected === null ? null : deps.paneRowForId(selected);
    if (!row || !deps.rowShown(row)) return;
    const area = deps.leftById("side-scroll").getBoundingClientRect();
    const box = row.getBoundingClientRect();
    if ((box.top < area.top || box.bottom > area.bottom) && typeof row.scrollIntoView === "function") {
      row.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }

  /** The next order after the one the cards are in. */
  const nextThreadOrder = (): string => threadOrderAfter(threadOrders.current());

  /** d, over the Threads view: the cards in the next order. Elsewhere, nothing. */
  function orderFromKeyboard(): boolean {
    if (!deps.threadListOnScreen() || deps.sideMode() !== "list") return false;
    setThreadOrder(nextThreadOrder());
    deps.setStatus(`Threads in order of ${(THREAD_ORDERS[threadOrders.current()] as { name: string }).name.toLowerCase()}.`);
    return true;
  }

  /**
   * The main channel's messages that head no thread a card stands for, oldest first, as Main draws them:
   * the store's copies of Main's rows (`inView`), less the roots of `carded` and anything removed upstream,
   * no empty message where those are hidden, and under Hide read only what is still to do.
   */
  function mainRowMessages(carded: ReadonlySet<string>): MainCardsMessage[] {
    if (deps.storeChannel() !== String(deps.selectedChannelId())) return [];
    const messages = deps.storeMessages().filter((message) => {
      if (!deps.inView(message, "main", null) || deps.storeRemoved().has(String(message.id)) || deps.hiddenEmpty(message)) return false;
      const thread = threadOf(message as never);
      if (thread !== null && message.thread && message.thread.is_root === true && carded.has(thread)) return false;
      return !deps.hidingRead() || deps.stillToDo(message, deps.storeDismissed()) || deps.sentFromThisPage(message);
    });
    return inTimeOrder(messages as never, "timestamp") as MainCardsMessage[];
  }

  /**
   * Main's messages `messages`, one run between two cards, as rows of the cards' list: built and dressed
   * as every list's rows are, in the right column, whose list the keys and the mouse move them in.
   */
  function mainRows(messages: readonly MainCardsMessage[], tally: ReplyTally): HTMLElement[] {
    // They offer the way into a thread, as All's do, and point at nothing (`rowDrawing.main`).
    const rows = deps.whileMainRows(() => deps.glom(messages).map((group) => {
      const row = deps.discordNode(group);
      row.setAttribute("data-main-row", "");
      row.className = `${row.className} main-row`;
      return row;
    }));
    const viewing = String(deps.selectedChannelId());
    const pass = { tally, answered: deps.answeredBy(rows, viewing), viewing, inMain: false, inAll: false };
    for (const row of rows) deps.dressChannelRow(row, pass);
    return rows;
  }

  /**
   * The main channel's box as the last card of the list, while Main's messages are among the cards;
   * back in its place under the left column's list otherwise. The box's draft and where it posts are the
   * left column's, which is Main behind the list (`leftColumnToMain`).
   */
  function placeMainCard(): void {
    const box = deps.leftById("channel-composer");
    const list = deps.leftById("side-list");
    // `#430 threads-main-own-sent`. And the outbox's receipts over it, as in Main: a send still on its
    // way, or one that failed, is shown where it was sent from, not in the list hidden behind the cards.
    const receipts = deps.leftById("outgoing-log");
    const wanted = mainRowsOnScreen();
    const there = box.parentNode === list;
    if (wanted && !there) list.append(receipts, box);
    else if (!wanted && there) {
      deps.leftById("pane-discord").insertBefore(box, deps.leftById("column-reply"));
      deps.leftById("pane-discord").insertBefore(receipts, box);
    }
    if (box.hasAttribute("data-main-card") !== wanted) box.toggleAttribute("data-main-card", wanted);
  }

  return {
    THREADS_MAIN_KEY,
    mainRowsChoice,
    threadOrders,
    mainRowsOnScreen,
    renderMainToggle,
    setMainRowsShown,
    mainRowsFromKeyboard,
    cardOrderOnScreen,
    renderOrderToggle,
    setThreadOrder,
    nextThreadOrder,
    orderFromKeyboard,
    mainRowMessages,
    mainRows,
    placeMainCard,
  };
}

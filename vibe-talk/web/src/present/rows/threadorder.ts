// The order of the Threads view's cards (`#426 threads-sort-toggle`, `#457 settings-server-side`; `#624
// voice-page-modularity`, moved from the legacy script into present/rows): the orders and their words,
// the time a card stands at in each, the cards put in one, the next order round, and the one owner of
// the order chosen, kept on the device. The chip, d, and keeping the selected card where it shows are
// the page's. Why a toggle and not a menu, and why only over the Threads view's cards, is told where the
// legacy script opens the section.

import type { StoragePort } from "../../ports/index.ts";

/** The device's choice of order for the Threads view's cards: a key of `THREAD_ORDERS`. */
const THREAD_ORDER_KEY = "vibe-talk.voice.thread-order";

/** A card's thread as the orders read it: when it last moved, and when it was started. */
export interface OrderedCard {
  readonly at?: number;
  readonly started?: number;
}

/** One order: what the chip says (whole, and on a phone), what a press does, and the time a card stands at. */
export interface ThreadOrder {
  readonly name: string;
  readonly short: string;
  readonly says: string;
  readonly time: (choice: OrderedCard) => number;
}

/** When a card's thread last moved, for its place among Main's messages: its newest activity, or its start. */
function cardTime(choice: OrderedCard): number {
  return Number.isFinite(choice.at) ? choice.at as number : Number.isFinite(choice.started) ? choice.started as number : 0;
}

/**
 * The orders the Threads view's cards can be in, the first the default, in the order a press goes round
 * them: what the chip says (whole, and on a phone), what its tooltip says a press does, and the time a
 * card stands at, oldest first, the newest at the foot.
 */
const THREAD_ORDERS: Readonly<Record<string, ThreadOrder>> = Object.freeze({
  updated: {
    name: "Latest update", short: "Latest", says: "the threads by when each last moved",
    time: (choice: OrderedCard) => cardTime(choice),
  },
  created: {
    name: "Thread created", short: "Created", says: "the threads by when each was started",
    time: (choice: OrderedCard) => (Number.isFinite(choice.started) ? choice.started as number : cardTime(choice)),
  },
});

/** The orders' names, in the order a press goes round them. */
const THREAD_ORDER_NAMES: readonly string[] = Object.freeze(Object.keys(THREAD_ORDERS));

/** The order after `order`, round the cycle. */
function threadOrderAfter(order: string): string {
  return THREAD_ORDER_NAMES[(THREAD_ORDER_NAMES.indexOf(order) + 1) % THREAD_ORDER_NAMES.length] as string;
}

/** `choices` in order `order`, oldest first: the default as the thread list's read keeps them, any other by its time. */
function inThreadOrder<C extends OrderedCard>(choices: readonly C[], order: string): C[] {
  const oldest = [...choices].reverse();
  if (order === THREAD_ORDER_NAMES[0]) return oldest;
  const time = (THREAD_ORDERS[order] as ThreadOrder).time;
  // Stable, so threads at the same time keep the default's order among them.
  return oldest.map((choice, at) => ({ choice, at, when: time(choice) }))
    .sort((a, b) => a.when - b.when || a.at - b.at).map((one) => one.choice);
}

/** What the order asks of the page. */
export interface ThreadOrderDependencies {
  /** The device's storage (`deviceStorage`). */
  storage(): StoragePort;
}

/** The order's commands and questions. */
export interface ThreadOrderChoice {
  /** The order the Threads view's cards are in. */
  current(): string;
  /** The order as kept, or the default where none is, or one this page no longer knows. */
  stored(): string;
  /** Put the cards in `order` (the default for one this page does not know), and keep it. */
  set(order: string): void;
}

/** The order as kept, read now. */
export function createThreadOrder(deps: ThreadOrderDependencies): ThreadOrderChoice {
  function storedThreadOrder(): string {
    try {
      const kept = deps.storage().getItem(THREAD_ORDER_KEY);
      return kept && Object.prototype.hasOwnProperty.call(THREAD_ORDERS, kept) ? kept : THREAD_ORDER_NAMES[0] as string;
    } catch (_error) {
      return THREAD_ORDER_NAMES[0] as string;
    }
  }

  /** The order the Threads view's cards are in. */
  let threadOrder = storedThreadOrder();

  return {
    current: () => threadOrder,
    stored: storedThreadOrder,
    set(order) {
      threadOrder = Object.prototype.hasOwnProperty.call(THREAD_ORDERS, order) ? order : THREAD_ORDER_NAMES[0] as string;
      try {
        deps.storage().setItem(THREAD_ORDER_KEY, threadOrder);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this session.
      }
    },
  };
}

// What the page and the tests use of it.
export { THREAD_ORDERS, THREAD_ORDER_KEY, THREAD_ORDER_NAMES, cardTime, inThreadOrder, threadOrderAfter };

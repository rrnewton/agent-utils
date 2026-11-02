// Each thread packed or unpacked from its card, and Unpack's state over them all (`#404
// threads-unpack-all`, `#440 thread-card-pack-toggle`, `#457 settings-server-side`; `#624
// voice-page-modularity`, moved from the legacy script into present/rows). The one owner of both
// choices and of how they are kept: Unpack three ways (Pack all, Unpack unread, Unpack all), and each
// thread's own choice of its card, which outranks Unpack for that thread until Unpack is pressed
// again. Which cards are on screen, and drawing them, are the page's.

import type { StoragePort } from "../../ports/index.ts";

/** Unpack's states, in the order a press goes round them. */
const UNPACK_MODES: readonly string[] = Object.freeze(["packed", "unread", "all"]);

/** What the Unpack chip says in each state: the state it is in, as the read mode's button does. */
const UNPACK_MODE_NAMES: Readonly<Record<string, string>> = Object.freeze({ packed: "Pack all", unread: "Unpack unread", all: "Unpack all" });

/** What u says on the status line as it moves the chip to each state. */
const UNPACK_MODE_SAID: Readonly<Record<string, string>> = Object.freeze({
  packed: "Threads packed away.", unread: "The unread threads unpacked, the read ones packed.",
  all: "Every thread unpacked under its card.",
});

/** A thread's card, three ways, in the order a press goes round them. */
const CARD_STATES: readonly string[] = Object.freeze(["collapsed", "expanded", "unpacked"]);

/** Unpack's state on the device, kept on the server for every device. */
const THREADS_UNPACKED_KEY = "vibe-talk.voice.threads-unpacked";

/** The device's choice of each thread's card, by channel and thread: what outranks Unpack for it. */
const THREAD_CARD_STATES_KEY = "vibe-talk.voice.thread-card-states";

/** How many threads' choices are kept on the device: the most recent, far more than a list shows. */
const THREAD_CARD_STATES_KEPT = 300;

/** What the pack choices ask of the page: the device's storage, read and written when they change. */
export interface CardPackingDependencies {
  storage(): StoragePort;
}

/** The pack choices: Unpack's state and each thread's own. */
export interface CardPacking {
  /** Unpack's state: one of `UNPACK_MODES`. */
  mode(): string;
  /** The state after Unpack's, round the cycle. */
  nextMode(): string;
  /**
   * The reader's choice of Unpack: kept, and every thread of `channel` given back to it, their own
   * choices let go. Anything not one of `UNPACK_MODES` is Pack all.
   */
  setMode(mode: string, channel: string): void;
  /** Thread `id` of `channel`'s own choice of its card, or undefined where Unpack decides it. */
  choiceOf(channel: string, id: string): string | undefined;
  /** Thread `id` of `channel` drawn `state` by its own choice, the most recent kept last. */
  choose(channel: string, id: string, state: string): void;
  /** Whether any thread of `channel` is unpacked by its own choice. */
  unpackedIn(channel: string): boolean;
  /** Unpack's state read again from the device, as a change from another device brings it. */
  reloadMode(): void;
  /** Every thread's choice read again from the device, as a change from another device brings them. */
  reloadChoices(): void;
}

/** Where thread `id` of `channel` keeps its choice. */
const choiceKey = (channel: string, id: string): string => `${channel}\n${id}`;

/** The pack choices, read from the device's storage now. */
export function createCardPacking(deps: CardPackingDependencies): CardPacking {
  /** Unpack's state as kept; an older page's "on" means Unpack all. */
  const storedMode = (): string => {
    try {
      const kept = deps.storage().getItem(THREADS_UNPACKED_KEY);
      if (kept === "on") return "all";
      return UNPACK_MODES.includes(String(kept)) ? String(kept) : "packed";
    } catch (_error) {
      return "packed";
    }
  };
  /** The threads' choices as kept, or none where they cannot be read. */
  const storedChoices = (): Map<string, string> => {
    const states = new Map<string, string>();
    try {
      const kept: unknown = JSON.parse(deps.storage().getItem(THREAD_CARD_STATES_KEY) || "[]");
      if (Array.isArray(kept)) {
        for (const pair of kept as unknown[]) {
          if (Array.isArray(pair) && typeof pair[0] === "string" && CARD_STATES.includes(pair[1] as string)) {
            states.set(pair[0], pair[1] as string);
          }
        }
      }
    } catch (_error) {
      // A record that cannot be read is no choice at all: Unpack decides every thread.
    }
    return states;
  };
  let mode = storedMode();
  /** `channel\nthread` -> "collapsed", "expanded" or "unpacked", the most recent last. */
  const choices = storedChoices();
  /** Keep the threads' choices, the most recent `THREAD_CARD_STATES_KEPT`. */
  const keepChoices = (): void => {
    try {
      deps.storage().setItem(THREAD_CARD_STATES_KEY, JSON.stringify([...choices].slice(-THREAD_CARD_STATES_KEPT)));
    } catch (_error) {
      // A browser that refuses storage still honours the choices for this session.
    }
  };
  return {
    mode: () => mode,
    nextMode: () => UNPACK_MODES[(UNPACK_MODES.indexOf(mode) + 1) % UNPACK_MODES.length] as string,
    setMode(next, channel) {
      mode = UNPACK_MODES.includes(next) ? next : "packed";
      try {
        deps.storage().setItem(THREADS_UNPACKED_KEY, mode);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this session.
      }
      const prefix = `${channel}\n`;
      for (const key of [...choices.keys()]) {
        if (key.startsWith(prefix)) choices.delete(key);
      }
      keepChoices();
    },
    choiceOf: (channel, id) => choices.get(choiceKey(channel, id)),
    choose(channel, id, state) {
      const key = choiceKey(channel, id);
      choices.delete(key);
      choices.set(key, state);
      keepChoices();
    },
    unpackedIn: (channel) => [...choices].some(([key, state]) => state === "unpacked" && key.startsWith(`${channel}\n`)),
    reloadMode() {
      mode = storedMode();
    },
    reloadChoices() {
      const held = storedChoices();
      choices.clear();
      for (const [name, state] of held) choices.set(name, state);
    },
  };
}

// What the page and the tests use of it.
export { CARD_STATES, THREADS_UNPACKED_KEY, THREAD_CARD_STATES_KEPT, THREAD_CARD_STATES_KEY, UNPACK_MODES, UNPACK_MODE_NAMES, UNPACK_MODE_SAID };

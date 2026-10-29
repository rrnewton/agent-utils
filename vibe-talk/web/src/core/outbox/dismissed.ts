// Sends dismissed from the outbox, kept for Undo and then for a while longer (`#193 compose-draft-loss`;
// `#624 voice-page-modularity`, moved from the legacy script; plan step 12): newest first, each with
// when it was dismissed, saved on the device. The list is private to its owner; the outbox itself,
// Undo's chip and the Settings list stay with the page.

import type { StoragePort } from "../../ports/index.ts";
import { storedExactly as storedExactlyIn } from "./storage.ts";

/** How many dismissed sends are kept, newest first, and for how many days. */
const DISMISSED_SENDS_LIMIT = 10;
const DISMISSED_SENDS_DAYS = 7;
const DISMISSED_SENDS_MAX_AGE_MS = DISMISSED_SENDS_DAYS * 24 * 60 * 60 * 1000;

/** Where the device keeps them. */
const DISMISSED_SENDS_KEY = "vibe-talk.dismissed-sends";

/** One dismissed send, and when it was dismissed. */
export interface DismissedSend<Entry> {
  readonly entry: Entry & { readonly id: string };
  readonly at: number;
}

/** What the owner asks of the page. */
export interface DismissedSendsDependencies<Entry> {
  readonly storage: StoragePort;
  /** The wall clock, in milliseconds. */
  now(): number;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
  /** A saved entry as an outbox entry, or null where it cannot be one (`outgoingFromSaved`). */
  decode(saved: unknown): (Entry & { readonly id: string }) | null;
}

/** The owner's named commands. */
export interface DismissedSends<Entry> {
  /** Every dismissed send, newest first. */
  list(): readonly DismissedSend<Entry>[];
  /** The dismissed send of outbox entry `id`, if any. */
  find(id: string): DismissedSend<Entry> | undefined;
  /** Dismiss `entry` now: first in the list, any older copy of it gone. Answers whether the device kept it. */
  add(entry: Entry & { readonly id: string }): boolean;
  /** Take `held` out of the list (put back in the outbox). Answers whether the device kept the change. */
  remove(held: DismissedSend<Entry>): boolean;
  /** Forget every dismissed send now, rather than when its days are up. */
  clear(): boolean;
  /** Drop, here only, what is past the limit or its days; the device's copy waits for the next save. */
  prune(): void;
  /** Read what the device kept, pruned; a damaged copy is nothing. */
  load(): void;
}

/** An owner holding nothing dismissed. */
export function createDismissedSends<Entry>(deps: DismissedSendsDependencies<Entry>): DismissedSends<Entry> {
  let dismissedSends: DismissedSend<Entry>[] = [];
  /** Only the newest DISMISSED_SENDS_LIMIT, and none older than DISMISSED_SENDS_DAYS. */
  const pruneDismissedSends = (): void => {
    const oldest = deps.now() - DISMISSED_SENDS_MAX_AGE_MS;
    dismissedSends = dismissedSends.filter((held) => held.at > oldest).slice(0, DISMISSED_SENDS_LIMIT);
  };
  const storedExactly = (encoded: string): boolean => storedExactlyIn(deps.storage, DISMISSED_SENDS_KEY, encoded);
  /** Save the recently dismissed, giving up the saved channel rows first where the quota is full. */
  const persistDismissedSends = (): boolean => {
    pruneDismissedSends();
    try {
      if (dismissedSends.length === 0) {
        deps.storage.removeItem(DISMISSED_SENDS_KEY);
        return true;
      }
      const encoded = JSON.stringify(dismissedSends);
      if (storedExactly(encoded)) return true;
      deps.makeRoom();
      return storedExactly(encoded);
    } catch (_error) {
      return false;
    }
  };
  return {
    list: () => dismissedSends,
    find: (id) => dismissedSends.find((item) => item.entry.id === id),
    add(entry) {
      dismissedSends = [{ entry, at: deps.now() }, ...dismissedSends.filter((held) => held.entry.id !== entry.id)];
      return persistDismissedSends();
    },
    remove(held) {
      dismissedSends = dismissedSends.filter((item) => item !== held);
      return persistDismissedSends();
    },
    clear() {
      dismissedSends = [];
      return persistDismissedSends();
    },
    prune: pruneDismissedSends,
    load() {
      try {
        const saved: unknown = JSON.parse(deps.storage.getItem(DISMISSED_SENDS_KEY) || "[]");
        if (!Array.isArray(saved)) return;
        dismissedSends = saved
          .map((held: { entry?: unknown; at?: unknown } | null) =>
            ({ entry: held && deps.decode(held.entry), at: Number(held && held.at) }))
          .filter((held): held is DismissedSend<Entry> => Boolean(held.entry) && Number.isFinite(held.at))
          .sort((a, b) => b.at - a.at);
        const before = dismissedSends.length;
        pruneDismissedSends();
        if (dismissedSends.length !== before) persistDismissedSends();
      } catch (_error) {
        dismissedSends = [];
      }
    },
  };
}

export { DISMISSED_SENDS_DAYS, DISMISSED_SENDS_LIMIT };

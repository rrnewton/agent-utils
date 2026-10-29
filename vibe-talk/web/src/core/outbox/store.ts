// The outbox: every send this page has not seen through, by id, in the order written (`#193
// compose-draft-loss`, `#195 send-resilience`, `#277 send-order`, `#466 reply-promotes-thread`, `#500
// sent-receipts-pile-up`; `#624 voice-page-modularity`, moved from the legacy script; plan step 12).
// The entries are the page's own objects, changed in place as a send goes on; this owner holds which
// are in the outbox and in what order, how often that has changed, and the device's copy. How a send
// goes, and how the outbox is drawn, stay with the page.

import type { StoragePort } from "../../ports/index.ts";
import { storedExactly } from "./storage.ts";

/** Where the device keeps the outbox. */
const OUTGOING_KEY = "vibe-talk.outgoing-messages";

/** The fields of an entry the outbox itself reads. */
export interface OutboxEntry {
  readonly id: string;
  readonly channel: string;
  readonly state: string;
  readonly createdAt: number;
}

/** What the owner asks of the page. */
export interface OutboxDependencies {
  readonly storage: StoragePort;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
  /**
   * Whether the page's store keeps its own posts in `channel` against a page from older state
   * (`storeKeepsOwnPosts`): a confirmed send there is never saved, its receipt being a display aid.
   */
  keepsOwnPosts(channel: string): boolean;
}

/** The owner's read surface and named commands. */
export interface OutboxStore<Entry extends OutboxEntry> {
  /** The entry of id `id`, if it is in the outbox. */
  get(id: string): Entry | undefined;
  /** Whether an entry of id `id` is in the outbox. */
  has(id: string): boolean;
  /** How many entries are in the outbox. */
  size(): number;
  /** The entries, in the order written; live, so an entry removed while they are walked is passed over. */
  values(): IterableIterator<Entry>;
  /** The entries' ids, in the order written; live, as `values` is. */
  ids(): IterableIterator<string>;
  /** `entry` is in the outbox now, after every entry already there. */
  add(entry: Entry): void;
  /** Entry `id` is out of the outbox. */
  remove(id: string): void;
  /** `entry` is back in the outbox, where its `createdAt` puts it among the entries still there. */
  putBackInOrder(entry: Entry): void;
  /** How often the outbox has changed (a save, or `touch`), for what is worked out from it. */
  version(): number;
  /** An entry changed in a way what is worked out from the outbox depends on, without a save. */
  touch(): void;
  /** Whether the device kept the last save (or the load found it readable). */
  storageOkay(): boolean;
  /**
   * Save the outbox, giving up the saved channel rows first where the quota is full: a full quota is
   * not a reason to lose an unsent message. Moves `version`. Answers whether the device kept it.
   */
  save(): boolean;
  /** What the device saved, parsed; throws where it cannot be read, as `refused` then says. */
  saved(): unknown;
  /** The device's copy could not be read or loaded. */
  refused(): void;
}

/** An empty outbox. */
export function createOutboxStore<Entry extends OutboxEntry>(deps: OutboxDependencies): OutboxStore<Entry> {
  const outgoingMessages = new Map<string, Entry>();
  let outboxVersion = 0;
  let outgoingStorageOkay = true;
  return {
    get: (id) => outgoingMessages.get(id),
    has: (id) => outgoingMessages.has(id),
    size: () => outgoingMessages.size,
    values: () => outgoingMessages.values(),
    ids: () => outgoingMessages.keys(),
    add(entry) {
      outgoingMessages.set(entry.id, entry);
    },
    remove(id) {
      outgoingMessages.delete(id);
    },
    putBackInOrder(entry) {
      // In the order it was written among the sends still there, as it was before it was dismissed.
      const all = [...outgoingMessages.values(), entry].sort((a, b) => a.createdAt - b.createdAt);
      outgoingMessages.clear();
      for (const one of all) outgoingMessages.set(one.id, one);
    },
    version: () => outboxVersion,
    touch() {
      outboxVersion += 1;
    },
    storageOkay: () => outgoingStorageOkay,
    save() {
      // `#466 reply-promotes-thread`: what `pendingByThread` worked out is stale.
      outboxVersion += 1;
      try {
        // `#500 sent-receipts-pile-up`. Never a confirmed send where the store keeps the page's own
        // posts (`sentReceiptRetires`): its receipt is a display aid for this page, and a reload keeps
        // the post itself against a read from older state as this page does (`ownPosts`). Saved, a
        // receipt no read of this page ever retired came back on every reload.
        const encoded = JSON.stringify([...outgoingMessages.values()]
          .filter((entry) => entry.state !== "sent" || !deps.keepsOwnPosts(entry.channel)));
        outgoingStorageOkay = storedExactly(deps.storage, OUTGOING_KEY, encoded);
        if (!outgoingStorageOkay) {
          // A full quota is not a reason to lose an unsent message: saved channel rows can be read
          // again, so they give up their space first.
          deps.makeRoom();
          outgoingStorageOkay = storedExactly(deps.storage, OUTGOING_KEY, encoded);
        }
      } catch (_error) {
        outgoingStorageOkay = false;
      }
      return outgoingStorageOkay;
    },
    saved: () => JSON.parse(deps.storage.getItem(OUTGOING_KEY) || "[]") as unknown,
    refused() {
      outgoingStorageOkay = false;
    },
  };
}

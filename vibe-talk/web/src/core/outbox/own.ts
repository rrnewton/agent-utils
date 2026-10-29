// The messages this page posted (`#430 threads-main-own-sent`, `#466 reply-promotes-thread`, `#500
// sent-receipts-pile-up`; `#624 voice-page-modularity`, moved from the legacy script; plan step 12):
// which were sent from here, for the page's life, and when each was acknowledged, kept on the device
// for OWN_POST_GRACE_MS so a reload in the grace keeps the page's own post against a read from older
// state as the page that sent it did. Every window of the origin writes the device's copy, each adding
// its own. Why a page's own fresh post stands is told where the legacy script asks (`ownPostStands`).

import type { StoragePort } from "../../ports/index.ts";

/**
 * How long a message this page posted is kept though a page of the channel does not carry it: a chat
 * service answering from older state catches up within seconds, and a reply deleted upstream meanwhile
 * still goes by the removal the page is told of.
 */
const OWN_POST_GRACE_MS = 600000;

/** Where the device keeps them, by id. */
const OWN_POSTS_KEY = "vibe-talk.own-posts";

/** What the owner asks of the page. */
export interface OwnPostsDependencies {
  readonly storage: StoragePort;
  /** The wall clock, in milliseconds. */
  now(): number;
}

/** The owner's named commands. */
export interface OwnPosts {
  /** Messages `ids` were posted from this page and acknowledged now; the device keeps them too. */
  noteSent(ids: readonly string[]): void;
  /** Whether message `id` was sent from this page, ever in its life. */
  sentFromHere(id: string): boolean;
  /** When message `id`, one this page or the device sent in the grace, was acknowledged. */
  sentAt(id: string): number | undefined;
  /** Whether message `id` was posted within OWN_POST_GRACE_MS; one past it is forgotten as it is asked. */
  stands(id: string): boolean;
  /** Message `id` was removed upstream: kept no longer, here or on the device. */
  removed(id: string): void;
  /** Take back what earlier pages of the device sent in the grace. */
  load(): void;
}

/** An owner holding nothing sent. */
export function createOwnPosts(deps: OwnPostsDependencies): OwnPosts {
  const sentFromHere = new Set<string>();
  const sentFromHereAt = new Map<string, number>();
  /** The saved `sentFromHereAt` still in its grace, by id. */
  const savedOwnPosts = (): Map<string, number> => {
    const kept = new Map<string, number>();
    try {
      const saved: unknown = JSON.parse(deps.storage.getItem(OWN_POSTS_KEY) || "{}");
      for (const [id, at] of Object.entries(saved && typeof saved === "object" ? saved : {})) {
        if (typeof at === "number" && deps.now() - at < OWN_POST_GRACE_MS) kept.set(id, at);
      }
    } catch (_error) {
      // Unreadable: nothing kept, as on a device that never sent.
    }
    return kept;
  };
  /** Save `sentFromHereAt`, with what other windows saved, within the grace, and without `removed`. */
  const saveOwnPosts = (removed: string | null = null): void => {
    const all = savedOwnPosts();
    for (const [id, at] of sentFromHereAt) if (deps.now() - at < OWN_POST_GRACE_MS) all.set(id, at);
    if (removed !== null) all.delete(removed);
    try {
      if (all.size) deps.storage.setItem(OWN_POSTS_KEY, JSON.stringify(Object.fromEntries(all)));
      else deps.storage.removeItem(OWN_POSTS_KEY);
    } catch (_error) {
      // A full quota: the page keeps them for itself, as before this was saved.
    }
  };
  return {
    noteSent(ids) {
      for (const id of ids) {
        sentFromHere.add(id);
        sentFromHereAt.set(id, deps.now());
      }
      saveOwnPosts();
    },
    sentFromHere: (id) => sentFromHere.has(id),
    sentAt: (id) => sentFromHereAt.get(id),
    stands(id) {
      const at = sentFromHereAt.get(id);
      if (at === undefined) return false;
      if (deps.now() - at < OWN_POST_GRACE_MS) return true;
      sentFromHereAt.delete(id);
      return false;
    },
    removed(id) {
      if (sentFromHereAt.delete(id) || savedOwnPosts().has(id)) saveOwnPosts(id);
    },
    load() {
      for (const [id, at] of savedOwnPosts()) if (!sentFromHereAt.has(id)) sentFromHereAt.set(id, at);
    },
  };
}

export { OWN_POST_GRACE_MS };

// The removals upstream a channel's store has been told of, beyond its own record of them
// (`canon.removed`, ordered by when reads were sent): when the server said it observed each, on its
// own clock (`#473 replication-c7-server-versions`, `#315 removed-reply-target`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 10).

import type { Message } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import type { ServerVersions } from "./versions.ts";
import { serverVersions } from "./versions.ts";

/**
 * Message `id` is gone upstream — a removal from the stream (`noteLiveRemoval`), or a delta's
 * deletions (`foldTimelineDelta`) — so nothing left unplaced in store `canon` keeps it: no answer of
 * All sent before it was removed can put it back (`placeFromPage`), and none is asked for it. Not a
 * message a newest page of Main took out: Main never carries a thread reply, so that page cannot tell
 * one from a message removed, and the placing read is what puts such a reply back.
 */
export function forgetUnplaced(canon: ChannelCanon, id: string): void {
  canon.unplaced.delete(id);
  canon.unplacedSince.delete(id);
  canon.unplacedAnswered.delete(id);
}

/** What the record of removals does. */
export interface Removals {
  /** Message `id` is removed upstream, as of read `number`; `observedMs`, when the server observed it. */
  noteRemoved(canon: ChannelCanon, id: string, number: number, observedMs?: number | null): void;
  /** Whether a page whose versions are `versions` began before the server observed message `id`'s removal. */
  removedAfterRead(canon: ChannelCanon, id: string, versions: ServerVersions | null): boolean;
  /** A read sent after a removal that still carries the message says it is there after all. */
  believeCarried(canon: ChannelCanon, payload: { readonly messages?: readonly Message[] | null }, numberOf: () => number): void;
}

/**
 * An empty record. By channel, then message id, when the server observed each removal the page was
 * told of, on the server's clock: a page whose read began before it never brings the message back,
 * however late it was sent (the replication model's rule 3). Kept beside `removed`, which orders
 * removals by when reads were sent, for a server that says neither.
 */
export function createRemovals(): Removals {
  const removalsObserved = new Map<string, Map<string, number>>();
  /** The removals of `channel` the server said when it observed. */
  const observedFor = (channel: unknown): Map<string, number> => {
    const key = String(channel);
    let held = removalsObserved.get(key);
    if (!held) {
      held = new Map();
      removalsObserved.set(key, held);
    }
    return held;
  };
  const record: Removals = {
    /**
     * Message `id` is removed upstream — a removal from the stream (`noteLiveRemoval`), or a delta's
     * deletions (`foldTimelineDelta`): nothing left unplaced in store `canon` keeps it (`forgetUnplaced`),
     * and it is removed there (`canon.removed`) as of the read sent last, `number` (`timelineAnswersAsked`).
     * `#315 removed-reply-target`.
     *
     * NOT "NO LONGER UNPLACED". A message that is not unplaced is placed, to every other reader of that
     * set: the reply route took a bare copy the store still held, the read behind its removal not yet
     * landed, for a message on the main channel, and posted a reply to it there, with no read first —
     * during an outage, with no end to that window. Before `#308 live-reply-in-main` it was still
     * unplaced, and Send read All first, did not find it, and refused. Now a reply to a message removed
     * upstream is refused, saying so, wherever it was (`replyRoute`, `placeReplyTarget`), and nothing
     * places it (`placeFromPage`), until a read sent after the removal carries it after all
     * (`believeCarried`).
     *
     * `observedMs`: when the server observed the removal, if it said.
     */
    noteRemoved(canon, id, number, observedMs = null) {
      forgetUnplaced(canon, id);
      canon.removed.set(id, number);
      if (typeof observedMs === "number") observedFor(canon.channel).set(String(id), observedMs);
    },
    /**
     * Whether a page whose versions are `versions` (`serverVersions`) began before the server observed the
     * removal of message `id` of `canon`: it is older than that removal, and never brings the message back.
     */
    removedAfterRead(canon, id, versions) {
      if (!versions) return false;
      const at = observedFor(canon.channel).get(String(id));
      return at !== undefined && versions.observed <= at;
    },
    /**
     * A read sent after a message was removed upstream that still carries it says the message is there
     * after all — as the server's own read cache believes a fetch begun after a removal — and it is no
     * longer taken for removed (`noteRemoved`). `#315 removed-reply-target`. `numberOf`: where the read
     * was sent in the order of reads (`sentNumberOf`), asked only when the store holds a removal.
     */
    believeCarried(canon, payload, numberOf) {
      if (canon.removed.size === 0) return;
      const number = numberOf();
      const versions = serverVersions(payload);
      for (const message of payload.messages || []) {
        const at = canon.removed.get(String(message.id));
        // `#473`: not a page whose read began before the server observed the removal.
        if (record.removedAfterRead(canon, String(message.id), versions)) continue;
        if (at !== undefined && number > at) {
          canon.removed.delete(String(message.id));
          observedFor(canon.channel).delete(String(message.id));
        }
      }
    },
  };
  return record;
}

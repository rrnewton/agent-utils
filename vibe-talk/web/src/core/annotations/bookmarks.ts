// The reading bookmark, one per channel, kept on the server (`#240 reading-bookmark`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 11): what this page holds of each
// channel's, and the bookkeeping of its own writes, private to the owner and reached by its named
// commands. The write itself — its place in the queue of the reader's writes, the request, what is
// said when it fails — stays with the page; this is only what it changes, in the order it always did.

import type { BookmarkAnchor, BookmarkResponse, Message } from "../../contract/index.ts";

/**
 * A channel's bookmark as this page holds it. `revision` is the server's, as it last said; `anchor` is
 * where the line is, null for none, and may be a change the server has not answered yet, which `token`
 * then names; `known` is whether the server has said anything at all.
 */
export interface HeldBookmark {
  readonly revision: number;
  readonly anchor: BookmarkAnchor | null;
  readonly token: object | null;
  readonly known: boolean;
}

/** A bookmark as the server sends one, or as the device saved it. */
export interface BookmarkState {
  readonly revision: number;
  readonly anchor: BookmarkAnchor | null;
}

/** A change made here, at once, ahead of the write that tells the server (`stage`). */
export interface BookmarkMove {
  readonly channel: string;
  readonly before: BookmarkAnchor | null;
  readonly anchor: BookmarkAnchor | null;
  readonly token: object;
}

/** Whether two anchors put the line in the same place. */
export function sameAnchor(a: BookmarkAnchor | null | undefined, b: BookmarkAnchor | null | undefined): boolean {
  if (!a || !b) return !a && !b;
  return String(a.message_id) === String(b.message_id) && a.place === b.place;
}

/** Whether `value` is a bookmark as the server sends one: a page or to-do read is not decoded whole. */
export function isBookmarkState(value: unknown): value is BookmarkState {
  const held = value as { revision?: unknown; anchor?: unknown } | null | undefined;
  if (!held || typeof held !== "object" || typeof held.revision !== "number") return false;
  const anchor = held.anchor as { message_id?: unknown; place?: unknown; timestamp?: unknown } | null | undefined;
  return anchor === null || (Boolean(anchor) && typeof anchor === "object" && typeof anchor.message_id === "string" &&
    (anchor.place === "above" || anchor.place === "below") && typeof anchor.timestamp === "string");
}

/** The anchor for the line just above `message` (`above`), or just below it (`below`). */
export function anchorOfMessage(message: Message, place: "above" | "below"): BookmarkAnchor {
  const anchor: BookmarkAnchor = { message_id: String(message.id), place, timestamp: String(message.timestamp || "") };
  const thread = message && message.thread && typeof message.thread.id === "string" ? message.thread.id : null;
  if (thread) anchor.thread_id = thread;
  return anchor;
}

/** What the owner asks of the page. */
export interface BookmarksDependencies {
  /** Move the read-state epoch on, as every write's start and end does; answers the new one. */
  bumpEpoch(): number;
}

/** The owner's named commands. */
export interface Bookmarks {
  /** This channel's bookmark as held, or null. */
  held(channel: string): HeldBookmark | null;
  /** Whether one of this page's writes of `channel`'s bookmark is on its way. */
  writing(channel: string): boolean;
  /**
   * Forget every channel's bookmark: another credential. The writes on their way are still counted:
   * the server keeps one bookmark whoever asks, so a read sent before one of them settles does not
   * speak for it, whichever credential sent the read.
   */
  forget(): void;
  /**
   * A read sent at `sentEpoch` said where `channel`'s bookmark is, `said`. Taken as the truth unless a
   * write of this page's is on its way, or settled after the read was sent: then the read answers for
   * the state before it. Answers whether the line moved (false when the read was not taken).
   */
  note(channel: string, said: BookmarkState, sentEpoch: number): boolean;
  /**
   * `channel`'s bookmark for the device's saved copy of the channel, so a page opened offline draws the
   * line before any read: the server's last word, or a move of the owner's on its way. Undefined, and
   * so not saved, while the server has said nothing of it.
   */
  toSave(channel: string): BookmarkState | undefined;
  /**
   * A saved copy's bookmark, for a channel drawn from the device before any read: shown until the first
   * read says where it is now. Never over one this page holds already, which is newer: a read's, or the
   * owner's own move. Not `known`: it is not the server speaking now.
   */
  restoreSaved(channel: string, saved: unknown): void;
  /** Change `channel`'s bookmark here, at once, ahead of the write that tells the server. */
  stage(channel: string, anchor: BookmarkAnchor | null): BookmarkMove;
  /** The revision to make a conditional write of `channel`'s on: the one held when the write goes. */
  revision(channel: string): number | null;
  /** A write of `channel`'s ended, however it went: one fewer on its way, settled at a new epoch. */
  writeEnded(channel: string): void;
  /**
   * What a write of `move` settles, with the server's `answer` (null when it failed or was cancelled):
   * the server's revision taken, and the anchor the server says where the change is still this one's,
   * or taken back where it failed. Null when the bookmark is gone with the credential that made it;
   * else whether the change was still this one's, and the line before and after.
   */
  settle(move: BookmarkMove, answer: BookmarkResponse | null):
    { mine: boolean; before: BookmarkAnchor | null; after: BookmarkAnchor | null } | null;
}

/** An owner holding no channel's bookmark. */
export function createBookmarks(deps: BookmarksDependencies): Bookmarks {
  /** Each channel's bookmark as this page holds it, by channel id. */
  const bookmarks = new Map<string, HeldBookmark>();
  /**
   * Each channel's bookmark writes: how many are on their way, and the read-state epoch the last one
   * settled at. A read sent before that, or while one is on its way, does not speak for the bookmark:
   * it answers for the state before the owner's own act. `readSpeaksFor` is the same rule for Done.
   */
  const bookmarkWrites = new Map<string, { pending: number; from: number }>();
  return {
    held: (channel) => bookmarks.get(String(channel)) || null,
    writing(channel) {
      const write = bookmarkWrites.get(channel);
      return Boolean(write && write.pending > 0);
    },
    forget() {
      bookmarks.clear();
    },
    note(channel, said, sentEpoch) {
      const write = bookmarkWrites.get(channel);
      if (write && (write.pending > 0 || sentEpoch < write.from)) return false;
      const held = bookmarks.get(channel);
      const moved = !held || !sameAnchor(held.anchor, said.anchor);
      bookmarks.set(channel, { revision: said.revision, anchor: said.anchor || null, token: null, known: true });
      return moved;
    },
    toSave(channel) {
      const held = bookmarks.get(String(channel)) || null;
      return held && held.known ? { revision: held.revision, anchor: held.anchor } : undefined;
    },
    restoreSaved(channel, saved) {
      const key = String(channel);
      if (bookmarks.has(key) || !isBookmarkState(saved)) return;
      bookmarks.set(key, { revision: saved.revision, anchor: saved.anchor || null, token: null, known: false });
    },
    stage(channel, anchor) {
      const held = bookmarks.get(channel) || { revision: 0, anchor: null, token: null, known: false };
      const move = { channel, before: held.anchor, anchor, token: {} };
      bookmarks.set(channel, { ...held, anchor, token: move.token });
      const write = bookmarkWrites.get(channel) || { pending: 0, from: 0 };
      write.pending += 1;
      deps.bumpEpoch();
      bookmarkWrites.set(channel, write);
      return move;
    },
    revision(channel) {
      const held = bookmarks.get(channel);
      return held ? held.revision : null;
    },
    writeEnded(channel) {
      const write = bookmarkWrites.get(channel);
      if (write) {
        write.pending = Math.max(0, write.pending - 1);
        write.from = deps.bumpEpoch();
      }
    },
    settle(move, answer) {
      const { channel } = move;
      const held = bookmarks.get(channel);
      if (!held) return null;
      const mine = held.token === move.token;
      const before = held.anchor;
      if (answer) {
        const said = answer.bookmark;
        bookmarks.set(channel, {
          revision: said.revision,
          anchor: mine ? said.anchor || null : held.anchor,
          token: mine ? null : held.token,
          known: true,
        });
      } else if (mine) {
        bookmarks.set(channel, { ...held, anchor: move.before, token: null });
      }
      const now = bookmarks.get(channel) || null;
      return { mine, before, after: now ? now.anchor : null };
    },
  };
}

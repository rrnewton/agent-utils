// One store per channel, and the views projected from it (`#624 voice-page-modularity`, moved from
// the legacy script a group at a time; plan step 10).
//
// Main, Threads, All and each thread are FOUR ANSWERS FROM ONE CHANNEL, and the page holds the
// channel once: every message any view has read, by id, in time order, and every thread summary.
// A view is a projection of that store — Main is what was posted to the channel directly, thread
// roots included; All is everything; a thread is its own messages — so switching view is local
// work, a live arrival reaches every view at once, and a correction read in one view is the row
// every other view shows.

import type { Message, ThreadSummary } from "../../contract/index.ts";

/** How far back one view's own pages reached, and what its reads last said. */
export interface ViewCover {
  /** The oldest time its pages reached when there is more below; null for "unknown" or "all of it". */
  floor: number | null;
  /** Whether there is more of the view below the floor. */
  more: boolean;
  /** The server's notice with the view's last page ("" for none). */
  notice: string;
  /** `#578 search-status-clarity`. The newest top-level time any read of the view carried. */
  top: number | null;
  /** ...and where the stretch with no gaps begins, when it is not the floor. */
  dense: number | null;
  /** When the view's rows were last proved, by the wall clock. */
  at: number;
  /** Whether the cover was proved by a read in this page's life (not restored from a save). */
  live: boolean;
  /** The view's walk-back cursor, or null. */
  cursor: string | null;
  /**
   * `#203 incremental-refresh`. The forward cursor of the last newest read or delta folded, or null.
   * This and the next two are in memory only: absent on a cover restored from the device's save.
   */
  newest?: string | null;
  /** ...and when the last newest read landed. */
  fullAt?: number;
  /** `#308 live-reply-in-main`. Where that read was sent in the order of reads (`sentNumberOf`). */
  fullSent?: number;
  /** Main's cover borrowed from All's (`coverFromAll`). */
  fromAll?: boolean;
  /** Rows a landed page carried that the view hid. */
  landedHidden?: unknown;
}

/** The selected channel's store. */
export interface ChannelCanon {
  channel: string;
  messages: Message[];
  threads: ThreadSummary[];
  /** By view key ("main", "flat", "threads", "thread:<id>"), how far back that view is covered. */
  views: Map<string, ViewCover>;
  dismissed: Set<string>;
  hasThreads: boolean;
  /** The one thread a channel registered as a single conversation is, or null. */
  scope: string | null;
  /** `#185 reply-new-thread`. Ids that arrived with no thread record that no page has carried since. */
  unplaced: Set<string>;
  /**
   * `#308 live-reply-in-main`. By unplaced id, `unplacedArrivals` as its arrival made it — absent for
   * one saved before a reload — and the unplaced ids a read of All sent after they arrived has
   * answered for, carried or not (`placeUnplaced`).
   */
  unplacedSince: Map<string, number>;
  unplacedAnswered: Set<string>;
  /**
   * `#315 removed-reply-target`. By id, each message removed upstream — by the stream, or by a delta's
   * deletions — with `timelineAnswersAsked` as the removal came (`noteRemoved`). The store may still
   * hold it until the read the removal asks for lands; meanwhile nothing places it, and a reply to it
   * is refused (`replyRoute`, `placeReplyTarget`). Not "no longer unplaced", which reads as placed.
   */
  removed: Map<string, number>;
  /**
   * `#203 incremental-refresh`. Whether this channel's forward reads carry edits and deletions:
   * null until one has landed. The backend's property rather than a view's, so it outlives a
   * view's cover being reset by an explicit refresh.
   */
  complete: boolean | null;
  /**
   * `#222 unread-replies`. By thread id: how many of its replies the store lacks for being older
   * than anything read of it (`noteReplyGaps`). Absent is "not known yet".
   */
  replyGaps: Map<string, number>;
  /**
   * ...and the threads whose gap is not known because All's newest page carried their root and
   * none of their replies, saying it leaves some out (Slack's threads past fifty replies, and before
   * `#603 server-side-search` past its ten threads a page): All reaching such a root says nothing of
   * its replies' age (`noteReplyGaps`).
   */
  passedOver: Set<string>;
  /**
   * ...and by thread id, the gap Done on the root set for a thread whose gap was not known
   * (`settleUnknownReplies`), until a read sets one of its own (a read that only lowers it leaves
   * it Done's, lowered): putting the root back takes it back (`unsettleReplies`).
   */
  settledByDone: Map<string, number>;
}

/** An empty store for `channel`. */
export function emptyCanon(channel: unknown = ""): ChannelCanon {
  return {
    channel: String(channel), messages: [], threads: [], views: new Map(), dismissed: new Set(), hasThreads: false,
    scope: null, unplaced: new Set(),
    unplacedSince: new Map(), unplacedAnswered: new Set(),
    removed: new Map(),
    complete: null,
    replyGaps: new Map(),
    passedOver: new Set(),
    settledByDone: new Map(),
  };
}

/** A view key's view and thread. */
export function viewOfKey(key: string): { view: string; thread: string | null } {
  return key.startsWith("thread:") ? { view: "thread", thread: key.slice("thread:".length) } : { view: key, thread: null };
}

/** The time `item[field]` names, in milliseconds; NaN where it names none. */
export const timeOf = (item: unknown, field: string): number =>
  Date.parse((item && (item as Record<string, unknown>)[field]) as string);

/** Stable by time when every row has one; otherwise in the order given, which is the server's. */
export function inTimeOrder<T>(items: T[], field: string): T[] {
  const times = items.map((item) => timeOf(item, field));
  if (!times.every(Number.isFinite)) return items;
  return items
    .map((item, index) => ({ item, at: times[index] as number, index }))
    .sort((a, b) => a.at - b.at || a.index - b.index)
    .map(({ item }) => item);
}

/** The id of the thread `message` is in, or null. */
export function threadOf(message: unknown): string | null {
  const thread = message ? (message as { thread?: { id?: unknown } | null }).thread : null;
  return thread && typeof thread.id === "string" ? thread.id : null;
}

/**
 * Whether `message` belongs in `view`: the same rule the server's timeline applies.
 *
 * `scope` is the one thread a channel registered as a single conversation is: its Main is that
 * whole conversation, root and replies, so a reply in it is a Main row. A channel with child
 * threads has no scope, and its Main keeps only roots and unthreaded messages.
 */
export function inViewOf(message: Message, view: string, thread: string | null, scope: string | null): boolean {
  const id = threadOf(message);
  if (view === "flat") return true;
  if (view === "thread") return id === thread;
  return !id || (message.thread as { is_root?: unknown }).is_root === true || id === scope;
}

/** Whether `held` is `arriving`, word for word: the same message, to draw and to keep. */
export const sameMessage = (held: unknown, arriving: unknown): boolean =>
  held !== undefined && JSON.stringify(held) === JSON.stringify(arriving);

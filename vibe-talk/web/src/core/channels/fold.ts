// Folding what the server answers into one channel's store (`#624 voice-page-modularity`, moved from
// the legacy script a group at a time; plan step 10). Each takes the store it works on.

import type { Message, MessageThread, ThreadSummary } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { inTimeOrder, sameMessage, threadOf, timeOf } from "./canon.ts";

/** A row of either list the store holds. */
type Row = Message | ThreadSummary;

/** `#473 replication-c7-server-versions`. How new a page of the server's is, on the server's clock. */
export interface ServerVersions {
  readonly observed: number;
  readonly settledBelow: number | null;
}

/**
 * Fold one page into the store. A NEWEST page is the truth for its view from its oldest row up, so
 * a row of that view it does not carry there was deleted upstream and goes; an older page only
 * adds. Rows another view holds are untouched either way. `page` is every row the page carries, where
 * it reaches back to, when `arriving` is only the rows the store takes of it (`pageFold`).
 *
 * `#328 removal-outside-window`. Below the page's oldest row a newest page speaks for nothing, so a row
 * held there stays, except one removed upstream (`noteRemoved`): the stream has said it is gone, and
 * the page, which answers for its view, does not bring it back. It used to stay, shown and saved, until
 * a read reached back to it, which a removed message never does: the README's "the first newest page to
 * land takes the message out" was true only within that page's window.
 */
export function mergeIntoCanon(
  canon: ChannelCanon,
  field: "messages",
  arriving: Message[],
  older: boolean,
  hasMore: boolean,
  belongs: (item: Message) => boolean,
  page?: Message[],
): void;
export function mergeIntoCanon(
  canon: ChannelCanon,
  field: "threads",
  arriving: ThreadSummary[],
  older: boolean,
  hasMore: boolean,
  belongs: (item: ThreadSummary) => boolean,
  page?: ThreadSummary[],
): void;
export function mergeIntoCanon(
  canon: ChannelCanon,
  field: "messages" | "threads",
  arriving: Row[],
  older: boolean,
  hasMore: boolean,
  belongsTo: ((item: Message) => boolean) | ((item: ThreadSummary) => boolean),
  page: Row[] = arriving,
): void {
  const belongs = belongsTo as (item: Row) => boolean;
  const timeField = field === "threads" ? "updated_at" : "timestamp";
  // A message the store holds word for word keeps the store's object, the one the rows on screen
  // were drawn from, as a delta's does (`foldTimelineDelta`). `#229 desktop-two-column`: a thread's
  // page carries its root, which Main shows too, and replacing that object would make Main's next
  // refresh that brings nothing redraw its rows all the same (`rowsAsDrawn` compares objects).
  if (field === "messages") {
    const held = heldCopies(canon, arriving as Message[]);
    arriving = (arriving as Message[]).map((item) => {
      const copy = held.get(String(item.id));
      if (copy && sameMessage(copy, item)) return copy;
      // `#408 copy-upstream-link`: as in `foldLiveMessage`, a copy formed without the
      // message's upstream address — a read answered before the server learned its workspace
      // — does not take a held copy's away.
      if (copy && copy.upstream_url && !item.upstream_url) return { ...item, upstream_url: copy.upstream_url };
      return item;
    });
  }
  const incoming = new Set(arriving.map((item) => String(item.id)));
  const edge = page.length ? timeOf(page[0], timeField) : NaN;
  const removed = (item: Row) => field === "messages" && canon.removed.has(String(item.id));
  const kept = (canon[field] as Row[]).filter((item) => !incoming.has(String(item.id)) &&
    (older || !belongs(item) || (hasMore && Number.isFinite(edge) && timeOf(item, timeField) < edge && !removed(item))));
  const merged = inTimeOrder(older ? [...arriving, ...kept] : [...kept, ...arriving], timeField);
  if (field === "messages") canon.messages = merged as Message[];
  else canon.threads = merged as ThreadSummary[];
}

/** A thread's summary, from a thread page or a live reply, replacing the one held by id. */
export function upsertThreadSummary(canon: ChannelCanon, summary: ThreadSummary): void {
  const id = String(summary.id);
  canon.threads = inTimeOrder(
    [...canon.threads.filter((held) => String(held.id) !== id), summary], "updated_at");
}

/**
 * `#473 replication-c7-server-versions`. How new a page of the server's is, on the server's clock: when
 * its read began (`observed_ms`), and on a newest page the provider time below which its absence of a
 * message proves the message gone (`settled_below_ms`). Null from an older server, which says neither:
 * the page then orders copies by when it sent its reads (`copyOrder`), as before.
 *
 */
export function serverVersions(payload: unknown): ServerVersions | null {
  const page = payload as { observed_ms?: unknown; settled_below_ms?: unknown } | null | undefined;
  if (!page || typeof page.observed_ms !== "number") return null;
  return {
    observed: page.observed_ms,
    settledBelow: typeof page.settled_below_ms === "number" ? page.settled_below_ms : null,
  };
}

/**
 * `#473`. Whether a page whose versions are `versions` takes its copy of a message, `offered`, over the
 * store's, `held`: true when the store holds none; by edit time where both say one (an earlier edit
 * never replaces a later one, however late it was seen); otherwise by the server's observation of each,
 * the page's being when its read began. Null where the store's copy does not say how new it is (saved
 * by an older build, or put together by one): the caller decides as before.
 *
 */
export function pageCopyNewer(held: Message | undefined, offered: Message, versions: { observed: number }): boolean | null {
  if (!held) return null;
  if (typeof held.observed_ms !== "number") return null;
  const heldEdit = held.edited_timestamp ? Date.parse(held.edited_timestamp) : NaN;
  const offeredEdit = offered.edited_timestamp ? Date.parse(offered.edited_timestamp) : NaN;
  if (Number.isFinite(heldEdit) && Number.isFinite(offeredEdit) && heldEdit !== offeredEdit) {
    return offeredEdit > heldEdit;
  }
  const seen = typeof offered.observed_ms === "number" ? offered.observed_ms : versions.observed;
  return seen >= held.observed_ms;
}

/**
 * `#473`. Whether a page whose versions are `versions`, which does not carry the store's copy `held` of a
 * message, says the message is gone: only a newest page (one that says where its proven stretch ends),
 * only for a copy the server observed before the page's read began, and only for a message created
 * below that stretch. A copy observed after the read began — the reply just sent, a message the stream
 * delivered — is newer than the page; one created in the unsettled band may simply not be listed yet.
 * Null where the store's copy does not say how new it is: the caller decides as before.
 *
 */
export function goneByAbsence(held: Message, versions: ServerVersions): boolean | null {
  if (typeof held.observed_ms !== "number") return null;
  if (versions.settledBelow === null) return false;
  const created = timeOf(held, "timestamp");
  return held.observed_ms < versions.observed && Number.isFinite(created) && created < versions.settledBelow;
}

/**
 * Which held messages a newest page of All answers for, `messages` being the page's and `bounded`
 * whether it carries a notice: `mergeIntoCanon` drops one of those that the page does not carry, as
 * deleted upstream. `#222 unread-replies`.
 *
 * NOT EVERY REPLY ITS WINDOW SPANS. Slack's All is Main's page with the replies of at most ten of
 * its threads, those with the latest activity, and the newest fifty of each, and when that leaves
 * anything out the page's notice says so. A thread outside those ten has nothing on the page, though
 * its replies are as new as the rows around them, and dropping the ones held — from an earlier page
 * of All, the thread's own read — took an unread answer out of the count on its root: the chip went,
 * and Hide read hid the root, after nothing more than a look at All.
 *
 * So a page answers for EVERY reply of a thread whose root it carries when it says nothing of
 * leaving anything out — a backend whose All is the whole channel by time carries every reply posted
 * since the root, and Slack's carries every reply of each root when it reads them all — or when it
 * carries as many replies as that root says the thread has, none included: then a reply held and not
 * carried was deleted upstream, and goes (it used to stay counted as unread, and keep its root, until
 * the thread was read). Otherwise it answers for a thread's replies from the oldest one it carries
 * up, and for none of a thread it carries nothing of. Every other message — a root, a message in no
 * thread, a reply in the one thread a channel is — is answered for as before. What this still keeps
 * that was dropped before is, on a backend whose All is the whole channel by time, a reply deleted
 * upstream of a thread whose root is older than the page, until the thread is read.
 *
 */
export function allPageAnswersFor(canon: ChannelCanon, messages: Message[], bounded: boolean): (message: Message) => boolean {
  /** By thread id, the time of the oldest reply the page carries. */
  const oldest = new Map<string, number>();
  /** By thread id, how many of its replies the page carries. */
  const carried = new Map<string, number>();
  /** The threads whose root the page carries, to the count it gives. */
  const roots = new Map<string, number | null>();
  for (const message of messages) {
    const id = threadOf(message);
    if (!id) continue;
    const thread = message.thread as MessageThread;
    if (thread.is_root === true) {
      roots.set(id, thread.reply_count === 0 ? 0 : exactCount(thread));
      continue;
    }
    carried.set(id, (carried.get(id) || 0) + 1);
    const at = timeOf(message, "timestamp");
    const known = oldest.get(id);
    oldest.set(id, known === undefined || at < known ? at : known);
  }
  /** The threads every reply of which the page answers for. */
  const whole = new Set<string>();
  for (const [id, count] of roots) {
    if (!bounded || (count !== null && (carried.get(id) || 0) >= count)) whole.add(id);
  }
  return (message) => {
    const id = threadOf(message);
    if (!id || (message.thread as MessageThread).is_root === true || id === canon.scope || whole.has(id)) return true;
    const from = oldest.get(id);
    return from !== undefined && !(timeOf(message, "timestamp") < from);
  };
}

/** The store's copies of `messages`, by id, for those it holds. One walk over the store. */
export function heldCopies(canon: ChannelCanon, messages: Message[]): Map<string, Message> {
  const wanted = new Set(messages.map((message) => String(message.id)));
  const found = new Map<string, Message>();
  if (wanted.size === 0) return found;
  for (const held of canon.messages) {
    if (wanted.has(String(held.id))) found.set(String(held.id), held);
  }
  return found;
}

/**
 * Whether the server says it could not read the chat service for this page and answered from its own
 * copy instead (`refresh_failed`, `#611 timeline-read-failures`). Its rows are added and updated as any
 * page's are, but it answers for nothing it lacks and moves no cover, cursor or full read
 * (`foldTimelinePage`); the pill says "Refresh failed · showing messages from …", quietly, over them.
 */
export function refreshFailed(payload: unknown): boolean {
  return Boolean(payload) && typeof (payload as { refresh_failed?: unknown }).refresh_failed === "string";
}

/** A record's or a summary's reply count when it says the count is exact, or null. */
export function exactCount(record: unknown): number | null {
  const held = record as { reply_count?: unknown; reply_count_exact?: unknown } | null | undefined;
  return held && typeof held.reply_count === "number" && held.reply_count_exact !== false
    ? held.reply_count : null;
}

/**
 * What a delta can change in the store besides its rows — the counts of replies the store lacks,
 * what Done settled of them, the threads passed over, what the channel is scoped to, whether it has
 * threads, and the summary of thread `thread` it carries — in store `canon`, to be compared before and
 * after one is folded.
 */
export function deltaSideState(canon: ChannelCanon, thread: { id: unknown } | null | undefined): string {
  const summary = thread ? canon.threads.find((held) => String(held.id) === String(thread.id)) : null;
  return JSON.stringify([[...canon.replyGaps], [...canon.settledByDone], [...canon.passedOver],
    canon.scope, canon.hasThreads, summary || null]);
}

/**
 * Whether message `id` was removed upstream (`noteRemoved`) after the read numbered `number`
 * (`sentNumberOf`) was sent: that read is older than the removal, and never brings the message back.
 * `#316 late-counts-resurrect`.
 */
export function removedSince(canon: ChannelCanon, id: string, number: number): boolean {
  const at = canon.removed.get(id);
  return at !== undefined && at >= number;
}

/**
 * The store's copy of each message store `canon` holds now, by id: what a placing read sent now is
 * folded over only if the store still holds it when the read lands (`heldAsSent`), and what tells a
 * page sent now whether a copy with no place in the order of reads is older than it (`copyOlderThan`).
 */
export function heldNow(canon: ChannelCanon): Map<string, Message> {
  return new Map(canon.messages.map((message) => [String(message.id), message]));
}

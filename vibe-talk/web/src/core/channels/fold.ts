// Folding what the server answers into one channel's store (`#624 voice-page-modularity`, moved from
// the legacy script a group at a time; plan step 10). Each takes the store it works on.

import type { Message, MessageThread, ThreadSummary, TimelineDelta, TimelineResponse } from "../../contract/index.ts";
import type { ChannelCanon, ViewCover } from "./canon.ts";
import type { CopyOrder } from "./order.ts";
import type { Removals } from "./removals.ts";
import type { ServerVersions } from "./versions.ts";
import { serverVersions } from "./versions.ts";
import { inTimeOrder, inViewOf, sameMessage, threadOf, timeOf } from "./canon.ts";
import { placeFromPage } from "./placement.ts";
import { coverViewOf } from "./views.ts";

/** A row of either list the store holds. */
type Row = Message | ThreadSummary;


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

/** What `pageFoldIn` asks of the page: where the read was sent, the copy ledger, and three questions. */
export interface PageFoldDependencies {
  /** Where the page's read was sent in the order of every timeline read (`sentNumberOf`). */
  readonly number: number;
  /** The copy-order ledger (`createCopyOrder`). */
  readonly ledger: CopyOrder;
  /** Whether a page with versions `versions` began before the server observed message `id`'s removal. */
  removedAfterRead(canon: ChannelCanon, id: string, versions: ServerVersions | null): boolean;
  /** Whether a newest page that speaks for `message`, sent after read `number`, has landed. */
  newerPageSpeaksFor(canon: ChannelCanon, message: Message, number: number): boolean;
  /** Whether the reader's own fresh post `id` still stands against a page's absence of it. */
  ownPostStands(id: string): boolean;
}

/** What one page may fold: the messages it takes its copy of, and those it answers for. */
export interface PageFold {
  takes(message: Message): boolean;
  answers(message: Message): boolean;
}

/**
 * What a page of a view, `payload`, may fold into store `canon` (`foldTimelinePage`), its read sent
 * at `deps.number` in the order of reads (`sentNumberOf`): the messages it
 * `takes` its copy of, and those it `answers` for — takes out as deleted where it does not carry them.
 * `#316 late-counts-resurrect`.
 *
 * NEVER A MESSAGE REMOVED UPSTREAM AFTER IT WAS SENT (`removedSince`), AND THAT ONE ALWAYS OUT. It is
 * older than the removal, so it takes no copy of the message: the read of All for Main's counts, sent
 * before a removal and landing after Main's read had taken the message out, brought it back onto Main
 * until the next full read. And it takes the message out, whatever it carries: the removal is newer,
 * and says the message is gone. That used to be left to the read behind the removal, which may never
 * say it: a newest page sent before the removal and answered after it does not carry the message, and
 * its cursor is past the deletion, so the read behind the removal, a delta from that cursor, did not
 * report it either, and the message stayed shown and saved until the next newest page, up to
 * `TIMELINE_RECONCILE_MS` later. A removal from the stream's replay tail asks for no read at all.
 *
 * AND WITH `held` — the store's copies as the read was sent (`heldNow`) — ONLY OVER THE COPIES OLDER
 * THAN IT (`copyOlderThan`), in the order reads were sent: a copy a read wrote stands where that read
 * was sent, a message from the stream where it went into the store, and a count or a thread record put
 * on here where the copy it was put on stood (`copyBuiltOn`). It takes its copy of a message whose copy
 * in the store is older, or says the message as it was posted (`postedCopies`), and of one the store has
 * not held at all — unless a newest page of the view that message belongs in, its thread or Main, sent
 * after it has landed (`newerPageSpeaksFor`), which speaks for it and did not leave it in the store, as
 * an answer of All that is not folded puts back nothing such a page took out (`placeFromPage`). It
 * answers only for the messages whose copy is older, and those removed since. A copy newer than the
 * page — an edit, a thread record or a count that a read sent after it brought, a message the stream
 * delivered after it was sent — it would undo, or take out as deleted (`#310 stale-live-read-flicker`);
 * a message taken out since it was sent, it would put back. But the stream's copy of a message is newer
 * only in being there: the page, which carries the message as it was posted or newer, takes its own
 * over it. Refused, a placeholder the stream drew while the page was out, and a bot then edited to its
 * answer, stayed a placeholder until the next newest page. And a copy that an OLDER read brought while
 * it was out is older than the page, which takes its own over it: the read of All for Main's counts,
 * answered after one edit and landing while Main's newer newest page was out, made that page refuse its
 * copy of the next edit, or of a removal, until the next newest page. Of what it does not take, a
 * message the stream left unplaced still takes its thread record (`placeFromPage`), and the counts,
 * which come of the replies it brings that the store never held, are what such a read is for.
 *
 */
export function pageFoldIn(
  canon: ChannelCanon,
  payload: TimelineResponse,
  held: Map<string, Message> | null,
  deps: PageFoldDependencies,
): PageFold {
  const number = deps.number;
  const holds = held ? heldNow(canon) : null;
  const versions = serverVersions(payload);
  /** The store's copies by id, for the server's versions: built once. */
  let stored: Map<string, Message> | null = null;
  const storedNow = () => {
    if (!stored) stored = holds || new Map(canon.messages.map((copy) => [String(copy.id), copy]));
    return stored;
  };
  /** The ids the page carries, for the server's versions: built once. */
  let carriedIds: Set<string> | null = null;
  const carried = () => {
    if (!carriedIds) carriedIds = new Set((payload.messages || []).map((copy) => String(copy.id)));
    return carriedIds;
  };
  /** Whether the store holds a copy of message `id` older than this page; `says`, older in what it says. */
  const older = (id: string, says = false) => {
    const copy = holds && holds.get(id);
    return Boolean(held && copy && deps.ledger.copyOlderThan(copy, id, number, held, says));
  };
  return {
    takes: (message) => {
      const id = String(message.id);
      if (removedSince(canon, id, number) || deps.removedAfterRead(canon, id, versions)) return false;
      // `#473 replication-c7-server-versions`: where the server says how new both copies are, that
      // decides, whatever order the reads were sent in.
      const kept = versions ? storedNow().get(id) : undefined;
      // A copy that says the message as it was posted (the stream's, the reader's own send) is older
      // in what it says than any read that carries the message, however late it arrived.
      const byServer = kept && deps.ledger.isPosted(kept) ? true : versions ? pageCopyNewer(kept, message, versions) : null;
      if (byServer !== null) return byServer;
      if (!held || !holds) return true;
      if (holds.has(id)) return older(id, true);
      // Taken out since it was sent.
      if (held.has(id)) return false;
      return !deps.newerPageSpeaksFor(canon, message, number);
    },
    answers: (message) => {
      const id = String(message.id);
      if (removedSince(canon, id, number) || deps.removedAfterRead(canon, id, versions)) return true;
      // `#473 replication-c7-server-versions`: the replication model's rule 4, as the server applies
      // it. A held copy is gone by its absence only where the server says it observed that copy
      // before this page's read began and it was created below the stretch the page proves.
      // A message the page carries is there, whichever copy of it the store keeps.
      const byServer = versions ? (carried().has(id) ? false : goneByAbsence(message, versions)) : null;
      if (byServer !== null) return byServer;
      return (!held || older(id)) && (canon.removed.has(id) || !deps.ownPostStands(message.id));
    },
  };
}

/** What `foldDeltaIn` asks of the page, each at the point the fold always did it. */
export interface DeltaFoldDependencies {
  /** The copy-order ledger. */
  readonly ledger: CopyOrder;
  /** The record of removals. */
  readonly removals: Removals;
  /** Where the delta's read was sent in the order of reads (`sentNumberOf`). */
  numberOf(): number;
  /** How many timeline reads have been sent, as a removal is noted (`timelineAnswersAsked`). */
  readsSent(): number;
  /** A message the delta deleted is no longer Done here (`archivedIds`). */
  forgetArchived(id: string): void;
  /** Fold the delta's read state; whether it changed any (`foldReadState`). */
  foldReadState(): boolean;
  /** What the delta says of the replies the store lacks, `before` its copies were folded (`noteReplyGaps`). */
  noteReplyGaps(before: Map<string, Message>): void;
  /** The delta changed nothing (`unchangedDeltas`). */
  markUnchanged(): void;
  /** When the server says the delta is complete up to, if old enough to say so (`staleAsOf`). */
  staleAsOf(): number | null;
}

/**
 * Fold a DELTA — what changed in view `view` (under view key `key`) since its forward cursor `sent` —
 * into store `canon`. `#203
 * incremental-refresh`. False, and nothing touched, when the cover no longer holds the cursor the
 * read was sent with: a newest read or another delta landed in between, and the next read asks
 * again from where that one left the view.
 *
 * NEVER THE NEWEST PAGE'S RULE. A row a delta does not carry is unchanged, not deleted: removal is
 * said only by `delta.deleted`, channel-wide, and by `delta.removed_threads`. Coverage is not
 * changed either — a delta continues the view upward from its previous newest position without a
 * gap, so the floor and whether older history exists stay as they were, and "a projection never
 * invents coverage" still holds. Its rows are the server's own answer, so a thread record on one
 * replaces the store's, and the id leaves `unplaced`, as a page row does (`#185`).
 *
 * `#226 page-energy-profile`. A copy the store holds word for word changes nothing, so the store
 * keeps its own object, the one the row on screen was drawn from. A delta that changed nothing at
 * all — no row, no deletion, no read state, no thread, no count, no notice — is put in
 * `unchangedDeltas` (`deps.markUnchanged`), which is what lets a refresh with nothing new draw nothing
 * (`rowsAsDrawn`).
 */
export function foldDeltaIn(
  canon: ChannelCanon,
  payload: TimelineResponse,
  sent: string | null | undefined,
  view: string,
  key: string,
  deps: DeltaFoldDependencies,
): boolean {
  const cover = canon.views.get(key);
  if (!cover || !sent || cover.newest !== sent) return false;
  const delta = payload.delta as TimelineDelta;
  const sideBefore = deltaSideState(canon, payload.thread);
  const gone = new Set((delta.deleted || []).map(String));
  if (gone.size) {
    canon.messages = canon.messages.filter((message) => !gone.has(String(message.id)));
    for (const id of gone) {
      canon.dismissed.delete(id);
      deps.removals.noteRemoved(canon, id, deps.readsSent(), typeof payload.observed_ms === "number" ? payload.observed_ms : null);
      deps.forgetArchived(id);
    }
  }
  // Never a message removed upstream after it was sent (`removedSince`, `#316 late-counts-resurrect`).
  const number = deps.numberOf();
  const unfiltered = (payload.messages || []).filter((message) => !removedSince(canon, String(message.id), number));
  const before = heldCopies(canon, unfiltered);
  // `#408 copy-upstream-link`: as in `foldLiveMessage`, a copy formed without the message's
  // upstream address does not take a held copy's away. Before anything compares or orders
  // them, so the copy that is kept is the copy that is counted.
  const arriving = unfiltered.map((message) => {
    const held = before.get(String(message.id));
    return held && held.upstream_url && !message.upstream_url
      ? { ...message, upstream_url: held.upstream_url }
      : message;
  });
  const fresh = arriving.filter((message) => !sameMessage(before.get(String(message.id)), message));
  if (fresh.length) {
    const ids = new Set(fresh.map((message) => String(message.id)));
    canon.messages = inTimeOrder(
      [...canon.messages.filter((held) => !ids.has(String(held.id))), ...fresh], "timestamp");
  }
  // What it brought, or found word for word, stands where it was sent (`copyOrder`).
  deps.ledger.orderCopiesRead(canon, arriving, number);
  for (const message of arriving) canon.unplaced.delete(String(message.id));
  deps.removals.believeCarried(canon, payload, deps.numberOf);
  const readChanged = deps.foldReadState();
  for (const summary of payload.threads || []) upsertThreadSummary(canon, summary);
  const removed = new Set((delta.removed_threads || []).map(String));
  if (removed.size) canon.threads = canon.threads.filter((held) => !removed.has(String(held.id)));
  if (payload.thread) upsertThreadSummary(canon, payload.thread);
  for (const summary of [...(payload.threads || []), payload.thread]) {
    if (summary && summary.root) canon.unplaced.delete(String(summary.root.id));
  }
  if (view !== "thread") {
    canon.scope = payload.thread ? String(payload.thread.id) : null;
    canon.hasThreads = payload.has_threads === true;
  }
  deps.noteReplyGaps(before);
  if (!gone.size && !fresh.length && !readChanged && !(payload.threads || []).length && !removed.size &&
      (payload.notice || "") === (cover.notice || "") && deltaSideState(canon, payload.thread) === sideBefore) {
    deps.markUnchanged();
  }
  const asOf = deps.staleAsOf();
  // `#578 search-status-clarity`. A delta continues the view up from where it was without a gap, so
  // the newest message it carries is one the next newest read may join (`coverView`).
  const carried = arriving.filter((message) => view !== "flat" || !threadOf(message) || (message.thread as MessageThread).is_root === true)
    .map((message) => timeOf(message, "timestamp")).filter(Number.isFinite);
  const top = Math.max(Number.isFinite(cover.top) ? (cover.top as number) : -Infinity, ...carried);
  canon.views.set(key, {
    ...cover,
    notice: payload.notice || "",
    at: asOf === null ? Date.now() : asOf,
    live: true,
    newest: payload.next_after || null,
    top: Number.isFinite(top) ? top : cover.top ?? null,
  });
  canon.complete = delta.complete === true;
  return true;
}

/** What `foldPageIn` asks of the page, each at the point the fold always did it. */
export interface PageFoldInDependencies {
  /** The copy-order ledger. */
  readonly ledger: CopyOrder;
  /** The record of removals. */
  readonly removals: Removals;
  /** Where the page's read was sent in the order of reads (`sentNumberOf`). */
  numberOf(): number;
  /** What the page may fold over the store's copies as its read was sent, `held` (`pageFold`). */
  foldOf(held: Map<string, Message> | null): PageFold;
  /** The channel's scope changed: a channel that is one thread has no other to show beside it. */
  scopeChanged(): void;
  /** When the server says the page is complete up to, if old enough to say so (`staleAsOf`). */
  staleAsOf(): number | null;
  /** Fold the page's read state (`foldReadState`). */
  foldReadState(): void;
  /** What the page says of the replies the store lacks, `before` its copies were folded (`noteReplyGaps`). */
  noteReplyGaps(before: Map<string, Message>): void;
}

/**
 * Fold a page read for view `view` (thread `thread`, under view key `key`) into store `canon`. Never a message removed
 * upstream after the read was sent; and `held`, when given, is the store's copies as a late read was
 * sent (`heldNow`): it is folded only over the copies older than it (`pageFold`, `#316
 * late-counts-resurrect`, `#310 stale-live-read-flicker`). What it takes stands where it was sent
 * (`copyOrder`).
 */
export function foldPageIn(
  canon: ChannelCanon,
  payload: TimelineResponse,
  older: boolean,
  view: string,
  thread: string | null,
  key: string,
  held: Map<string, Message> | null,
  deps: PageFoldInDependencies,
): void {
  const hasMore = payload.has_more === true;
  // Outside the thread view a page names a thread only when the channel IS that one thread.
  const scopeWas = canon.scope;
  if (view !== "thread") canon.scope = payload.thread ? String(payload.thread.id) : null;
  // A channel that is one thread has no other thread to show beside it. `#229 desktop-two-column`.
  if (canon.scope !== scopeWas) deps.scopeChanged();
  const before = view === "threads" ? new Map<string, Message>() : heldCopies(canon, payload.messages || []);
  const fold = deps.foldOf(held);
  // `#611 timeline-read-failures`. A page the server answered from its own copy, the chat service
  // having failed or kept it waiting, adds and updates rows and answers for nothing it lacks: the
  // copy may have gaps and may lag. Nor does it move a view already covered, its forward cursor or
  // its last full read, so the next read reads as if this one had failed.
  const failed = refreshFailed(payload);
  const covered = canon.views.has(key);
  const taken = view === "threads" ? [] : (payload.messages || []).filter(fold.takes);
  if (view === "threads") {
    // `#614 timeline-read-failures`: a thread list from the server's own copy answers for no card
    // it lacks either.
    mergeIntoCanon(canon, "threads", payload.threads || [], older, hasMore, () => !failed);
  } else {
    const inPage = view === "flat" ? allPageAnswersFor(canon, payload.messages || [], Boolean(payload.notice))
      : (message: Message) => inViewOf(message, view, thread, canon.scope);
    mergeIntoCanon(canon, "messages", taken, older, hasMore,
      (message) => !failed && fold.answers(message) && inPage(message), payload.messages || []);
    // What it took stands where it was sent (`copyOrder`).
    deps.ledger.orderCopiesRead(canon, taken, deps.numberOf());
  }
  if (payload.thread) upsertThreadSummary(canon, payload.thread);
  if (view !== "thread") canon.hasThreads = payload.has_threads === true;
  if (!failed || !covered) {
    coverViewOf(canon, key, view, view === "threads" ? payload.threads || [] : payload.messages || [], older, hasMore,
      payload.notice || "");
  }
  if (!older) {
    const cover = canon.views.get(key) as ViewCover;
    if (!failed) {
      cover.newest = payload.next_after || null;
      cover.fullAt = Date.now();
      cover.fullSent = deps.numberOf();
    }
    const asOf = deps.staleAsOf();
    if (asOf !== null) cover.at = asOf;
  }
  // A page is the server's own answer for every message on it, thread record or none...
  for (const message of taken) canon.unplaced.delete(String(message.id));
  deps.removals.believeCarried(canon, payload, deps.numberOf);
  for (const summary of [...(payload.threads || []), payload.thread]) {
    if (summary && summary.root) canon.unplaced.delete(String(summary.root.id));
  }
  // ...and one it did not take the copy of still says where it is, if the stream left that unplaced.
  if (view !== "threads" && taken.length < (payload.messages || []).length) {
    placeFromPage(payload, canon, deps.numberOf(), held, deps.ledger);
  }
  // ...and read state too, and what it says of the replies the store lacks. `#222 unread-replies`.
  deps.foldReadState();
  deps.noteReplyGaps(before);
}

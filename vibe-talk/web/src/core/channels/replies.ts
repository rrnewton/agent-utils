// The replies a channel's store lacks for being older than anything read of them, by thread (`#222
// unread-replies`; `#624 voice-page-modularity`, moved from the legacy script; plan step 10).

import type { Message, MessageThread, ThreadSummary } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { threadOf, timeOf } from "./canon.ts";
import { exactCount, upsertThreadSummary } from "./fold.ts";

/** What a page or a delta brings that the gaps read: its rows, the thread it is of, its notice. */
export interface GapPage {
  readonly messages?: Message[] | null;
  readonly thread?: ThreadSummary | null;
  readonly notice?: string | null;
}

/**
 * Whether All's cover in store `canon` reaches thread `id` from its first message: All is the whole channel, or the
 * thread's root — held, or carried by its summary — is no older than the oldest row All covers.
 */
export function allReachesThread(canon: ChannelCanon, id: unknown, summary: ThreadSummary | null | undefined): boolean {
  const all = canon.views.get("flat");
  if (!all) return false;
  if (!all.more) return true;
  const held = canon.threads.find((candidate) => String(candidate.id) === String(id)) || summary;
  const root = canon.messages.find((message) => threadOf(message) === String(id) && (message.thread as MessageThread).is_root) ||
    (held && held.root) || null;
  const startedAt = timeOf(root, "timestamp");
  return all.floor !== null && Number.isFinite(startedAt) && startedAt >= all.floor;
}

/**
 * What one page says of the replies store `canon` lacks for being old, by thread, into
 * `canon.replyGaps`. `#222 unread-replies`. Called after the page is folded; `before` is the
 * store's copies of the page's messages from before it was, by id.
 *
 * A thread's gap is its exact count less the replies held, at a moment the held ones are known to
 * reach its newest: then every one missing is older than anything read of it. So it is SET by a read
 * that brings a thread's newest replies together with its count — the thread's own newest page or
 * delta, with the summary it carries, and All's newest page, for each root on it whose thread it
 * carries replies of — and otherwise only ever lowered: by each reply a step back brings (one fewer
 * is missing), and by a count that has dropped below it (old replies were deleted, or brought). What
 * a count says beyond the gap is new (`unseenReplies`).
 *
 * NOT FOR A THREAD ALL'S PAGE CARRIES NONE OF, when it says it leaves some out. Slack's All brought
 * the replies of at most ten threads per page until `#603 server-side-search`, and a server from
 * before it still does (`allPageAnswersFor`); a root among the rest is on the
 * page with its count and nothing of its thread, and setting its gap from that made the whole count
 * old — an unread answer the store held, or a new one only Main's count knew of, stopped counting
 * after a look at All. So such a page only lowers a gap it knows, as Main's does, unless the root's
 * count is nothing. A STEP BACK that brings a root the page has no gap for reads its shortfall as
 * old, as Main's step back does: the thread is older than All's newest page, and on Slack its replies
 * may not be on the step either.
 *
 * Main's page carries counts but no replies, so it sets no gap it can lower, with one exception: a
 * thread it is the first to say anything of. That one's gap is nothing when the
 * thread can have no old replies missing — its root was held with no replies, as Slack sends a
 * message before its first, or All reaches back to where it starts, so a reply posted since All's
 * read is new — and otherwise its whole shortfall, read as old: one posted between All's last read
 * and this one, to a thread older than All reaches, is not counted until that thread or All is read.
 * Without any read of All there is no gap at all, and every reply missing counts. NOR, by any read
 * that does not bring its replies, when All's newest page carried the root and passed over its
 * thread (`passedOver`): All reaching the root says nothing of replies that page did not bring, and
 * neither does All falling short of it, nor a step back bringing the root alone, so the gap stays
 * unknown — counted whole, as new, and settled by Done on the root (`settleUnknownReplies`) as well
 * as by a read that brings them.
 *
 * Every root's count, read now, is also the thread's count from here on: the summary the store holds
 * is put to it (`exactReplyCount` takes the higher of the two, so a summary from a thread read before
 * a reply was deleted kept saying "1 new" for a reply that is gone). And a read of Main may be followed
 * by one of All, behind it: `readAllForCounts`, called last, after a newest read of Main.
 */
export function noteReplyGapsIn(
  canon: ChannelCanon,
  payload: GapPage,
  view: string,
  older: boolean,
  delta: boolean,
  before: Map<string, Message>,
  readAllForCounts: () => void,
): void {
  if (view === "threads" || canon.scope) return;
  const gaps = canon.replyGaps;
  // What Done settled (`settledByDone`): a gap a read SETS is that read's from then on, and putting
  // the root back leaves it; one a read only LOWERS is still Done's, lowered — without Done's gap the
  // read would have lowered nothing — and putting the root back takes it back all the same.
  const place = (id: string, gap: number) => {
    gaps.set(id, gap);
    canon.settledByDone.delete(id);
  };
  const lower = (id: string, gap: number) => {
    if (canon.settledByDone.has(id)) canon.settledByDone.set(id, gap);
    gaps.set(id, gap);
  };
  const messages = payload.messages || [];
  if (older) {
    for (const message of messages) {
      const id = threadOf(message);
      if (!id || (message.thread as MessageThread).is_root === true || before.has(String(message.id)) || !gaps.has(id)) continue;
      lower(id, Math.max(0, (gaps.get(id) as number) - 1));
    }
  }
  const held = new Map<string, number>();
  for (const message of canon.messages) {
    const id = threadOf(message);
    if (id && (message.thread as MessageThread).is_root !== true) held.set(id, (held.get(id) || 0) + 1);
  }
  const shortfall = (id: string, count: number) => Math.max(0, count - (held.get(id) || 0));
  if (view === "thread") {
    const count = exactCount(payload.thread);
    if (!older && count !== null) place(String((payload.thread as ThreadSummary).id), shortfall(String((payload.thread as ThreadSummary).id), count));
    return;
  }
  const allRead = canon.views.has("flat");
  /** The threads this page carries a reply of. */
  const carried = new Set<string>();
  for (const message of messages) {
    const id = threadOf(message);
    if (id && (message.thread as MessageThread).is_root !== true) carried.add(id);
  }
  const summaries = new Map(canon.threads.map((summary) => [String(summary.id), summary]));
  for (const message of messages) {
    if (!threadOf(message) || (message.thread as MessageThread).is_root !== true) continue;
    const thread = message.thread as MessageThread;
    const id = String(thread.id);
    const count = exactCount(thread);
    if (count === null) continue;
    const summary = summaries.get(id);
    if (summary && exactCount(summary) !== count) {
      upsertThreadSummary(canon, { ...summary, reply_count: count, reply_count_exact: true });
    }
    const short = shortfall(id, count);
    const allNewest = view === "flat" && !older && !delta;
    if (allNewest && (carried.has(id) || count === 0 || !payload.notice)) {
      place(id, short);
      canon.passedOver.delete(id);
      continue;
    }
    // All's newest page carried the root and nothing of its thread, saying it leaves some out: kept
    // whether its gap is known or not, for the moment it is not (a root put back, `unsettleReplies`).
    if (allNewest) canon.passedOver.add(id);
    if (gaps.has(id)) {
      if (short < (gaps.get(id) as number)) lower(id, short);
      continue;
    }
    const prior = before.get(String(message.id));
    const hadNone = prior !== undefined && (!threadOf(prior) || exactCount(prior.thread) === 0);
    if (hadNone) place(id, 0);
    // Passed over: no read that does not bring its replies says how old they are — not Main's, even
    // once a cut copy leaves All's cover short of the root, nor a step back in All that brings the
    // root alone. Its unknown replies count as new until a read brings them, or Done on the root
    // settles them.
    else if (canon.passedOver.has(id) && !carried.has(id)) continue;
    else if (view === "flat" && older) place(id, short);
    else if (allReachesThread(canon, id, null)) place(id, 0);
    else if (allRead) place(id, short);
  }
  if (view === "main" && !older) readAllForCounts();
}

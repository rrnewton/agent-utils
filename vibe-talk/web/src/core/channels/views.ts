// What one channel's store says about each view: how far back a view is covered, and the rows it
// projects (`#624 voice-page-modularity`, moved from the legacy script; plan step 10).
//
// A PROJECTION NEVER INVENTS COVERAGE. Each view remembers how far back its OWN pages reached, and
// shows the store only from there up: Main's newest page does not prove anything about the thread
// replies between its rows, so All is not drawn from it. A view no page has covered is read.

import type { Message, ThreadSummary } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { inViewOf, threadOf, timeOf } from "./canon.ts";

/**
 * Record what one page proved about view `view`'s coverage, under view key `key`, in `canon`.
 * `lower` of two floors: null is "unknown".
 */
export function coverViewOf(
  canon: ChannelCanon,
  key: string,
  view: string,
  arriving: readonly unknown[],
  older: boolean,
  hasMore: boolean,
  notice: string,
): void {
  const timeField = view === "threads" ? "updated_at" : "timestamp";
  const prior = canon.views.get(key);
  const parsed = arriving.length ? timeOf(arriving[0], timeField) : NaN;
  const edge = Number.isFinite(parsed) ? parsed : null;
  const lower = (a: number | null, b: number | null) => (a === null || b === null ? null : Math.min(a, b));
  let more = hasMore;
  let floor = hasMore ? edge : null;
  // `#578 search-status-clarity`. Where the stretch with no gaps begins when it is not the floor
  // (`historyReach`), and the newest message any read of the view carried, which says whether the next
  // newest read joins what is held. All's is its newest top-level message: its pages bring the replies
  // of a few threads, which say nothing of the top-level messages between.
  const counted = arriving.filter((item) => view !== "flat" || !threadOf(item) ||
    (item as { thread: { is_root?: unknown } }).thread.is_root === true ||
    threadOf(item) === canon.scope).map((item) => timeOf(item, timeField)).filter(Number.isFinite);
  const pageTop = counted.length ? Math.max(...counted) : null;
  let top = prior && Number.isFinite(prior.top) ? prior.top : null;
  let dense = prior && Number.isFinite(prior.dense) ? prior.dense : null;
  if (hasMore && prior && !older) {
    // A refresh that stops short keeps the history already walked back to, as the rows do.
    more = prior.more;
    floor = prior.more ? lower(prior.floor, edge) : null;
    // ...but joins it only where it reaches down to the newest message a read before it carried. One
    // that does not — the page was away while more than a page arrived — leaves messages between that
    // no read brought, and what is below them is scattered.
    if (edge !== null && !(top !== null && edge <= top)) dense = edge;
  } else if (hasMore && prior && older) {
    // A step back that brought nothing — a page served from the cache whose walk found nothing older
    // yet — moves nothing: the floor is where the rows are, not "unknown".
    floor = edge === null ? prior.floor : lower(prior.floor, edge);
  } else if (!hasMore && !older) {
    dense = null;
  }
  if (pageTop !== null && (top === null || (!older && pageTop > top))) top = pageTop;
  canon.views.set(key, {
    floor, more, notice, top, dense,
    at: older && prior ? prior.at : Date.now(),
    live: true,
    cursor: prior ? prior.cursor : null,
    // `#203 incremental-refresh`. The forward cursor of the last newest read or delta folded for
    // this view, and when the last newest read landed. In memory only: a cursor saved with the
    // rows would usually have expired by the time a cold start could use it.
    newest: prior ? prior.newest || null : null,
    fullAt: prior ? prior.fullAt || 0 : 0,
    // `#308 live-reply-in-main`. And where that read was sent in the order of reads (`sentNumberOf`),
    // which says whether another newest page of the view is older or newer than it. In memory only,
    // as the order is.
    fullSent: prior ? prior.fullSent || 0 : 0,
  });
}

/**
 * View `view`'s rows (of thread `thread`, under view key `key`) from `canon`, or null when no page
 * has covered it.
 */
export function projectionOf(canon: ChannelCanon, key: string, view: string, thread: string | null): (Message | ThreadSummary)[] | null {
  const cover = canon.views.get(key);
  if (!cover) return null;
  const threads = view === "threads";
  const timeField = threads ? "updated_at" : "timestamp";
  return (canon[threads ? "threads" : "messages"] as (Message | ThreadSummary)[]).filter((item) =>
    (threads || inViewOf(item as Message, view, thread, canon.scope)) &&
    !(cover.more && cover.floor !== null && timeOf(item, timeField) < cover.floor));
}

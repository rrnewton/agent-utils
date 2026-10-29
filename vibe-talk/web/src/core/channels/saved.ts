// One channel's store as this device keeps it between visits, and back (`#18 offline-message-cache`;
// `#624 voice-page-modularity`, moved from the legacy script; plan step 10). What reads and writes
// the device's cache, and what else a load restores (the bookmark, thread colours), stay with the
// page; this is only the store's side of it.

import type { Message, ThreadSummary } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { threadOf } from "./canon.ts";

/** What is kept of one view's cover: not its cursors, which have expired by the time they are read. */
export interface SavedCover {
  floor: number | null;
  more: boolean;
  at: number;
  notice: string;
  top: number | null;
  dense: number | null;
}

/** A channel's store as saved in timeline mode. */
export interface SavedTimeline {
  mode: "timeline";
  usedAt: number;
  messages: Message[];
  threads: ThreadSummary[];
  views: Record<string, SavedCover>;
  hasThreads: boolean;
  scope: string | null;
  dismissed: string[];
  unplaced?: string[];
  removed?: string[];
  replyGaps?: Record<string, number>;
  passedOver?: string[];
  settledByDone?: Record<string, number>;
  bookmark?: unknown;
}

/** `canon` as this device saves it, used at `usedAt`, with the bookmark `bookmark` beside it. */
export function savedTimelineOf(canon: ChannelCanon, usedAt: number, bookmark: unknown): SavedTimeline {
  const views: Record<string, SavedCover> = {};
  for (const [view, cover] of canon.views) {
    views[view] = { floor: cover.floor, more: cover.more, at: cover.at, notice: cover.notice,
      top: Number.isFinite(cover.top) ? cover.top : null, dense: Number.isFinite(cover.dense) ? cover.dense : null };
  }
  const held = new Set(canon.messages.map((message) => String(message.id)));
  const heldThreads = new Set(canon.messages.map(threadOf).filter((id) => id !== null));
  return {
    mode: "timeline",
    usedAt,
    messages: canon.messages.slice(),
    threads: canon.threads.slice(),
    views,
    hasThreads: canon.hasThreads,
    scope: canon.scope,
    dismissed: [...canon.dismissed].filter((id) => held.has(id)),
    unplaced: [...canon.unplaced].filter((id) => held.has(id)),
    // `#315 removed-reply-target`. What it still holds of what was removed upstream: the read that
    // takes it out may never have landed, and a reply to it is still refused after a reload.
    removed: [...canon.removed.keys()].filter((id) => held.has(id)),
    // Only for threads the store still holds something of: the rest have no chip to draw.
    replyGaps: Object.fromEntries([...canon.replyGaps].filter(([id]) => heldThreads.has(id))),
    passedOver: [...canon.passedOver].filter((id) => heldThreads.has(id)),
    settledByDone: Object.fromEntries([...canon.settledByDone].filter(([id]) => heldThreads.has(id))),
    bookmark,
  };
}

/** Fill `canon` (a fresh, empty store) from what this device saved of its channel, `entry`. */
export function restoreCanon(canon: ChannelCanon, entry: SavedTimeline): void {
  canon.messages = entry.messages;
  canon.threads = entry.threads;
  canon.hasThreads = entry.hasThreads;
  canon.scope = entry.scope || null;
  canon.dismissed = new Set(entry.dismissed.map(String));
  // A reload does not place a message either: what the stream left unplaced stays so until read.
  canon.unplaced = new Set((entry.unplaced || []).map(String));
  // ...nor bring back a message removed upstream: every read from here on is sent after its removal.
  canon.removed = new Map((entry.removed || []).map((id) => [String(id), 0]));
  // `#222 unread-replies`. A copy saved before passed-over threads and Done's settling were both
  // kept may hold a wrong gap for a thread All's page passed over: nothing, which made its unknown
  // replies "N new" for good, past Done; or its whole count, set by Main's read after the copy was
  // cut, which hid an unread answer. Its gaps are not trusted, and All's cover goes with them, so
  // All is read behind Main once more and places them afresh (`readAllForCounts`).
  const trusted = entry.passedOver !== undefined && entry.settledByDone !== undefined;
  canon.replyGaps = trusted ? new Map(Object.entries(entry.replyGaps || {})) : new Map();
  canon.passedOver = new Set((entry.passedOver || []).map(String));
  canon.settledByDone = trusted ? new Map(Object.entries(entry.settledByDone || {})) : new Map();
  for (const [key, cover] of Object.entries(entry.views)) {
    if (!trusted && key === "flat") continue;
    canon.views.set(key, { ...cover, live: false, cursor: null });
  }
}

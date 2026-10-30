// Which read a refresh makes, and how fresh what came back is (`#203 incremental-refresh`, `#611
// timeline-read-failures`; `#624 voice-page-modularity`, moved from the legacy script; plan step 14).
// Why a refresh reads a delta at all, and when it must read in full, is told where the legacy script
// reads (`forwardCursor`, "WHICH READ").

/**
 * How old a view's last newest read may be before a refresh reads the newest page again: fifteen
 * minutes. A bridge with a change record reconciles its own snapshot and conveys what that finds in
 * its deltas, so this guards only against the page folding a delta wrongly; fifteen minutes is about
 * one reading session, so a session pays for at most one full read beyond the one it opened with.
 */
const TIMELINE_RECONCILE_MS = 900000;
/** How many delta pages one refresh follows before it reads the newest page instead. */
const TIMELINE_DELTA_PAGES = 4;
/** How long the page must have been hidden for coming back to refresh the channel. */
const VISIBLE_REFRESH_MS = 15000;
/** How far behind a page's `as_of` must be before the pill says the service is slow. */
const STALE_AS_OF_MS = 120000;
/** Why a read may be a delta. Anything else — entering a view, a stream reset — reads in full. */
const DELTA_REASONS: readonly string[] = ["live", "visible", "poll", "pull", "replay", "mutation"];

/** A view's coverage, as far as choosing a delta needs it. */
export interface CursorCover {
  readonly live?: boolean;
  readonly newest?: string | null;
  readonly fullAt?: number;
}

/** What choosing a delta's cursor asks of the page, each when it is needed. */
export interface CursorFacts {
  /** The channel the store holds. */
  storeChannel(): string | null;
  /** The channel on screen. */
  selectedChannel(): string;
  /** The coverage of the view on screen, if the store has read it. */
  cover(): CursorCover | undefined;
  /** Whether the channel's deltas are complete: edits and deletions included. */
  complete(): boolean | null | undefined;
  /** Whether an edit or removal from the replay tail is waiting for a read to answer it. */
  mutationAwaits(): boolean;
  /** The wall clock, in milliseconds. */
  now(): number;
}

/**
 * The forward cursor a refresh for `reason` may read from, or null for a full read. Only a view this
 * page has read live, recently enough, and only where the reason can be served by what this channel's
 * deltas carry: an edit or deletion needs a complete one. A queued read standing for several triggers
 * carries all their reasons, and is a delta only if every one of them could be.
 */
export function deltaCursor(reason: string | readonly (string | undefined)[] | undefined, facts: CursorFacts): string | null {
  if (Array.isArray(reason)) {
    const cursors = (reason as readonly (string | undefined)[]).map((one) => deltaCursor(one, facts));
    return cursors.length > 0 && cursors.every((cursor) => cursor !== null) ? cursors[0] as string : null;
  }
  const single = reason as string | undefined;
  if (single === undefined || !DELTA_REASONS.includes(single)) return null;
  if (facts.storeChannel() !== facts.selectedChannel()) return null;
  const cover = facts.cover();
  if (!cover || !cover.live || !cover.newest) return null;
  if (facts.now() - (cover.fullAt || 0) >= TIMELINE_RECONCILE_MS) return null;
  const complete = facts.complete();
  if (single === "mutation") return complete === true ? cover.newest : null;
  if (single === "replay") return complete === true || !facts.mutationAwaits() ? cover.newest : null;
  if (complete === false && single !== "live" && single !== "visible") return null;
  return cover.newest;
}

/** Whether `key`, a replay's, is an edit or a removal. */
export const isMutationKey = (key: string): boolean => key.startsWith("message_update:") || key.startsWith("message_delete:");

/** The server can no longer continue from that cursor, or it was not issued for this view. */
export function isCursorRefusal(error: unknown): boolean {
  const refused = error as { readonly status?: unknown; readonly code?: unknown } | null | undefined;
  return Boolean(refused) && ((refused!.status === 410 && refused!.code === "cursor_expired") ||
    (refused!.status === 400 && refused!.code === "cursor_mismatch"));
}

/**
 * When the server says it could not bring this page up to date: the instant it is complete up to, as
 * a time, if that is old enough to say so — or null, which means now. A bridge serving a snapshot it
 * failed to refresh sends `as_of`; a page that took it for a fresh read would claim to be current when
 * it is not. `failed` is whether the server answered from its own copy (`refreshFailed`).
 */
export function staleAsOf(payload: unknown, failed: boolean, now: () => number): number | null {
  const held = payload as { readonly as_of?: unknown } | null | undefined;
  const asOf = held && typeof held.as_of === "string" ? Date.parse(held.as_of) : NaN;
  // `#611 timeline-read-failures`. A page the server answered from its own copy, the chat service
  // having failed or kept it waiting, is as old as that copy, however recent.
  if (failed) return Number.isFinite(asOf) ? Math.min(asOf, now()) : now();
  return Number.isFinite(asOf) && now() - asOf > STALE_AS_OF_MS ? asOf : null;
}

export { STALE_AS_OF_MS, TIMELINE_DELTA_PAGES, TIMELINE_RECONCILE_MS, VISIBLE_REFRESH_MS };

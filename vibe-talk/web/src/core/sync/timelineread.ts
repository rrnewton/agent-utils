// One timeline read on the wire, the path it is read by, and what its landing says
// (`#203 incremental-refresh`, `#222 unread-replies`, `#308 live-reply-in-main`, `#611
// timeline-read-failures`, `#617 cold-load-first-paint`, `remote-ops-audit` read reason tags;
// `#624 voice-page-modularity`, moved from the legacy script into core/sync; mu-readcore T1).
// Which read a refresh makes is freshness's (`deltaCursor`); this owner only binds the page's
// facts to it, sends the read remembering when and in what order it was sent, names the reason
// word it carries, and joins what landed. Folding, drawing, the poll and the freshness pill
// are the page's.

import type { Message, ThreadSummary, TimelineResponse } from "../../contract/index.ts";
import type { CancelSignal } from "../../ports/index.ts";
import { STALE_AS_OF_MS, deltaCursor, staleAsOf as freshnessStaleAsOf } from "./freshness.ts";
import type { CursorCover } from "./freshness.ts";

/**
 * When reads were sent, as far as sending one and joining deltas read it: the epoch then, and
 * when a landed page's read was sent. The ledger itself is the annotations package's
 * (`createReadLedger`); its commands are closures over its own maps and keep no `this`, so the
 * page hands the object itself over and a payload noted here is the payload the page's own
 * `sentEpochOf` reads back.
 */
export interface TimelineLedger {
  /** The epoch now. */
  epoch(): number;
  /** Read `payload` was sent at epoch `sent`. */
  noteSent(payload: object, sent: number): void;
  /** When `payload`'s read was sent, as the epoch then; now for a payload no read sent. */
  sentEpochOf(payload: object): number;
}

/**
 * Where reads stand in the order they were sent (`createReadOrder`), as far as sending one
 * reads it. As the ledger's, its commands keep no `this`: the page hands the object itself
 * over, so the number taken here is the number the page's `sentNumberOf` reads back.
 */
export interface TimelineOrder {
  /** How many timeline reads the page has sent, this one counted as it is sent (`decoded`). */
  asked(): number;
  /** `payload` answers the read sent as number `number` (`asked()` as it was sent). */
  noteSent(payload: object, number: number): void;
}

/** One timeline read on the wire, as the transport receives it: only its cancellation. */
export interface TimelineWireOptions {
  readonly signal?: CancelSignal;
}

/**
 * A read's options, as far as naming its reason reads them: `readAs`, where the page names the
 * word itself, else the first of `reason`. Any other option a read carries (`quiet`,
 * `keepPosition`, ...) is the read's own business and passes unread, as the index keeps it.
 */
export interface ReadReasonSource {
  readonly readAs?: unknown;
  readonly reason?: string | readonly (string | undefined)[] | undefined;
  readonly [option: string]: unknown;
}

/** What one refresh's landed pages answer for, for the replay ledger; see `landedAnswer`. */
export interface LandedAnswer {
  readonly messages: Message[];
  readonly threads: ThreadSummary[];
  /** Null for a landing that read in full; else whether every delta of it was complete. */
  readonly delta: { complete: boolean } | null;
}

/**
 * What a landed read says about freshness, before the page acts on it: as of when the rows are
 * known current (null for now), the instant the pill stands at, whether the server answered
 * from its own copy, and the pill's word. Acting on it — the column's `freshAt`, the stand-in
 * retries, bringing the poll forward, the pill itself, the pins' revision and the bookmark —
 * is the page's (`noteFreshRead`).
 */
export interface FreshDecision {
  readonly asOf: number | null;
  readonly freshAt: number;
  readonly failed: boolean;
  readonly freshness: "failed" | "fresh" | "slow";
}

/** What reading the timeline asks of the page, each asked when it is needed. */
export interface TimelineReaderDependencies {
  /**
   * One timeline page from the server, checked against the wire contract
   * (`apiDecoded("TimelineResponse", ...)`). It counts the read as sent as it is called,
   * before its answer is awaited (the server's `asking` hook), which is what lets
   * `readTimeline` take the read's number after calling it.
   */
  decoded(path: string, options?: TimelineWireOptions): Promise<TimelineResponse>;
  readonly ledger: TimelineLedger;
  readonly order: TimelineOrder;
  /** How many messages one page asks for (`DISCORD_PAGE_LIMIT`). */
  readonly pageLimit: number;
  /** The id of the channel chosen to read, as the path writes it (`selectedChannelId`). */
  channelId(): string;
  /** The view on screen (`selection().view`). */
  view(): string;
  /** The thread on screen, or null (`selection().threadId`). */
  threadId(): string | null;
  /** `path` with the thread on screen asked for, where there is one (`withThreadQuery`). */
  withThreadQuery(path: string, threadId: string | null): string;
  /** Whether the column's walk back for the search is under way. */
  searchWalkActive(): boolean;
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
  /** Whether the server answered `payload` from its own copy (`refreshFailed`). */
  refreshFailed(payload: unknown): boolean;
  /** The wall clock, in milliseconds. */
  now(): number;
}

/** The timeline's reads: sending them, naming them, and joining what landed. */
export interface TimelineReader {
  /**
   * Read one timeline page, remembering when it was sent (the ledger's epoch, for
   * `readSpeaksFor`), and in what order (the order's number, for `sentNumberOf`).
   */
  readTimeline(path: string, options?: TimelineWireOptions): Promise<TimelineResponse>;
  /** The path of a read of the view on screen. `reason` is the word it sends as `reason=`. */
  timelinePath(before?: string | null, after?: string | null, reason?: string | null): string;
  /**
   * The word a read of the channel sends the server as `reason=`: why it was made.
   * Attribution only: nothing on the server, and nothing here, decides anything by the word.
   * `readAs` names it where the refresh's `reason` would say the wrong thing or nothing;
   * otherwise the refresh's own `reason`, the first of several merged into one queued read.
   * Null when neither names one.
   */
  readReasonOf(options: ReadReasonSource | null | undefined): string | null;
  /** `path` with the `reason=` of `options`, when it names one. */
  withReadReason(path: string, options: ReadReasonSource | null | undefined): string;
  /** The word a step back sends: `search` while the column's walk for the search is under way. */
  olderReadReason(): "search" | "older";
  /**
   * The forward cursor this refresh may read from, or null for a full read. Only a view this
   * page has read live, recently enough, and only where the reason can be served by what this
   * channel's deltas carry: an edit or deletion needs a complete one. A queued read standing
   * for several triggers carries all their reasons, and is a delta only if every one of them
   * could be. The choice itself is freshness's (`deltaCursor`).
   */
  forwardCursor(reason: string | readonly (string | undefined)[] | undefined): string | null;
  /**
   * Several delta pages of one refresh, as the one page that draws them: the last page's own
   * fields, every page's rows in order, and, for `readSpeaksFor`, the oldest page's epoch.
   * `pages` is not empty: a refresh joins only pages that landed.
   */
  joinedDeltas(pages: readonly TimelineResponse[]): TimelineResponse;
  /** What one refresh's landed pages answer for, for `settleReplays`; null when none landed. */
  landedAnswer(pages: readonly TimelineResponse[]): LandedAnswer | null;
  /**
   * When the server says it could not bring this page up to date: the instant it is complete
   * up to, as a time, if that is old enough to say so — or null, which means now. A bridge
   * serving a snapshot it failed to refresh sends `as_of`; a page that took it for a fresh
   * read would claim to be current when it is not. The rule is freshness's (`staleAsOf`).
   */
  staleAsOf(payload: unknown): number | null;
  /**
   * A read landed: how fresh the rows are — now, or as of what the server could vouch for.
   * A page from the server's own copy says "failed" only when that copy is older than
   * `STALE_AS_OF_MS`; a younger one is as current as the pill says. Either way the next read
   * comes soon; spending that retry and telling the reader stay with the page.
   */
  freshDecision(payload: TimelineResponse): FreshDecision;
}

/** A reader over `deps`. It holds nothing of its own: every fact is asked as it is needed. */
export function createTimelineReader(deps: TimelineReaderDependencies): TimelineReader {
  function readReasonOf(options: ReadReasonSource | null | undefined): string | null {
    if (options && typeof options.readAs === "string") return options.readAs;
    const reason = options && options.reason;
    const first = Array.isArray(reason) ? (reason as readonly (string | undefined)[]).find((one) => typeof one === "string") : reason;
    return typeof first === "string" ? first : null;
  }

  return {
    async readTimeline(path, options) {
      const sent = deps.ledger.epoch();
      // Counted as it is sent, before the answer is awaited (`decoded`).
      const asked = deps.decoded(path, options);
      const number = deps.order.asked();
      const payload = await asked;
      deps.ledger.noteSent(payload, sent);
      deps.order.noteSent(payload, number);
      return payload;
    },
    timelinePath(before = null, after = null, reason = null) {
      let path = `/api/v1/channels/${encodeURIComponent(deps.channelId())}/timeline` +
        `?view=${deps.view()}&limit=${deps.pageLimit}`;
      path = deps.withThreadQuery(path, deps.threadId());
      if (before) path += `&before=${encodeURIComponent(before)}`;
      if (after) path += `&after=${encodeURIComponent(after)}`;
      if (reason) path += `&reason=${encodeURIComponent(reason)}`;
      return path;
    },
    readReasonOf,
    withReadReason(path, options) {
      const reason = readReasonOf(options);
      return reason ? `${path}${path.includes("?") ? "&" : "?"}reason=${encodeURIComponent(reason)}` : path;
    },
    olderReadReason() {
      return deps.searchWalkActive() ? "search" : "older";
    },
    forwardCursor(reason) {
      return deltaCursor(reason, {
        storeChannel: () => deps.storeChannel(),
        selectedChannel: () => deps.selectedChannel(),
        cover: () => deps.cover(),
        complete: () => deps.complete(),
        mutationAwaits: deps.mutationAwaits,
        now: () => deps.now(),
      });
    },
    joinedDeltas(pages) {
      const last = pages[pages.length - 1] as TimelineResponse;
      const joined: TimelineResponse = {
        ...last,
        messages: pages.flatMap((page) => page.messages || []),
        threads: pages.flatMap((page) => page.threads || []),
        dismissed: pages.flatMap((page) => page.dismissed || []),
      };
      // As old as its oldest page, for `readSpeaksFor`. `#222 unread-replies`.
      deps.ledger.noteSent(joined, Math.min(...pages.map(deps.ledger.sentEpochOf)));
      return joined;
    },
    landedAnswer(pages) {
      if (!pages.length) return null;
      const full = pages.some((page) => !page.delta);
      return {
        messages: pages.flatMap((page) => page.messages || []),
        threads: pages.flatMap((page) => page.threads || []),
        delta: full ? null : { complete: pages.every((page) => page.delta && page.delta.complete === true) },
      };
    },
    staleAsOf(payload) {
      return freshnessStaleAsOf(payload, deps.refreshFailed(payload), () => deps.now());
    },
    freshDecision(payload) {
      const asOf = freshnessStaleAsOf(payload, deps.refreshFailed(payload), () => deps.now());
      const freshAt = asOf === null ? deps.now() : asOf;
      const failed = deps.refreshFailed(payload);
      const old = deps.now() - freshAt > STALE_AS_OF_MS;
      return { asOf, freshAt, failed, freshness: failed ? (old ? "failed" : "fresh") : asOf === null ? "fresh" : "slow" };
    },
  };
}

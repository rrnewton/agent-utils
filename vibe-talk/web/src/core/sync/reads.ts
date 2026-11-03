// The channel's timeline reads on the wire (`#203 incremental-refresh`, `#220 view-switch-instant`, `#229
// desktop-two-column`; `#624 voice-page-modularity`, moved from the legacy script into core/sync): one read
// at a time across both columns, the view the one out is for, and the read queued behind it by the column it
// is for — one each, so a read of the thread on the right never stands in for a refresh of Main or the
// other way round. The one queued read stands for every trigger that asked meanwhile (`joinQueuedReads`).
// And whether the last live page said more exists above but carried no cursor to walk back on.
// Reading, and what each read draws, are the page's (`loadDiscord`, `finishDiscordLoad`).

import { joinQueuedReads } from "./streams.ts";
import type { QueuedReadOptions } from "./streams.ts";

/** The reads' commands and questions, over the page's columns. */
export interface ChannelReads<Column> {
  /** Whether a read is on the wire. */
  busy(): boolean;
  /**
   * The view a timeline read on the wire is for (the page's channel context), or null: a switch back to a
   * view whose read is still out lets that read land rather than queueing another.
   */
  readContext(): string | null;
  /** A read for view `context` (null where it is not a timeline read) goes on the wire, where none is: whether it may. */
  claim(context: string | null): boolean;
  /** Remember the newest read asked for in `column` while another is on the wire, joined with any queued there. */
  queue(column: Column, options: QueuedReadOptions | undefined): void;
  /** Whether a read is queued for `column`. */
  queued(column: Column): boolean;
  /** The options of the read queued for `column`, or undefined: what a probe of the joined read reads. */
  pending(column: Column): QueuedReadOptions | undefined;
  /** Forget the read queued for `column`, unread. */
  drop(column: Column): void;
  /**
   * The read on the wire has ended: none is from now on, and the read queued for the first of `columns`
   * that has one is taken, or null.
   */
  release(columns: readonly Column[]): { column: Column; options: QueuedReadOptions | undefined } | null;
  /**
   * Whether the last live page said more exists above but carried no cursor to walk back on — a page a server
   * before `#510 cache-serve-every-read` answered from its message cache (`#384 cache-serve`); a server now
   * mints a cursor of its own for such a page. The first step back then owes the view a read of its own
   * newest page first.
   */
  cursorOwed(): boolean;
  /** A live page has landed: whether it owes the walk a cursor. */
  oweCursor(owed: boolean): void;
  /** The cursor is owed no longer: read, or the view is another. */
  payCursor(): void;
}

/** No read out, none queued. */
export function createChannelReads<Column>(): ChannelReads<Column> {
  let discordFetchInFlight = false;
  let timelineReadContext: string | null = null;
  const discordQueuedLoads = new Map<Column, { options: QueuedReadOptions | undefined }>();
  let discordWalkCursorOwed = false;

  return {
    busy: () => discordFetchInFlight,
    readContext: () => timelineReadContext,
    claim(context) {
      if (discordFetchInFlight) return false;
      discordFetchInFlight = true;
      timelineReadContext = context;
      return true;
    },
    queue(column, options) {
      const queued = discordQueuedLoads.get(column);
      discordQueuedLoads.set(column, { options: queued ? joinQueuedReads(queued.options, options) : options });
    },
    queued: (column) => discordQueuedLoads.has(column),
    pending(column) {
      const queued = discordQueuedLoads.get(column);
      return queued ? queued.options : undefined;
    },
    drop(column) {
      discordQueuedLoads.delete(column);
    },
    release(columns) {
      discordFetchInFlight = false;
      timelineReadContext = null;
      const next = columns.find((one) => discordQueuedLoads.has(one));
      if (next === undefined) return null;
      const queued = discordQueuedLoads.get(next) as { options: QueuedReadOptions | undefined };
      discordQueuedLoads.delete(next);
      return { column: next, options: queued.options };
    },
    cursorOwed: () => discordWalkCursorOwed,
    oweCursor(owed) {
      discordWalkCursorOwed = owed;
    },
    payCursor() {
      discordWalkCursorOwed = false;
    },
  };
}

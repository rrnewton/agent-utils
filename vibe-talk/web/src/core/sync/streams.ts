// The live streams' generations and reconnect delay, and how a read asked for while another is out
// joins the one queued behind it (`#203 incremental-refresh`, `#220 view-switch-instant`, `#337
// voice-relay-silent`; `#624 voice-page-modularity`, moved from the legacy script; plan step 14).

/** How long a dropped stream waits before it attaches again. */
const LIVE_RETRY_MS = 5000;

/**
 * A stream's generation: bumped on every start and stop. The read loop and its retry both check it
 * before doing anything, which is what stops a stream belonging to the previous channel — or to a
 * signed-out session — from acting after the page has moved on.
 */
export interface StreamGeneration {
  current(): number;
  /** A start or a stop: every read loop of an earlier generation is stale now. */
  bump(): void;
}

/** Generation 0. */
export function createStreamGeneration(): StreamGeneration {
  let generation = 0;
  return {
    current: () => generation,
    bump() {
      generation += 1;
    },
  };
}

/** A read's options, as far as joining two queued reads reads them. */
export interface QueuedReadOptions {
  readonly reason?: string | readonly (string | undefined)[];
  readonly quiet?: boolean;
  readonly [option: string]: unknown;
}

/**
 * The read that stands for `queued` (already waiting) and `options` (asked for now): it carries EVERY
 * reason among them, so a stream reset's full read is not narrowed to a delta because a live message
 * arrived behind it; the newest request's other options win, except `quiet`, kept from either, so a
 * read behind rows a view switch drew stays one without the loading line over them when a poll queues
 * behind it.
 */
export function joinQueuedReads(queued: QueuedReadOptions | undefined, options: QueuedReadOptions | undefined): QueuedReadOptions {
  const reasons = (opts: QueuedReadOptions | undefined): readonly (string | undefined)[] =>
    (opts && Array.isArray(opts.reason) ? opts.reason as readonly (string | undefined)[] : [opts ? opts.reason as string | undefined : undefined]);
  const all = [...new Set([...reasons(queued), ...reasons(options)])];
  const quiet = Boolean(queued && queued.quiet) || Boolean(options && options.quiet);
  return { ...(options || {}), reason: all.length === 1 ? all[0] : all, ...(quiet ? { quiet } : {}) };
}

export { LIVE_RETRY_MS };

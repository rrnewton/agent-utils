// Following one channel's live stream (`#44 live-push`, `#189 restore-ui-state`, `#226
// page-energy-profile`, `#337 voice-relay-silent`; `#624 voice-page-modularity`, moved from the legacy
// script into core/sync). The one owner of the stream the page follows: which channel, its generation,
// the last event id it saw, whether the response body is open and whether that attach resumed from a
// cursor, the reads waiting for it to attach, and the loop that reads it and retries. What a frame
// means, what Settings says about live updates, and the request itself are the page's. Why the stream is
// read with `fetch` and not `EventSource`, and what the server does with `Last-Event-ID`, is told where
// the legacy script opens the section.

import type { ClockPort } from "../../ports/index.ts";
import { readEventStream } from "./events.ts";
import type { ByteReader, EventFrame, TextChunkDecoder } from "./events.ts";
import { LIVE_RETRY_MS, createStreamGeneration } from "./streams.ts";

/**
 * The longest a read waits for the stream to attach (`whenAttached`). The server answers a
 * stream as soon as it has subscribed it, so on an ordinary connection this is one round trip; a
 * proxy that holds a streaming response back costs the reader this much of saved rows, no more.
 */
const LIVE_ATTACH_WAIT_MS = 3000;

/** The stream's response, as far as following it reads it. */
export interface StreamResponse {
  readonly ok: boolean;
  readonly status: number;
  readonly body: { getReader?(): ByteReader } | null;
}

/** What following a stream asks of the page. */
export interface ChannelFollowDependencies {
  readonly clock: ClockPort;
  /** The saved credential, or "" with none. */
  token(): string;
  /** How the server delivers live updates: `off`, `poll`, or `push`. */
  delivery(): string;
  /** Ask the server for the stream at `path` with `headers`. */
  open(path: string, headers: Record<string, string>): Promise<StreamResponse>;
  /** A decoder for the stream's bytes (a `TextDecoder`). */
  decoder(): TextChunkDecoder;
  /** One frame of the stream, after its id was taken down; may answer `reset`. */
  frame(frame: EventFrame): string | void;
  /** The response body opened (the replay ledger notes the attach, Settings redraws). */
  attached(): void;
  /** Whether the stream is attached changed, or the server has it off (Settings redraws). */
  changed(): void;
  /** The stream dropped, for the connection details: `why`. */
  dropped(why: string): void;
  /** Run the read loop, its failures caught quietly (`guardQuietly`). */
  guard(run: () => Promise<void>): void;
}

/** The followed stream's commands and questions. */
export interface ChannelFollow {
  /**
   * Follow `channelId`, replacing any stream already running — from event `resumeFrom` on, for a
   * stream a hidden page let go (`#226 page-energy-profile`), and otherwise from the server's tail.
   */
  start(channelId: string | null, resumeFrom?: string | null): void;
  /** Stop following whatever is being followed. Idempotent. */
  stop(): void;
  /**
   * Run `then` once the stream being followed has attached — or failed, or ended, or not answered in
   * `LIVE_ATTACH_WAIT_MS` — and at once when it already has, or nothing is being followed.
   */
  whenAttached(then: () => void): void;
  /** The channel the stream is following, or null when nothing is attached. */
  channel(): string | null;
  /** The last `id:` this page saw, or null. */
  lastEventId(): string | null;
  /** Whether the response body is currently open. */
  attached(): boolean;
  /** Whether the attach now open resumed from a cursor. `#337 voice-relay-silent`. */
  resumed(): boolean;
  /** The stream's generation, bumped on every start and stop. */
  generation(): number;
}

/** Following nothing. */
export function createChannelFollow(deps: ChannelFollowDependencies): ChannelFollow {
  /** The channel the stream is following, or null when nothing is attached. */
  let liveChannel: string | null = null;
  /**
   * Bumped on every start and stop.
   *
   * The read loop and its retry both check it before doing anything, which is what stops a stream
   * belonging to the previous channel — or to a signed-out session — from appending a row after the
   * page has moved on. An AbortController would be the other way; this one needs nothing of the
   * browser that a `fetch` polyfill might not have.
   */
  const liveGeneration = createStreamGeneration();
  /** The last `id:` this page saw, sent back as `Last-Event-ID` so a reconnect does not duplicate. */
  let liveLastEventId: string | null = null;
  /** Whether the response body is currently open. */
  let liveAttached = false;
  /** Whether the channel stream's attach now open resumed from a cursor. `#337 voice-relay-silent`. */
  let liveAttachResumed = false;

  /** Reads waiting for the stream to attach, each to be run once. */
  const liveAttachWaiters = new Set<() => void>();

  function stopChannelStream(): void {
    liveGeneration.bump();
    liveChannel = null;
    liveAttached = false;
    liveLastEventId = null;
    deps.changed();
  }

  /** The stream attached, or will not for now: nothing waits on it any longer. */
  function releaseLiveAttachWaiters(): void {
    for (const waiter of [...liveAttachWaiters]) waiter();
  }

  /**
   * Read, and keep reading.
   *
   * A dropped stream RETRIES rather than dying quietly. That is the whole reason this is a loop: the
   * failure this page was written against is a connection that goes away and takes the interface's
   * honesty with it, leaving a list that looks current and is not.
   */
  async function followChannel(channelId: string, generation: number): Promise<void> {
    while (generation === liveGeneration.current()) {
      try {
        await readChannelStream(channelId, generation);
      } catch (error) {
        if (generation !== liveGeneration.current()) {
          return;
        }
        // Not `showError`: a live feed that dropped is not a reason to put a red panel over a call
        // in progress. It is recorded where connection facts belong.
        deps.dropped((error as Error).message);
      }
      if (generation !== liveGeneration.current()) {
        return;
      }
      liveAttached = false;
      deps.changed();
      // Refused, or gone: a read held for the attach is not held through the retry delay as well.
      releaseLiveAttachWaiters();
      const resumed = await liveRetryDelay(generation);
      if (!resumed) {
        return;
      }
    }
  }

  /** Wait out the reconnect delay. Answers false if the stream was stopped while waiting. */
  function liveRetryDelay(generation: number): Promise<boolean> {
    return new Promise((resolve) => {
      deps.clock.setTimeout(() => resolve(generation === liveGeneration.current()), LIVE_RETRY_MS);
    });
  }

  async function readChannelStream(channelId: string, generation: number): Promise<void> {
    const headers: Record<string, string> = { Authorization: `Bearer ${deps.token()}` };
    if (liveLastEventId) {
      // The one thing that makes a reconnect neither duplicate nor drop: the server replays what
      // came after this id out of its own bounded tail, and anything it cannot replay arrives as an
      // `event: reset` instead of as a silent gap.
      headers["Last-Event-ID"] = liveLastEventId;
    }
    const response = await deps.open(`/api/v1/channels/${encodeURIComponent(channelId)}/stream`, headers);
    if (!response.ok) {
      throw new Error(`the live stream was refused (HTTP ${response.status})`);
    }
    const body = response.body;
    if (!body || typeof body.getReader !== "function") {
      throw new Error("this browser cannot read a streaming response");
    }
    const reader = body.getReader();
    liveAttached = true;
    // `#337 voice-relay-silent`. What a resumed attach replays is what it missed, which a call may
    // not have heard: `relayToAgent` tells that apart from a fresh attach's history.
    liveAttachResumed = Boolean(headers["Last-Event-ID"]);
    // The server subscribed this stream before it answered, so its replay tail is everything a read
    // begun from here on will see.
    deps.attached();
    releaseLiveAttachWaiters();
    await readEventStream(reader, () => generation === liveGeneration.current(), onStreamFrame, deps.decoder());
  }

  /** Take down the frame's cursor, then hand it to the page. */
  function onStreamFrame(frame: EventFrame): string | void {
    if (frame.id) {
      liveLastEventId = frame.id;
    }
    if (frame.event === "reset") {
      // A reset says there IS a gap the stream cannot fill. The cursor is dropped with it — resuming
      // from an id on the far side of a gap would ask the server to replay what it has already said
      // it cannot. What the page does about the gap is its own (`onStreamFrame`).
      liveLastEventId = null;
    }
    return deps.frame(frame);
  }

  return {
    start(channelId, resumeFrom = null) {
      stopChannelStream();
      liveLastEventId = resumeFrom;
      if (!channelId || !deps.token()) {
        return;
      }
      if (deps.delivery() === "off") {
        // Nothing publishes, so this would be a held-open connection that can never deliver anything.
        // Not attaching is also what makes the OFF state observable: the page says the server is not
        // watching AND is not pretending to listen. Turning ingestion on is a server restart, and the
        // page learns about it the next time it reads `/api/v1/client-config`.
        deps.changed();
        return;
      }
      liveChannel = channelId;
      const generation = liveGeneration.current();
      deps.guard(() => followChannel(channelId, generation));
    },
    stop: stopChannelStream,
    whenAttached(then) {
      if (liveChannel === null || liveAttached) {
        then();
        return;
      }
      const waiter = (): void => {
        liveAttachWaiters.delete(waiter);
        deps.clock.clearTimeout(timer);
        then();
      };
      const timer = deps.clock.setTimeout(waiter, LIVE_ATTACH_WAIT_MS);
      liveAttachWaiters.add(waiter);
    },
    channel: () => liveChannel,
    lastEventId: () => liveLastEventId,
    attached: () => liveAttached,
    resumed: () => liveAttachResumed,
    generation: () => liveGeneration.current(),
  };
}

// What the page and the tests use of it.
export { LIVE_ATTACH_WAIT_MS };

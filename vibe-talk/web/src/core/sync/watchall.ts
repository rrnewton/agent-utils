// Following the stream of every channel while a call is open (`#337 voice-relay-silent`; `#624
// voice-page-modularity`, moved from the legacy script into core/sync). The one owner of that stream:
// its generation, the cursor of the last event it delivered, whether its response body is open and
// whether that attach resumed from a cursor, whether the server can stream every channel at all, the
// handle that lets the attach now open go, and the loop that reads it and retries. What a frame means,
// what the connection details say, and the request itself are the page's. Why a call follows it, and
// only a call, is told where the legacy script opens the section.

import type { ClockPort } from "../../ports/index.ts";
import { readEventStream } from "./events.ts";
import type { EventFrame, TextChunkDecoder } from "./events.ts";
import type { StreamResponse } from "./follow.ts";
import { LIVE_RETRY_MS, createStreamGeneration } from "./streams.ts";

/** What lets one attach's connection go at once (an `AbortController`). */
export interface Abortable {
  abort(): void;
}

/** What following the stream of every channel asks of the page. */
export interface WatchAllDependencies<A extends Abortable = Abortable> {
  readonly clock: ClockPort;
  /** The saved credential, or "" with none. */
  token(): string;
  /** How the server delivers live updates: `off`, `poll`, or `push`. */
  delivery(): string;
  /** A handle that lets the next attach go, or null where the browser has none. */
  abortable(): A | null;
  /** Ask the server for the stream at `path` with `headers`, to be let go through `abort` when there is one. */
  open(path: string, headers: Record<string, string>, abort: A | null): Promise<StreamResponse>;
  /** A decoder for the stream's bytes (a `TextDecoder`). */
  decoder(): TextChunkDecoder;
  /** One frame of the stream, after its cursor was taken down; may answer `reset`. */
  frame(frame: EventFrame): string | void;
  /** The stream dropped, for the connection details: `why`. */
  dropped(why: string): void;
  /** The server answered 404: it cannot stream every channel, and is not asked again. */
  unsupported(): void;
  /** Run the read loop, its failures caught quietly (`guardQuietly`). */
  guard(run: () => Promise<void>): void;
}

/** The stream of every channel: its commands and questions. */
export interface WatchAll {
  /**
   * Follow every watched channel for the call that has just connected, replacing any stream already
   * running, from no cursor. Nothing is followed once the server said it cannot, while the server has
   * live delivery off, or without a credential.
   */
  start(): void;
  /** Stop following every channel, and let the attach now open go. Idempotent. */
  stop(): void;
  /** Whether its response body is open now. */
  attached(): boolean;
  /** Whether the attach now open resumed from a cursor: what it replays, the call missed. */
  resumed(): boolean;
}

/** Following nothing, on a server not yet known to lack the stream. */
export function createWatchAll<A extends Abortable>(deps: WatchAllDependencies<A>): WatchAll {
  /** Bumped on every start and stop, as the channel stream's is (`createChannelFollow`). */
  const watchAllGeneration = createStreamGeneration();
  /** The cursor of the last event it delivered, sent back as `Last-Event-ID` on a reconnect. */
  let watchAllLastEventId: string | null = null;
  /** Whether its response body is open now. */
  let watchAllAttached = false;
  /** Whether the attach now open resumed from a cursor: what it replays, the call missed. */
  let watchAllResumed = false;
  /** False once the server answered 404: it cannot stream every channel, and is not asked again. */
  let watchAllSupported = true;
  /**
   * Aborts the attach now open, where the browser can: a call that has ended lets its connection go at
   * once rather than at the stream's next keep-alive, so a phone on HTTP/1.1 does not spend one of
   * its few connections to this server on a stream nobody reads.
   */
  let watchAllAbort: A | null = null;

  function stopWatchAll(): void {
    watchAllGeneration.bump();
    if (watchAllAbort !== null) {
      try {
        watchAllAbort.abort();
      } catch (_error) {
        // Already finished: there is nothing left to let go.
      }
      watchAllAbort = null;
    }
    watchAllAttached = false;
    watchAllResumed = false;
    watchAllLastEventId = null;
  }

  /** Read the stream of every channel, and keep reading it, as `createChannelFollow` does one channel's. */
  async function followWatchAll(generation: number): Promise<void> {
    while (generation === watchAllGeneration.current()) {
      let ended = "dropped";
      try {
        ended = await readWatchAll(generation);
      } catch (error) {
        if (generation !== watchAllGeneration.current()) return;
        deps.dropped((error as Error).message);
      }
      if (generation !== watchAllGeneration.current() || ended === "unsupported") return;
      watchAllAttached = false;
      // A reset is answered at once, from now: the gap has been said, and waiting would widen it.
      if (ended === "reset") continue;
      const resumed = await new Promise<boolean>((resolve) => {
        deps.clock.setTimeout(() => resolve(generation === watchAllGeneration.current()), LIVE_RETRY_MS);
      });
      if (!resumed) return;
    }
  }

  /**
   * One attach of the stream of every channel, until it ends. Answers how: `unsupported` for a server
   * without it, `reset` when it said the gap cannot be filled, `ended` otherwise.
   */
  async function readWatchAll(generation: number): Promise<"unsupported" | "reset" | "ended"> {
    const headers: Record<string, string> = { Authorization: `Bearer ${deps.token()}` };
    if (watchAllLastEventId) headers["Last-Event-ID"] = watchAllLastEventId;
    const resumed = Boolean(watchAllLastEventId);
    const abort = deps.abortable();
    watchAllAbort = abort;
    const response = await deps.open("/api/v1/live/stream", headers, abort);
    if (generation !== watchAllGeneration.current()) return "ended";
    if (response.status === 404) {
      watchAllSupported = false;
      deps.unsupported();
      return "unsupported";
    }
    if (!response.ok) {
      throw new Error(`it was refused (HTTP ${response.status})`);
    }
    const body = response.body;
    if (!body || typeof body.getReader !== "function") {
      throw new Error("this browser cannot read a streaming response");
    }
    watchAllAttached = true;
    watchAllResumed = resumed;
    return readEventStream(body.getReader(), () => generation === watchAllGeneration.current(), onWatchAllFrame, deps.decoder());
  }

  /**
   * Take down the frame's cursor, the cursor over every channel, then hand it to the page. A reset
   * drops the cursor: resuming from an id on the far side of a gap would ask the server to replay what
   * it has already said it cannot.
   */
  function onWatchAllFrame(frame: EventFrame): string | void {
    if (frame.id) watchAllLastEventId = frame.id;
    if (frame.event === "reset") watchAllLastEventId = null;
    return deps.frame(frame);
  }

  return {
    start() {
      stopWatchAll();
      if (!watchAllSupported || deps.delivery() === "off" || !deps.token()) return;
      const generation = watchAllGeneration.current();
      deps.guard(() => followWatchAll(generation));
    },
    stop: stopWatchAll,
    attached: () => watchAllAttached,
    resumed: () => watchAllResumed,
  };
}

// The browser's implementation of the core's ports (`#624 voice-page-modularity`). Each reads the
// browser global it stands for at the moment it is used, never once at construction, so whatever
// the page's environment holds then is what is used.

import type { CancellationPort, ClockPort, HttpPort, HttpRequestInit, TimerHandle } from "../../ports/index.ts";

/** The ports the browser gives the core. */
export interface WebPorts {
  readonly http: HttpPort;
  readonly clock: ClockPort;
  readonly cancellation: CancellationPort;
}

/** The browser's ports: `fetch`, `setTimeout`/`clearTimeout`, and `AbortController` where there is one. */
export function createWebPorts(): WebPorts {
  return {
    http: { fetch: (path: string, init: HttpRequestInit) => fetch(path, init as RequestInit) },
    clock: {
      setTimeout: (run: () => void, ms: number): TimerHandle => setTimeout(run, ms),
      clearTimeout: (handle: TimerHandle): void => clearTimeout(handle as ReturnType<typeof setTimeout>),
    },
    cancellation: { create: () => (typeof AbortController === "function" ? new AbortController() : null) },
  };
}

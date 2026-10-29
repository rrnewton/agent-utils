// The browser's implementation of the core's ports (`#624 voice-page-modularity`). Each reads the
// browser global it stands for at the moment it is used, never once at construction, so whatever
// the page's environment holds then is what is used.

import type { CancellationPort, ClockPort, HttpPort, HttpRequestInit, StoragePort, TimerHandle } from "../../ports/index.ts";

/** The ports the browser gives the core. */
export interface WebPorts {
  readonly http: HttpPort;
  readonly clock: ClockPort;
  readonly cancellation: CancellationPort;
  /** The device's storage, `localStorage`. */
  readonly deviceStorage: StoragePort;
  /** This window's storage, `sessionStorage`. */
  readonly windowStorage: StoragePort;
}

/**
 * The browser's ports: `fetch`, `setTimeout`/`clearTimeout`, `AbortController` where there is one,
 * and `localStorage` and `sessionStorage`, whose very lookup throws where storage is refused.
 */
export function createWebPorts(): WebPorts {
  return {
    http: { fetch: (path: string, init: HttpRequestInit) => fetch(path, init as RequestInit) },
    clock: {
      setTimeout: (run: () => void, ms: number): TimerHandle => setTimeout(run, ms),
      clearTimeout: (handle: TimerHandle): void => clearTimeout(handle as ReturnType<typeof setTimeout>),
    },
    cancellation: { create: () => (typeof AbortController === "function" ? new AbortController() : null) },
    deviceStorage: {
      getItem: (key: string) => localStorage.getItem(key),
      setItem: (key: string, value: string) => localStorage.setItem(key, value),
      removeItem: (key: string) => localStorage.removeItem(key),
    },
    windowStorage: {
      getItem: (key: string) => sessionStorage.getItem(key),
      setItem: (key: string, value: string) => sessionStorage.setItem(key, value),
      removeItem: (key: string) => sessionStorage.removeItem(key),
    },
  };
}

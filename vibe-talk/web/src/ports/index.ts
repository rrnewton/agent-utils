// The capabilities the page's core asks of whatever runs it (`#624 voice-page-modularity`): each
// described by what it does, never by a browser class, so a native client can supply its own. Only
// the composition of the page (today the legacy script) constructs them, from web/src/platform/web.

/** What a cancellation hands a request: opaque to the core, read by the transport that honours it. */
export interface CancelSignal {
  readonly aborted: boolean;
}

/** One request a caller may give up on. */
export interface Cancellation {
  readonly signal: CancelSignal;
  abort(): void;
}

/** Makes cancellations; `null` where the platform cannot cancel a request. */
export interface CancellationPort {
  create(): Cancellation | null;
}

/** One HTTP request to the server, as the transport receives it. */
export interface HttpRequestInit {
  method: string;
  headers: Record<string, string>;
  body?: string;
  signal?: CancelSignal;
}

/** The server's answer, as far as the core reads it: its status, and its body as text. */
export interface HttpResponse {
  readonly status: number;
  readonly ok: boolean;
  text(): Promise<string>;
}

/** Sends a request to the server. A failure to reach it at all rejects. */
export interface HttpPort {
  fetch(path: string, init: HttpRequestInit): Promise<HttpResponse>;
}

/** A timer, opaque to the core. */
export type TimerHandle = unknown;

/** Runs something after a delay, and forgets one asked for. */
export interface ClockPort {
  setTimeout(run: () => void, ms: number): TimerHandle;
  clearTimeout(handle: TimerHandle): void;
}

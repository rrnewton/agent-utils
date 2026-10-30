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

/**
 * A small string store that outlives the page: the device's (kept across visits) or one window's.
 * Any call may throw, where the platform refuses storage (private browsing, a full quota, storage
 * turned off), and a write may also be silently dropped; callers that must know read it back.
 */
export interface StoragePort {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/**
 * Opens a socket to a URL, of the platform's own kind (`Socket`): the voice call's connection to its
 * voice service. Opening may throw where the URL is refused.
 */
export interface SocketPort<Socket = unknown> {
  open(url: string): Socket;
}

/**
 * The device's audio, of the platform's own kinds: the audio context class it has (`ContextClass`),
 * if any, for the call's playback and a streamed read aloud; and a media element (`Element`) that
 * plays one URL, for a read aloud played whole and the silent keeper that holds the media keys.
 */
export interface AudioPort<ContextClass = unknown, Element = unknown> {
  contextClass(): ContextClass | undefined;
  element(url: string): Element;
}

/**
 * Opens the microphone with the platform's constraints (`Constraints`), as a stream of its own kind
 * (`Stream`). Rejects where the reader or the platform refuses it, or there is no microphone.
 */
export interface MicrophonePort<Constraints = unknown, Stream = unknown> {
  open(constraints: Constraints): Promise<Stream>;
}

// One request to the vibe-talk server, with the token, and one error taxonomy out of it
// (`#624 voice-page-modularity`, moved from the legacy script). Transport, timers and
// cancellation come in as ports; what the page knows and the core does not (the token, its
// message cache, the name of the chat service) comes in as dependencies.

import { decode, is } from "../../contract/index.ts";
import type { Contract } from "../../contract/index.ts";
import type {
  CancelSignal,
  CancellationPort,
  ClockPort,
  HttpPort,
  HttpRequestInit,
  HttpResponse,
  TimerHandle,
} from "../../ports/index.ts";

/**
 * What {@link ServerApi.request} throws: an ordinary Error, plus the facts a caller branches on.
 *
 * `timedOut` and `shownInPill` are the page's own, from `#195 send-resilience`: the page stopped
 * waiting for the answer, and the freshness pill already says the read failed. `gatewayStatus` is
 * the status of a failed answer that was not this server's JSON — a proxy's own 502 or 504 page —
 * kept apart from `status` so nothing words it as the chat service's failure.
 */
export type ApiRequestError = Error & {
  network?: boolean;
  refused?: boolean;
  status?: number;
  code?: string;
  detail?: string;
  timedOut?: boolean;
  shownInPill?: boolean;
  gatewayStatus?: number;
  body?: unknown;
};

/** What a caller may say about one request. */
export interface ApiRequestOptions {
  method?: string;
  body?: unknown;
  signal?: CancelSignal | undefined;
}

/** The ports a {@link ServerApi} runs on. */
export interface ApiPorts {
  readonly http: HttpPort;
  readonly clock: ClockPort;
  readonly cancellation: CancellationPort;
}

/** What the page knows that a request needs. Each is asked at the moment it is needed. */
export interface ApiDependencies {
  /** The bearer token to send. */
  token(): string;
  /** The server refused this token, or a read with it: the device's copy of what it read ends. */
  readRefused(): void;
  /** A decoded request of `type` is about to be sent. */
  asking(type: keyof Contract): void;
  /** Who did not answer, at the start of a sentence: the chat service the reader is looking at. */
  answerer(): string;
}

/** The server, as the page asks it. */
export interface ServerApi {
  request(path: string, options?: ApiRequestOptions): Promise<unknown>;
  decoded<K extends keyof Contract>(type: K, path: string, options?: ApiRequestOptions): Promise<Contract[K]>;
  within<T>(
    ms: number,
    request: (signal: CancelSignal | undefined) => Promise<T>,
    abandoned?: ((answer: Promise<T>) => void) | null,
  ): Promise<T>;
}

/** A {@link ServerApi} on `ports`, asking `deps` for what only the page knows. */
export function createApi(ports: ApiPorts, deps: ApiDependencies): ServerApi {
  /**
   * The server answers with a real taxonomy — 503 `elevenlabs_not_configured` names the exact
   * setting that is missing, 502 `elevenlabs_error` carries the vendor's status and message,
   * including things only the vendor knows, such as an API key that lacks the `convai_write`
   * permission. Flattening that to "could not start" would throw away the only sentence that says
   * what to fix, so all of it is passed through.
   */
  async function request(path: string, options?: ApiRequestOptions): Promise<unknown> {
    const headers: Record<string, string> = { Authorization: `Bearer ${deps.token()}` };
    const init: HttpRequestInit = { method: (options && options.method) || "GET", headers };
    if (options && options.signal) init.signal = options.signal;
    // Only when there IS one. A GET with a Content-Type and no body is a request that says it is
    // carrying JSON and is not, which some proxies treat as a malformed request rather than as a
    // harmless extra header.
    if (options && options.body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(options.body);
    }
    let response: HttpResponse;
    try {
      response = await ports.http.fetch(path, init);
    } catch (cause) {
      const action = init.method === "GET" ? "loading data" : "saving your change";
      const error: ApiRequestError = new Error(
        `Could not reach vibe-talk while ${action}. Your current screen is still available; retry when the connection recovers.`
      );
      error.cause = cause;
      error.network = true;
      throw error;
    }
    const text = await response.text();
    let payload: unknown = null;
    try {
      payload = text ? JSON.parse(text) : null;
    } catch (_error) {
      const error: ApiRequestError = new Error(`vibe-talk returned non-JSON (HTTP ${response.status})`);
      if (!response.ok) error.gatewayStatus = response.status;
      throw error;
    }
    if (!response.ok) {
      // The server's error body is part of the wire contract. Anything else on a failed response —
      // a proxy's page, a gateway's JSON — is not this server speaking, so it names no code of its
      // own and the status line says so rather than inventing one.
      const body = is("ApiErrorBody", payload) ? payload : null;
      const detail = body && body.detail ? body.detail : "(no detail)";
      const code = body && body.error ? body.error : "error";
      const error: ApiRequestError = new Error(`HTTP ${response.status} ${code}: ${detail}`);
      // Only these two mean "your token is wrong". Everything else means the server is unhappy for
      // a reason that signing in again will not fix, and bouncing the owner back to the sign-in
      // screen for those would be a lie about which thing is broken.
      error.refused = response.status === 401 || response.status === 403;
      // A token the server no longer accepts, or a read it refuses, ends this device's copy of
      // what that token read. A 403 on a WRITE is narrower — a read-only channel — and is not this.
      if (response.status === 401 || (response.status === 403 && init.method === "GET")) {
        deps.readRefused();
      }
      // THE MACHINE-READABLE HALF, kept rather than folded into the sentence. The server answers
      // with a taxonomy — `summarizer_not_configured` is a fact about the DEPLOYMENT, while
      // `summarizer_error` is a fact about one attempt — and a caller that has to tell those apart
      // would otherwise be matching substrings of prose the server is free to rewrite. The sentence
      // is for the reader; these two are for the code.
      error.status = response.status;
      error.code = code;
      // ...and the server's own sentence on its own, for the places that want prose rather than a
      // diagnosis. `error.message` is the right thing on a status line, where the code and the
      // status are what an operator acts on; it is the wrong thing in a standing sentence that a
      // reader is meant to understand.
      error.detail = body && body.detail ? body.detail : "";
      // ...and the whole answer, for the one refusal that carries the state it refused against: a
      // bookmark moved meanwhile (409), which says where it is now. `#240 reading-bookmark`.
      error.body = payload;
      throw error;
    }
    return payload;
  }

  /**
   * {@link request}, with the answer checked against the wire contract before anything reads it.
   *
   * The page and the server ship in one binary, so a body that does not match is a defect — or
   * something between them rewriting it — rather than version skew to tolerate. It fails the way
   * the request itself would have: one error, whose sentence names the type and the field that is
   * wrong, instead of a missing property surfacing as `undefined` somewhere far from the cause.
   */
  async function decoded<K extends keyof Contract>(
    type: K,
    path: string,
    options?: ApiRequestOptions,
  ): Promise<Contract[K]> {
    // `remote-ops-audit` W7. Every timeline read the page sends passes here, and is told to the
    // page as it is sent, so a read given up on can tell whether any was sent after it.
    deps.asking(type);
    const payload = await request(path, options);
    try {
      return decode(type, payload);
    } catch (cause) {
      const error: ApiRequestError = new Error(
        `vibe-talk returned a malformed answer — ${(cause as { message?: unknown }).message}. Reload the page; if this persists, ` +
          `the page and the server disagree about the wire format.`
      );
      error.cause = cause;
      throw error;
    }
  }

  /**
   * A request that stops waiting after `ms`, failing as a timeout the reader can be told about
   * rather than as a "refreshing…" nobody ends.
   *
   * `#195 send-resilience`. The server bounds its own read of a slow chat service, but at a figure
   * set for the service — fifty seconds, in the incident this was written for — not for a reader
   * looking at the screen. Giving up here does not cancel the server's read; it only stops this
   * page waiting on it.
   *
   * `abandoned`, when given, is handed the request still on the wire when the wait is given up, and
   * the request is then not aborted: its answer, if it comes, is the caller's to use or drop.
   * Without it the request is aborted, as it always was.
   */
  async function within<T>(
    ms: number,
    request: (signal: CancelSignal | undefined) => Promise<T>,
    abandoned: ((answer: Promise<T>) => void) | null = null,
  ): Promise<T> {
    const controller = !abandoned ? ports.cancellation.create() : null;
    let timer: TimerHandle | null = null;
    let gaveUp = false;
    const late = new Promise<never>((_resolve, reject) => {
      timer = ports.clock.setTimeout(() => {
        gaveUp = true;
        if (controller) controller.abort();
        const error: ApiRequestError = new Error(
          `${deps.answerer()} did not answer within ` +
            `${Math.round(ms / 1000)} seconds.`
        );
        error.timedOut = true;
        reject(error);
      }, ms);
    });
    const asked = request(controller ? controller.signal : undefined);
    try {
      return await Promise.race([asked, late]);
    } catch (error) {
      if (gaveUp && abandoned) abandoned(asked);
      throw error;
    } finally {
      if (timer !== null) ports.clock.clearTimeout(timer);
    }
  }

  return { request, decoded, within };
}

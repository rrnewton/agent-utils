// What held the main thread, for the log (`#2276 vibe-talk-client-debug-mode`; new in view/shell):
// the owner's client debug mode. The freeze watch (`freezewatch.ts`, `#2066`) says WHEN the page
// stalled and what it was showing; on the owner's phone its long tasks and heartbeat freezes
// carried no attribution at all (that browser gives no long-animation-frame entries). With the
// switch on, this records what code ran:
//
// - Named spans around the page's own hot paths (`span(name, fn)`, wrapped at the composition
//   root's call sites): each with its layout reads (`getBoundingClientRect`/`getClientRects`
//   counted while the mode is on) and, for one of 16 ms or more, the page's element count at its
//   end. A stall gets the spans that ran inside it, the ones still open at its start, the last
//   ones to end before it, and the milliseconds no span covered (where a span is missing).
// - Event Timing entries (`evt:<type>` from the event to its paint, `evt-run:<type>` its handlers)
//   and long-animation-frame script detail, where the browser delivers them.
// - Main-thread samples from the JS Self-Profiling API, where the browser allows it (the server
//   serves the shell with `Document-Policy: js-profiling` only while the switch's cookie is set),
//   summarised per stall to its top frames; the raw samples never leave the device.
//
// The page never reads the user agent (a ground rule); the server names the browser's family and
// major version from the request's own header when it logs what this sends.
//
// What it finds rides with the records the page already posts: an attribution on a freeze
// report's events, and a summary (capabilities, span totals, the slowest long frames) on a freeze
// report or a scroll trace. Besides those, at most one summary-only freeze report a minute, and
// only after something slow (a frame of 100 ms or a span of 50 ms). Timings and code names only:
// every string is the page's own code, URLs lose their query, fragment and content hash, element
// ids that are not the page's own are dropped, and any word that looks like an id is redacted by
// the same rule the server refuses with (`src/client_debug.rs`). Off is wholly off: no observer,
// no profiler, no wrapped method, no timer, no cookie; `span` is one test and the call.

/** The device key holding the switch: kept "1", forgotten when off. */
export const CLIENT_DEBUG_KEY = "vibe-talk.voice.client-debug";
/** The cookie that tells the server to serve the profiling document policy. */
export const CLIENT_DEBUG_COOKIE = "vibe-talk-client-debug";
/** The one-load URL flag: `?client-debug=1`. */
export const CLIENT_DEBUG_FLAG = "client-debug";
/** How many finished spans are kept for attribution. */
export const DEBUG_SPAN_RING = 512;
/** The longest code name sent, in characters: the server's cap. */
export const DEBUG_NAME_CHARS = 96;
/** The most spans, open spans and recent ends one stall names: the server's caps. */
export const DEBUG_EVENT_SPANS = 6;
export const DEBUG_OPEN_SPANS = 4;
export const DEBUG_RECENT_SPANS = 8;
/** The most scripts one long frame names, and frames each profile list names: the server's caps. */
export const DEBUG_FRAME_SCRIPTS = 4;
export const DEBUG_PROFILE_FRAMES = 6;
/** The most span names and long frames one summary carries: the server's caps. */
export const DEBUG_SUMMARY_SPANS = 16;
export const DEBUG_SUMMARY_FRAMES = 20;
/** The most events of one freeze report that carry detail: the server's cap. */
export const DEBUG_DETAILED_EVENTS = 10;
/** The least while between two summary-only reports, in milliseconds. */
export const DEBUG_SUMMARY_EVERY_MS = 60000;
/** A frame or a span at or over these makes the period worth a summary, in milliseconds. */
export const DEBUG_SLOW_FRAME_MS = 100;
export const DEBUG_SLOW_SPAN_MS = 50;
/** A span at or over this records the page's element count at its end, in milliseconds. */
export const DEBUG_NODES_SPAN_MS = 16;
/**
 * The profiler's sampling interval, in milliseconds. Chosen from the measured CPU overhead with
 * the mode on (`#2276` evidence, renderer CPU seconds on the phone layout's scripted walk): 10 ms
 * cost +7.5%, 20 ms +3.1%, 30 ms nothing measurable; 20 ms keeps it under 5%.
 */
export const DEBUG_SAMPLE_MS = 20;
/** The profiler's buffer, in samples, and how often a quiet profiler is restarted, in ms. */
export const DEBUG_PROFILE_BUFFER = 6000;
export const DEBUG_PROFILE_ROLL_MS = 30000;
/**
 * The shortest Event Timing entry asked for, in milliseconds: an event that took this long to its
 * paint is lag a reader feels; shorter ones (the browser's floor is 16) are pointer noise that
 * would crowd the page's own spans out of a summary.
 */
const EVENT_TIMING_MIN_MS = 48;
/** A profile roll older than this no longer holds a report back, in milliseconds. */
const PENDING_GIVE_UP_MS = 2000;

/** One finished span. `page` spans are the page's own; `evt` ones come from Event Timing. */
interface SpanRecord {
  readonly name: string;
  readonly start: number;
  readonly end: number;
  readonly reads: number;
  readonly nodes: number | null;
  readonly page: boolean;
  /** The channel's messages held (`S`) and the list's rows mounted (`D`) at its end, for a long one. */
  readonly s?: number | null;
  readonly d?: number | null;
  /** Characters of saved cache read or written inside it, and rows it dressed (`note`). */
  readonly chars?: number;
  readonly rows?: number;
}

/** A span still running, with what `note` counted inside it so far. */
interface OpenRecord {
  readonly name: string;
  readonly start: number;
  chars: number;
  rows: number;
}

/** What the browser offers the debug mode (the server's `Caps`). */
export interface ClientDebugCaps {
  loaf: boolean;
  longtask: boolean;
  profiler: boolean;
  policy: boolean;
  event: boolean;
  sample_ms: number;
}

/** A started profiler, as the page's port hands it over (`new Profiler(...)`). */
export interface ProfilerLike {
  readonly sampleInterval?: number;
  stop(): Promise<unknown>;
}

/** A timer, opaque here. */
type TimerHandle = unknown;

/** What the debug mode asks of the page. */
export interface ClientDebugDependencies {
  /** The spans' clock, in milliseconds (`performance.now`). */
  now(): number;
  /** Whether the device holds the switch on. */
  held(): boolean;
  /** Keep the switch's choice on the device. */
  keep?(on: boolean): void;
  /** The page's location query (`?client-debug=1` turns the mode on for one load). */
  query(): string;
  /** Whether the switch's cookie is set now, and set or clear it. */
  cookieSet(): boolean;
  setCookie(on: boolean): void;
  /** The performance entry types this browser delivers (`PerformanceObserver.supportedEntryTypes`). */
  supportedEntryTypes(): readonly string[];
  /** Watch performance entries of `type`; answers the unwatch, or null where there are none. */
  observe(type: string, note: (entries: readonly unknown[]) => void, options?: { durationThreshold?: number }): (() => void) | null;
  /** Whether this browser has the JS Self-Profiling API at all. */
  profilerExists?(): boolean;
  /** Start a profiler; throws or answers null where the page may not (no document policy). */
  startProfiler?(sampleMs: number, maxBufferSize: number): ProfilerLike | null;
  /** `Element.prototype`, whose two layout reads are counted while the mode is on. */
  elementPrototype?(): object | null;
  /** The page's element count now. */
  nodeCount?(): number;
  /**
   * The sizes a long span ran at (`#2282` section 9 item 7): the channel's messages held (`S`,
   * `canon.messages.length`) and the list's rows mounted (`D`, `list.children.length`).
   */
  sizes?(): { readonly s: number; readonly d: number };
  /** Add a span to the browser's performance timeline (`performance.measure`), and clear them. */
  measure?(name: string, start: number, end: number): void;
  clearMeasures?(): void;
  /** Runs something after a delay, and forgets one asked for (the page's clock port). */
  clock: {
    setTimeout(run: () => void, ms: number): TimerHandle;
    clearTimeout(handle: TimerHandle): void;
  };
  /** Post one freeze-report body (the summary-only report); true where the server took it. */
  send(report: string): Promise<boolean>;
  /** Whether a posting may go out now: online, in sight, a write token. */
  canPost(): boolean;
}

/** The debug mode's commands. */
export interface ClientDebug {
  /** Whether the mode is on. */
  enabled(): boolean;
  /** Read the switch (and the URL flag) and apply it. Again: re-reads. */
  start(): void;
  /** The switch: keep the choice, set the cookie, and turn the mode on or off at once. */
  setOn(on: boolean): void;
  /** Run `fn` as a span named `name` while the mode is on; just `fn()` while it is off. */
  span<T>(name: string, fn: () => T): T;
  /** `fn` wrapped as a span named `name`. */
  spanned<A extends unknown[], R>(name: string, fn: (...args: A) => R): (...args: A) => R;
  /** Count `n` cache characters or dressed rows against the innermost running span, while on. */
  note(counter: "chars" | "rows", n: number): void;
  /**
   * What ran in a stall over [`start`, `end`] (with its long-frame entry, where it is one), or
   * null while the mode is off. The profiler's summary comes later, through `late`.
   */
  attribute(start: number, end: number, entry: unknown, late: (profile: unknown) => void): unknown;
  /** The summary since the last posted one, or null while the mode is off. */
  summary(): unknown;
  /** A report carrying the summary was taken: the period starts over. */
  posted(): void;
  /** True while a stall's profile is still being summarised (a report waits a beat for it). */
  pending(): boolean;
  /** What the browser offers. */
  caps(): ClientDebugCaps;
}

/** The code-name alphabet's complement: what never rides. */
const NOT_CODE = /[^A-Za-z0-9_$.#:/<>()[\] -]/gu;

/** `text` without a trailing `:123` or `:123:45` (a script position), and that tail. */
function splitPosition(text: string): [string, string] {
  let head = text;
  for (let i = 0; i < 2; i += 1) {
    const at = head.lastIndexOf(":");
    if (at < 0) break;
    const tail = head.slice(at + 1);
    if (tail === "" || !/^[0-9]+$/u.test(tail)) break;
    head = head.slice(0, at);
  }
  return [head, text.slice(head.length)];
}

/**
 * Whether one word looks like an id: six or more digits in a row, sixteen or more hex digits in a
 * row, or an upper-case word of eight or more characters with a digit in it. The server's
 * `looks_like_id`, word for word.
 */
export function looksLikeId(word: string): boolean {
  let digits = 0;
  let hex = 0;
  for (const c of word) {
    digits = /[0-9]/u.test(c) ? digits + 1 : 0;
    hex = /[0-9a-fA-F]/u.test(c) ? hex + 1 : 0;
    if (digits >= 6 || hex >= 16) return true;
  }
  return word.length >= 8 && /^[A-Z0-9]+$/u.test(word) && /[0-9]/u.test(word);
}

/** `raw` as a code name the server takes: the alphabet only, capped, ids redacted to `_`. */
export function codeName(raw: unknown): string {
  if (typeof raw !== "string") return "";
  const text = Array.from(raw.replace(NOT_CODE, "_").trim()).slice(0, DEBUG_NAME_CHARS).join("");
  const [head, tail] = splitPosition(text);
  return head.replace(/[A-Za-z0-9]+/gu, (word) => (looksLikeId(word) ? "_" : word)) + tail;
}

/** A script URL as its file name alone, without query, fragment or content hash, with `position`. */
export function srcName(url: unknown, ...position: readonly number[]): string {
  const path = (typeof url === "string" ? url : "").split(/[?#]/u)[0] ?? "";
  const file = (path.split("/").pop() ?? "").replace(/\.[0-9a-f]{8,}(?=\.)/gu, "");
  const at = position.filter((n) => Number.isFinite(n) && n >= 0).map((n) => String(Math.round(n)));
  return codeName([file, ...at].join(":"));
}

/** A long frame's invoker, page ids only: a URL is its file name, a `[src=...]` goes. */
export function invokerName(raw: unknown): string {
  if (typeof raw !== "string") return "";
  const text = raw.replace(/\[src=[^\]]*\]/gu, "[src]");
  if (/^[a-z][a-z0-9+.-]*:\/\//iu.test(text) || text.startsWith("/")) return srcName(text);
  return codeName(text.replace(/#([^.\s[\]]+)/gu, (_whole, id: string) => (/^[a-z][a-z0-9-]{0,48}$/u.test(id) ? `#${id}` : "#_")));
}

function whole(ms: number): number {
  return Number.isFinite(ms) ? Math.max(0, Math.round(ms)) : 0;
}

function num(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

const INVOKER_TYPES: readonly string[] = [
  "user-callback", "event-listener", "resolve-promise", "reject-promise", "classic-script", "module-script",
];

interface LoafScript {
  readonly duration?: unknown;
  readonly invokerType?: unknown;
  readonly invoker?: unknown;
  readonly sourceURL?: unknown;
  readonly sourceFunctionName?: unknown;
  readonly sourceCharPosition?: unknown;
  readonly forcedStyleAndLayoutDuration?: unknown;
}

interface LoafEntry {
  readonly startTime?: unknown;
  readonly duration?: unknown;
  readonly blockingDuration?: unknown;
  readonly renderStart?: unknown;
  readonly styleAndLayoutStart?: unknown;
  readonly scripts?: readonly LoafScript[];
}

/** A long frame's scripts, longest first. */
function longestScripts(entry: LoafEntry): LoafScript[] {
  const scripts = Array.isArray(entry.scripts) ? [...entry.scripts] : [];
  return scripts.sort((a, b) => num(b?.duration) - num(a?.duration));
}

/** One profile's trace, as the JS Self-Profiling API hands it over. */
interface ProfileTrace {
  readonly resources?: readonly unknown[];
  readonly frames?: readonly { readonly name?: unknown; readonly resourceId?: unknown; readonly line?: unknown; readonly column?: unknown }[];
  readonly stacks?: readonly { readonly frameId?: unknown; readonly parentId?: unknown }[];
  readonly samples?: readonly { readonly timestamp?: unknown; readonly stackId?: unknown }[];
}

/** The top `limit` of `counts`, most first, as profile frames. */
function topFrames(counts: Map<number, number>, label: (frameId: number) => { fn: string; src: string }): Array<{ fn: string; src: string; n: number }> {
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0] - b[0])
    .slice(0, DEBUG_PROFILE_FRAMES)
    .map(([frameId, n]) => ({ ...label(frameId), n }));
}

/** A trace's samples in [`start`, `end`], summarised: top self and inclusive frames. */
export function summariseProfile(raw: unknown, start: number, end: number, intervalMs: number): unknown {
  const trace = (raw ?? {}) as ProfileTrace;
  const samples = Array.isArray(trace.samples) ? trace.samples : [];
  const stacks = Array.isArray(trace.stacks) ? trace.stacks : [];
  const frames = Array.isArray(trace.frames) ? trace.frames : [];
  const resources = Array.isArray(trace.resources) ? trace.resources : [];
  const selfCounts = new Map<number, number>();
  const totalCounts = new Map<number, number>();
  let inWindow = 0;
  let js = 0;
  const slack = Math.max(0, intervalMs);
  for (const sample of samples) {
    const at = num(sample?.timestamp);
    if (at < start - slack || at > end + slack) continue;
    inWindow += 1;
    if (typeof sample.stackId !== "number") continue;
    js += 1;
    const seen = new Set<number>();
    let stackId: unknown = sample.stackId;
    let first = true;
    for (let depth = 0; typeof stackId === "number" && depth < 512; depth += 1) {
      const stack = stacks[stackId];
      if (!stack || typeof stack.frameId !== "number") break;
      if (first) selfCounts.set(stack.frameId, (selfCounts.get(stack.frameId) ?? 0) + 1);
      first = false;
      if (!seen.has(stack.frameId)) {
        seen.add(stack.frameId);
        totalCounts.set(stack.frameId, (totalCounts.get(stack.frameId) ?? 0) + 1);
      }
      stackId = stack.parentId;
    }
  }
  const label = (frameId: number): { fn: string; src: string } => {
    const frame = frames[frameId];
    const resource = typeof frame?.resourceId === "number" ? resources[frame.resourceId] : undefined;
    const position = [num(frame?.line), num(frame?.column)].filter((n) => n > 0);
    return { fn: codeName(frame?.name), src: resource === undefined ? "" : srcName(resource, ...position) };
  };
  return {
    samples: inWindow,
    js_samples: js,
    interval_ms: whole(intervalMs),
    self_top: topFrames(selfCounts, label),
    total_top: topFrames(totalCounts, label),
  };
}

/** The debug mode over `deps`; nothing runs until `start` or `setOn`. */
export function createClientDebug(deps: ClientDebugDependencies): ClientDebug {
  const createdAt = deps.now();
  let on = false;
  let undo: Array<() => void> = [];
  const ring: Array<SpanRecord | undefined> = new Array<SpanRecord | undefined>(DEBUG_SPAN_RING);
  let ringAt = 0;
  const openStack: OpenRecord[] = [];
  let reads = 0;
  let measured = 0;
  let caps: ClientDebugCaps = { loaf: false, longtask: false, profiler: false, policy: false, event: false, sample_ms: 0 };
  /** The period's totals since the last posted summary. */
  let periodStart = createdAt;
  let totals = new Map<string, { n: number; total: number; max: number }>();
  let framesOver50 = 0;
  let framesOver100 = 0;
  let maxFrame = 0;
  let slowFrames: Array<{ t: number; dur_ms: number; blocking_ms: number; fn: string; src: string }> = [];
  let slow = false;
  let lastPostAt = -Infinity;
  let summaryTimer: TimerHandle | null = null;
  let summarySending = false;
  /** The profiler: the one running, a roll in flight, and the stalls waiting on it. */
  let profiler: ProfilerLike | null = null;
  let rollTimer: TimerHandle | null = null;
  let rollingSince: number | null = null;
  let waiting: Array<{ start: number; end: number; late: (profile: unknown) => void }> = [];

  function keepRecord(record: SpanRecord): void {
    ring[ringAt] = record;
    ringAt = (ringAt + 1) % DEBUG_SPAN_RING;
    const dur = record.end - record.start;
    const total = totals.get(record.name);
    if (total) {
      total.n += 1;
      total.total += dur;
      if (dur > total.max) total.max = dur;
    } else {
      totals.set(record.name, { n: 1, total: dur, max: dur });
    }
    if (record.page && dur >= DEBUG_SLOW_SPAN_MS) slow = true;
  }

  function endSpan(open: OpenRecord, spanReads: number): void {
    const { name, start } = open;
    const end = deps.now();
    const dur = end - start;
    let nodes: number | null = null;
    let sizes: { readonly s: number; readonly d: number } | null = null;
    if (dur >= DEBUG_NODES_SPAN_MS) {
      try {
        nodes = deps.nodeCount ? deps.nodeCount() : null;
      } catch (_error) {
        nodes = null;
      }
      try {
        sizes = deps.sizes ? deps.sizes() : null;
      } catch (_error) {
        sizes = null;
      }
    }
    keepRecord({
      name, start, end, reads: spanReads, nodes, page: true,
      s: sizes ? sizes.s : null, d: sizes ? sizes.d : null, chars: open.chars, rows: open.rows,
    });
    if (deps.measure && dur >= 1) {
      try {
        deps.measure(`vt:${name}`, start, end);
        measured += 1;
        if (measured >= 500 && deps.clearMeasures) {
          deps.clearMeasures();
          measured = 0;
        }
      } catch (_error) {
        // The timeline refused it: the ring has it all the same.
      }
    }
  }

  function span<T>(name: string, fn: () => T): T {
    if (!on) return fn();
    const open: OpenRecord = { name, start: deps.now(), chars: 0, rows: 0 };
    const reads0 = reads;
    openStack.push(open);
    try {
      return fn();
    } finally {
      openStack.pop();
      if (on) endSpan(open, reads - reads0);
    }
  }

  function spanned<A extends unknown[], R>(name: string, fn: (...args: A) => R): (...args: A) => R {
    return (...args: A): R => (on ? span(name, () => fn(...args)) : fn(...args));
  }

  function note(counter: "chars" | "rows", n: number): void {
    if (!on || !Number.isFinite(n) || n <= 0) return;
    const top = openStack[openStack.length - 1];
    if (top) top[counter] += Math.round(n);
  }

  function noteEvents(entries: readonly unknown[]): void {
    for (const one of entries) {
      const entry = one as { name?: unknown; startTime?: unknown; duration?: unknown; processingStart?: unknown; processingEnd?: unknown };
      const type = typeof entry.name === "string" && /^[a-z]{1,32}$/u.test(entry.name) ? entry.name : null;
      if (!type) continue;
      const start = num(entry.startTime);
      keepRecord({ name: `evt:${type}`, start, end: start + num(entry.duration), reads: 0, nodes: null, page: false });
      const runStart = num(entry.processingStart);
      const runEnd = num(entry.processingEnd);
      if (runEnd > runStart) keepRecord({ name: `evt-run:${type}`, start: runStart, end: runEnd, reads: 0, nodes: null, page: false });
    }
  }

  function frameBrief(entry: LoafEntry): { t: number; dur_ms: number; blocking_ms: number; fn: string; src: string } {
    const top = longestScripts(entry)[0];
    return {
      t: whole(num(entry.startTime)),
      dur_ms: whole(num(entry.duration)),
      blocking_ms: whole(num(entry.blockingDuration)),
      fn: codeName(top?.sourceFunctionName),
      src: top ? srcName(top.sourceURL, num(top.sourceCharPosition)) : "",
    };
  }

  function noteFrames(entries: readonly unknown[]): void {
    for (const one of entries) {
      const entry = one as LoafEntry;
      const dur = num(entry.duration);
      if (dur < 50) continue;
      framesOver50 += 1;
      if (dur >= DEBUG_SLOW_FRAME_MS) {
        framesOver100 += 1;
        slow = true;
      }
      if (dur > maxFrame) maxFrame = dur;
      slowFrames.push(frameBrief(entry));
      slowFrames.sort((a, b) => b.dur_ms - a.dur_ms);
      if (slowFrames.length > DEBUG_SUMMARY_FRAMES) slowFrames.length = DEBUG_SUMMARY_FRAMES;
    }
  }

  function frameDetail(entry: LoafEntry): unknown {
    const end = num(entry.startTime) + num(entry.duration);
    const renderStart = num(entry.renderStart);
    const styleStart = num(entry.styleAndLayoutStart);
    return {
      blocking_ms: whole(num(entry.blockingDuration)),
      ...(renderStart > 0 ? { render_ms: whole(end - renderStart) } : {}),
      ...(styleStart > 0 ? { style_layout_ms: whole(end - styleStart) } : {}),
      scripts: longestScripts(entry).slice(0, DEBUG_FRAME_SCRIPTS).map((script) => ({
        fn: codeName(script?.sourceFunctionName),
        src: srcName(script?.sourceURL, num(script?.sourceCharPosition)),
        invoker_type: typeof script?.invokerType === "string" && INVOKER_TYPES.includes(script.invokerType) ? script.invokerType : "other",
        invoker: invokerName(script?.invoker),
        dur_ms: whole(num(script?.duration)),
        forced_layout_ms: whole(num(script?.forcedStyleAndLayoutDuration)),
      })),
    };
  }

  // --- the profiler ---------------------------------------------------------------------------

  function sampleMs(): number {
    const interval = profiler?.sampleInterval;
    return typeof interval === "number" && interval > 0 ? interval : DEBUG_SAMPLE_MS;
  }

  function startProfiler(): void {
    if (!deps.startProfiler) return;
    try {
      profiler = deps.startProfiler(DEBUG_SAMPLE_MS, DEBUG_PROFILE_BUFFER);
    } catch (_error) {
      profiler = null;
    }
    caps.policy = caps.policy || profiler !== null;
    caps.sample_ms = profiler ? whole(sampleMs()) : 0;
    if (profiler) rollTimer = deps.clock.setTimeout(() => roll(), DEBUG_PROFILE_ROLL_MS);
  }

  /** Stop the running profiler and start the next; summarise the stalls waiting on the old one. */
  function roll(): void {
    if (rollTimer !== null) {
      deps.clock.clearTimeout(rollTimer);
      rollTimer = null;
    }
    const old = profiler;
    if (!old || !on) return;
    const asked = waiting.splice(0);
    const interval = sampleMs();
    profiler = null;
    rollingSince = deps.now();
    startProfiler();
    const finish = (trace: unknown): void => {
      rollingSince = null;
      for (const one of asked) {
        try {
          one.late(trace === null ? null : summariseProfile(trace, one.start, one.end, interval));
        } catch (_error) {
          // A late attribution never breaks the page.
        }
      }
    };
    old.stop().then(finish, () => finish(null));
  }

  function stopProfiler(): void {
    if (rollTimer !== null) deps.clock.clearTimeout(rollTimer);
    rollTimer = null;
    const old = profiler;
    profiler = null;
    waiting = [];
    rollingSince = null;
    if (old) old.stop().then(() => undefined, () => undefined);
  }

  // --- attribution ----------------------------------------------------------------------------

  function attribute(start: number, end: number, entry: unknown, late: (profile: unknown) => void): unknown {
    if (!on) return null;
    const lo = Math.min(start, end);
    const hi = Math.max(start, end);
    const inWindow = new Map<string, {
      n: number; ms: number; reads: number; nodes: number | null; longest: number;
      s: number | null; d: number | null; chars: number; rows: number;
    }>();
    const covered: Array<[number, number]> = [];
    const open: Array<{ name: string; age: number }> = [];
    const before: SpanRecord[] = [];
    const now = deps.now();
    const considered: SpanRecord[] = ring.filter((one): one is SpanRecord => one !== undefined);
    // Still running now (a stall seen from inside a span): counted to now.
    for (const running of openStack) {
      considered.push({ name: running.name, start: running.start, end: now, reads: 0, nodes: null, page: true, chars: running.chars, rows: running.rows });
    }
    for (const one of considered) {
      if (one.end > lo && one.start < hi) {
        const from = Math.max(lo, one.start);
        const to = Math.min(hi, one.end);
        const kept = inWindow.get(one.name) ?? { n: 0, ms: 0, reads: 0, nodes: null, longest: -1, s: null, d: null, chars: 0, rows: 0 };
        kept.n += 1;
        kept.ms += to - from;
        kept.reads += one.reads;
        kept.chars += one.chars ?? 0;
        kept.rows += one.rows ?? 0;
        if (one.end - one.start > kept.longest) {
          kept.longest = one.end - one.start;
          kept.nodes = one.nodes;
          kept.s = one.s ?? null;
          kept.d = one.d ?? null;
        }
        inWindow.set(one.name, kept);
        if (one.page) covered.push([from, to]);
      }
      if (one.page && one.start <= lo && one.end > lo) open.push({ name: one.name, age: lo - one.start });
      if (one.page && one.end <= lo) before.push(one);
    }
    covered.sort((a, b) => a[0] - b[0]);
    let coveredMs = 0;
    let reach = -Infinity;
    for (const [from, to] of covered) {
      if (to <= reach) continue;
      coveredMs += to - Math.max(from, reach);
      reach = to;
    }
    const attribution: Record<string, unknown> = {
      spans: [...inWindow.entries()]
        .sort((a, b) => b[1].ms - a[1].ms)
        .slice(0, DEBUG_EVENT_SPANS)
        .map(([name, kept]) => ({
          name: codeName(name), n: kept.n, ms: whole(kept.ms), reads: kept.reads,
          ...(kept.nodes !== null ? { nodes: whole(kept.nodes) } : {}),
          ...(kept.s !== null ? { s: whole(kept.s) } : {}),
          ...(kept.d !== null ? { d: whole(kept.d) } : {}),
          ...(kept.chars > 0 ? { chars: kept.chars } : {}),
          ...(kept.rows > 0 ? { rows: kept.rows } : {}),
        })),
      open: open.sort((a, b) => b.age - a.age).slice(0, DEBUG_OPEN_SPANS)
        .map((one) => ({ name: codeName(one.name), age_ms: whole(one.age) })),
      recent: before.sort((a, b) => b.end - a.end).slice(0, DEBUG_RECENT_SPANS)
        .map((one) => ({ name: codeName(one.name), dur_ms: whole(one.end - one.start), ago_ms: whole(lo - one.end) })),
      unattributed_ms: whole(hi - lo - coveredMs),
    };
    if (entry !== null && entry !== undefined && typeof entry === "object") attribution.frame = frameDetail(entry as LoafEntry);
    if (profiler) {
      waiting.push({ start: lo, end: hi, late });
      // Off the observer's own task: the stall's samples are in the running profiler already.
      deps.clock.setTimeout(() => roll(), 0);
    }
    return attribution;
  }

  function pending(): boolean {
    if (!on) return false;
    if (rollingSince !== null && deps.now() - rollingSince < PENDING_GIVE_UP_MS) return true;
    return waiting.length > 0 && profiler !== null;
  }

  // --- the summary ----------------------------------------------------------------------------

  function summary(): unknown {
    if (!on) return null;
    caps.sample_ms = profiler ? whole(sampleMs()) : 0;
    return {
      caps: { ...caps },
      since: whole(periodStart),
      spans: [...totals.entries()]
        .sort((a, b) => b[1].total - a[1].total)
        .slice(0, DEBUG_SUMMARY_SPANS)
        .map(([name, total]) => ({ name: codeName(name), n: total.n, total_ms: whole(total.total), max_ms: whole(total.max) })),
      frames_over_50: framesOver50,
      frames_over_100: framesOver100,
      max_frame_ms: whole(maxFrame),
      frames: slowFrames.map((frame) => ({ ...frame })),
    };
  }

  function posted(): void {
    periodStart = deps.now();
    totals = new Map();
    framesOver50 = 0;
    framesOver100 = 0;
    maxFrame = 0;
    slowFrames = [];
    slow = false;
    lastPostAt = periodStart;
  }

  function armSummary(): void {
    summaryTimer = deps.clock.setTimeout(() => {
      summaryTimer = null;
      if (!on) return;
      summaryTick();
      armSummary();
    }, DEBUG_SUMMARY_EVERY_MS);
  }

  /** At most one summary-only report a minute, and only after something slow. */
  function summaryTick(): void {
    if (!slow || summarySending || deps.now() - lastPostAt < DEBUG_SUMMARY_EVERY_MS) return;
    let postable = false;
    try {
      postable = deps.canPost();
    } catch (_error) {
      postable = false;
    }
    if (!postable) return;
    const body = summary();
    if (!body) return;
    summarySending = true;
    lastPostAt = deps.now();
    void deps.send(JSON.stringify({ v: 1, startedAt: whole(createdAt), events: [], debug: body })).then((took) => {
      summarySending = false;
      if (took) posted();
    }, () => {
      summarySending = false;
    });
  }

  // --- on and off -----------------------------------------------------------------------------

  function begin(): void {
    if (on) return;
    on = true;
    let types: readonly string[] = [];
    try {
      types = deps.supportedEntryTypes();
    } catch (_error) {
      types = [];
    }
    let profilerExists = false;
    try {
      profilerExists = deps.profilerExists ? deps.profilerExists() : false;
    } catch (_error) {
      profilerExists = false;
    }
    caps = {
      loaf: types.includes("long-animation-frame"),
      longtask: types.includes("longtask"),
      event: types.includes("event"),
      profiler: profilerExists,
      policy: false,
      sample_ms: 0,
    };
    posted();
    lastPostAt = -Infinity;
    const watch = (type: string, note: (entries: readonly unknown[]) => void, options?: { durationThreshold?: number }): void => {
      if (!types.includes(type)) return;
      try {
        const unwatch = deps.observe(type, note, options);
        if (unwatch) undo.push(unwatch);
      } catch (_error) {
        // Not in this browser after all.
      }
    };
    watch("long-animation-frame", noteFrames);
    watch("event", noteEvents, { durationThreshold: EVENT_TIMING_MIN_MS });
    const proto = deps.elementPrototype ? deps.elementPrototype() : null;
    if (proto) {
      const methods = proto as Record<string, unknown>;
      for (const key of ["getBoundingClientRect", "getClientRects"]) {
        const original = methods[key];
        if (typeof original !== "function") continue;
        const counted = function (this: unknown, ...args: unknown[]): unknown {
          reads += 1;
          return (original as (...a: unknown[]) => unknown).apply(this, args);
        };
        methods[key] = counted;
        undo.push(() => {
          if (methods[key] === counted) methods[key] = original;
        });
      }
    }
    if (profilerExists) startProfiler();
    armSummary();
  }

  function end(): void {
    if (!on) return;
    on = false;
    for (const putBack of undo.splice(0)) putBack();
    stopProfiler();
    if (summaryTimer !== null) deps.clock.clearTimeout(summaryTimer);
    summaryTimer = null;
    ring.fill(undefined);
    ringAt = 0;
    openStack.length = 0;
    totals = new Map();
    slowFrames = [];
    if (deps.clearMeasures && measured > 0) {
      try {
        deps.clearMeasures();
      } catch (_error) {
        // Nothing to clear.
      }
    }
    measured = 0;
  }

  function flagged(): boolean {
    try {
      return deps.query().replace(/^\?/u, "").split("&").includes(`${CLIENT_DEBUG_FLAG}=1`);
    } catch (_error) {
      return false;
    }
  }

  function start(): void {
    let held = false;
    try {
      held = deps.held();
    } catch (_error) {
      held = false;
    }
    try {
      // The cookie follows the kept choice: the server reads it for the next load's shell.
      if (held !== deps.cookieSet()) deps.setCookie(held);
    } catch (_error) {
      // Cookies refused: the mode runs, without the profiler's policy.
    }
    if (held || flagged()) begin();
    else end();
  }

  function setOn(next: boolean): void {
    try {
      deps.keep?.(next);
    } catch (_error) {
      // Storage refused: on for this load alone.
    }
    try {
      deps.setCookie(next);
    } catch (_error) {
      // Cookies refused.
    }
    if (next) begin();
    else end();
  }

  return {
    enabled: () => on,
    start,
    setOn,
    span,
    spanned,
    note,
    attribute,
    summary,
    posted,
    pending,
    caps: () => ({ ...caps }),
  };
}

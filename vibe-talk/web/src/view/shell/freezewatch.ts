// The page stopped answering, kept for the log (`#2066 vibe-talk-page-freeze`; new in
// view/shell): the phone page locks up — swipes and presses do nothing for a while, then it
// answers again, laggy at first — and nothing on the page could say when, for how long, or
// what it was doing. This watch is always on and draws nothing. It keeps a heartbeat: an
// interval due every [`FREEZE_HEARTBEAT_MS`], whose lateness while the page is in sight is
// a freeze (the main thread ran nothing, this interval included — so the record is made
// after the thaw and says how long the silence lasted). A platform with no interval timer
// cannot tell a freeze from a quiet page, so there the watch does not run at all rather
// than keep half a record: nothing is heard, kept or posted, and a previous page's ring
// stays on the device for a load that can watch. Beside it, where the browser
// offers them, `longtask` and `long-animation-frame` entries over
// [`FREEZE_LONG_TASK_MS`], the frame's with the page code that ran in it. And every hide
// and show (owner thread 299: it recovers after an app switch, laggy at first): the show
// carries how long it was hidden, the live replays still waiting to be drawn, the inputs
// heard while hidden, and how late a zero timer and the first frame ran after it — the
// backlog draining, measured. Each event says what the page was doing: the view, the
// screen, the channel view, one column or two, what read-aloud was doing (a press waiting
// on speech preparing is `pending`, told apart from a busy thread), whether the lists were
// windowed, the reader's last act by kind and how long before, and — for stalls soon after
// a show — how long after it. The last [`FREEZE_MAX_EVENTS`] are kept on the device and
// posted to the server's log (`POST /api/v1/freeze-report`) on a freeze's recovery, on a
// show once its backlog is measured, when the ring fills, and on the next load for what a
// killed page left behind. No ids and no words from the screen ever ride in it: the one
// text is a long frame's invoker, the page code's own.

/** How often the heartbeat interval is due, in milliseconds. */
export const FREEZE_HEARTBEAT_MS = 250;
/** A heartbeat silence at or over this, while in sight, is a freeze, in milliseconds. */
export const FREEZE_GAP_MS = 1000;
/** A long task or long animation frame at or over this is kept, in milliseconds. */
export const FREEZE_LONG_TASK_MS = 200;
/** The most events the ring holds — and the most one report carries: the server's cap. */
export const FREEZE_MAX_EVENTS = 40;
/** The longest invoker an event carries, in characters: the server's cap. */
const SCRIPT_CHARS = 256;
/** The least while between one flush and the next, in milliseconds. */
const FLUSH_QUIET_MS = 5000;

/** Where the ring is kept on the device. */
export const FREEZE_STORAGE_KEY = "vibe-talk.voice.page-freeze";

type FreezeKind = "longtask" | "loaf" | "freeze" | "hidden" | "visible";
type FreezeAction = "none" | "pointer" | "key" | "wheel" | "touch" | "scroll";

/** One event, exactly as the server takes it. */
interface FreezeEvent {
  t: number;
  kind: FreezeKind;
  dur_ms: number;
  view: string;
  screen: string;
  channel_view: string;
  layout: string;
  reading: string;
  windowed: boolean;
  visibility: string;
  action: FreezeAction;
  action_age_ms?: number;
  resume_age_ms?: number;
  script: string;
  replays_waiting?: number;
  inputs_hidden?: number;
  timer_delay_ms?: number;
  raf_delay_ms?: number;
}

/** What the page was doing, in its own words; the watch maps them to the server's. */
export interface FreezeWatchState {
  readonly view: string;
  readonly screen: string;
  readonly channelView: string;
  /** Two columns side by side. */
  readonly twoColumns: boolean;
  /** What read-aloud was doing: `idle`, `playing`, `paused` or `pending`. */
  readonly reading: string;
  /** Only a slice of the lists' rows mounted. */
  readonly windowed: boolean;
}

/** A timer, opaque here. */
type TimerHandle = unknown;

/** What the watch asks of the page. */
export interface FreezeWatchDependencies {
  /** The events' clock, in milliseconds (`performance.now`). */
  now(): number;
  /** Runs something after a delay, and forgets one asked for (the page's clock port). */
  clock: {
    setTimeout(run: () => void, ms: number): TimerHandle;
    clearTimeout(handle: TimerHandle): void;
  };
  /**
   * Run `run` every `ms` until the answered call stops it — the heartbeat. Absent or null
   * where the platform has no interval timer: the watch then does not run (see the head).
   */
  every?(ms: number, run: () => void): (() => void) | null;
  /** The device's small string store; any call may throw where storage is refused. */
  storage: {
    getItem(key: string): string | null;
    setItem(key: string, value: string): void;
    removeItem(key: string): void;
  };
  /** Where visibility and the reader's acts are heard (the document), or null. */
  documentTarget(): EventTarget | null;
  /** Whether the page is out of sight now (`document.hidden`). */
  hidden(): boolean;
  /** Run `run` on the next animation frame, where the platform has one. Absent: no frame measure. */
  requestFrame?(run: () => void): void;
  /**
   * Watch performance entries of `type` (`longtask`, `long-animation-frame`), buffered
   * ones included; calls `note` with each batch's raw entries and answers the unwatch, or
   * null where the browser has no such entries.
   */
  observe(type: string, note: (entries: readonly unknown[]) => void): (() => void) | null;
  /** What the page is doing now. */
  state(): FreezeWatchState;
  /** Live replays still waiting to be drawn, or null where the page cannot say. */
  replaysWaiting(): number | null;
  /** Post one report; true where the server took it, so the watch may forget those events. */
  send(report: string): Promise<boolean>;
  /**
   * Whether a posting may go out now: the page online, in sight, and holding a token the
   * server will accept it from. Absent: always. While it says no, events keep and the
   * heartbeat retries — a freeze report is never a request to make offline, in the
   * background, or to be refused. `#2066` (the offline-cache walks count refusals).
   */
  canPost?(): boolean;
}

/** The watch's commands. */
export interface FreezeWatch {
  /** Begin watching: flush what a previous page left, then watch this one. Again: nothing. */
  start(): void;
  /** Stop watching; what was kept stays on the device for the next load. */
  stop(): void;
  /** Post what is kept now, whatever the quiet since the last posting. */
  flush(): void;
  /** What is kept, as one JSON text: `{v: 1, startedAt, events}`. */
  report(): string;
}

const VIEWS: readonly string[] = ["discord", "voice", "docs"];
const SCREENS: readonly string[] = ["signin", "main", "settings", "help", "welcome"];
const CHANNEL_VIEWS: readonly string[] = ["main", "flat", "thread", "threads"];
const READINGS: readonly string[] = ["idle", "playing", "paused", "pending"];

function oneOf(words: readonly string[], word: string, absent: string): string {
  return words.includes(word) ? word : absent;
}

/** `text` on one line, capped: what the server takes for an invoker. */
function capped(text: string): string {
  const line = text.replace(/[\u0000-\u001f\u007f]+/gu, " ").trim();
  return Array.from(line).slice(0, SCRIPT_CHARS).join("");
}

function whole(ms: number): number {
  return Math.max(0, Math.round(ms));
}

/** No watch running yet. */
export function createFreezeWatch(deps: FreezeWatchDependencies): FreezeWatch {
  let running = false;
  let startedAt = deps.now();
  let events: FreezeEvent[] = [];
  let lastBeat = 0;
  let undo: Array<() => void> = [];
  /** The reader's last act, by kind, and when. */
  let lastAction: FreezeAction = "none";
  let lastActionAt: number | null = null;
  /** When the page last went out of sight and came back, and what was heard between. */
  let hiddenAt: number | null = null;
  let resumedAt: number | null = null;
  let inputsHidden = 0;
  /** The show still being measured: its timer and frame delays land on it when they run. */
  let pendingVisible: FreezeEvent | null = null;
  let sending = false;
  let lastFlushAt = -Infinity;

  function persist(): void {
    try {
      if (events.length === 0) deps.storage.removeItem(FREEZE_STORAGE_KEY);
      else deps.storage.setItem(FREEZE_STORAGE_KEY, JSON.stringify({ v: 1, startedAt, events }));
    } catch (_error) {
      // Storage refused: the ring lives in memory alone, and a reload loses it.
    }
  }

  /** The previous page's kept ring, validated event by event; damage drops, never mends. */
  function stored(): { startedAt: number; events: FreezeEvent[] } | null {
    let text: string | null = null;
    try {
      text = deps.storage.getItem(FREEZE_STORAGE_KEY);
    } catch (_error) {
      return null;
    }
    if (!text) return null;
    try {
      const held = JSON.parse(text) as { v?: unknown; startedAt?: unknown; events?: unknown };
      if (held.v !== 1 || typeof held.startedAt !== "number" || !Array.isArray(held.events)) return null;
      const kept = held.events.filter((one): one is FreezeEvent => {
        if (one === null || typeof one !== "object") return false;
        const event = one as FreezeEvent;
        return typeof event.t === "number" && typeof event.dur_ms === "number"
          && ["longtask", "loaf", "freeze", "hidden", "visible"].includes(event.kind);
      });
      return kept.length > 0 ? { startedAt: held.startedAt, events: kept.slice(-FREEZE_MAX_EVENTS) } : null;
    } catch (_error) {
      return null;
    }
  }

  function mayPost(): boolean {
    try {
      return deps.canPost ? deps.canPost() : true;
    } catch (_error) {
      return false;
    }
  }

  function flushText(text: string, count: number): void {
    if (sending || count === 0 || !mayPost()) return;
    sending = true;
    lastFlushAt = deps.now();
    void deps.send(text).then((took) => {
      sending = false;
      if (took) {
        // Events that arrived while this posted stay: only the posted head goes.
        events = events.slice(count);
        persist();
      }
    }, () => {
      sending = false;
    });
  }

  function flush(force = false): void {
    if (events.length === 0 || sending) return;
    if (!force && deps.now() - lastFlushAt < FLUSH_QUIET_MS) return;
    flushText(JSON.stringify({ v: 1, startedAt, events }), events.length);
  }

  function record(kind: FreezeKind, durMs: number, extra?: Partial<FreezeEvent>): FreezeEvent {
    const t = deps.now();
    const state = deps.state();
    const event: FreezeEvent = {
      t,
      kind,
      dur_ms: whole(durMs),
      view: oneOf(VIEWS, state.view, "other"),
      screen: oneOf(SCREENS, state.screen, "other"),
      channel_view: oneOf(CHANNEL_VIEWS, state.channelView, state.view === "discord" ? "other" : "none"),
      layout: state.twoColumns ? "two_column" : "single",
      reading: oneOf(READINGS, state.reading, "other"),
      windowed: state.windowed,
      visibility: deps.hidden() ? "hidden" : "visible",
      action: lastAction,
      ...(lastActionAt !== null ? { action_age_ms: whole(t - lastActionAt) } : {}),
      ...(resumedAt !== null && kind !== "visible" ? { resume_age_ms: whole(t - resumedAt) } : {}),
      script: "",
      ...extra,
    };
    events.push(event);
    while (events.length > FREEZE_MAX_EVENTS) events.shift();
    persist();
    if (events.length >= FREEZE_MAX_EVENTS) flush();
    return event;
  }

  function beat(): void {
    if (!running) return;
    const t = deps.now();
    const silence = t - lastBeat;
    lastBeat = t;
    // Intervals are throttled out of sight, so a hidden gap is the browser's economy, not
    // a freeze: only an in-sight silence counts, and it posts on its own recovery.
    if (!deps.hidden() && silence >= FREEZE_HEARTBEAT_MS + FREEZE_GAP_MS) {
      record("freeze", silence);
      flush(true);
    } else if (events.length > 0) {
      // What could not go out (offline, hidden, a read token) retries here: the
      // heartbeat is the watch's own clock for "on recovery".
      flush();
    }
  }

  function noteTasks(raw: readonly unknown[], loaf: boolean): void {
    for (const one of raw) {
      const entry = one as {
        readonly startTime?: unknown; readonly duration?: unknown;
        readonly scripts?: readonly {
          readonly duration?: unknown; readonly invokerType?: unknown;
          readonly invoker?: unknown; readonly sourceFunctionName?: unknown;
        }[];
      };
      const duration = typeof entry.duration === "number" ? entry.duration : 0;
      if (duration < FREEZE_LONG_TASK_MS) continue;
      let script = "";
      if (loaf && Array.isArray(entry.scripts) && entry.scripts.length > 0) {
        let longest = entry.scripts[0];
        for (const candidate of entry.scripts) {
          if (typeof candidate?.duration === "number"
            && (typeof longest?.duration !== "number" || candidate.duration > longest.duration)) longest = candidate;
        }
        if (longest) {
          script = capped([longest.invokerType, longest.invoker, longest.sourceFunctionName]
            .filter((word) => typeof word === "string" && word !== "").join(" "));
        }
      }
      const at = typeof entry.startTime === "number" ? entry.startTime : deps.now();
      const event = record(loaf ? "loaf" : "longtask", duration, { script });
      event.t = at;
      persist();
    }
  }

  function noteInput(kind: FreezeAction): void {
    lastAction = kind;
    lastActionAt = deps.now();
    if (deps.hidden()) inputsHidden += 1;
  }

  /** The show's backlog, measured: the posted event is completed, then the ring posts. */
  function settleVisible(): void {
    if (!pendingVisible) return;
    if (pendingVisible.timer_delay_ms === undefined) return;
    if (deps.requestFrame && pendingVisible.raf_delay_ms === undefined) return;
    pendingVisible = null;
    persist();
    flush(true);
  }

  function noteVisibility(): void {
    const t = deps.now();
    if (deps.hidden()) {
      hiddenAt = t;
      inputsHidden = 0;
      // A page killed while hidden takes its memory with it; the ring is already stored.
      record("hidden", 0);
      return;
    }
    resumedAt = t;
    const waiting = deps.replaysWaiting();
    const event = record("visible", hiddenAt !== null ? t - hiddenAt : 0, {
      ...(waiting !== null ? { replays_waiting: whole(waiting) } : {}),
      inputs_hidden: inputsHidden,
    });
    hiddenAt = null;
    inputsHidden = 0;
    pendingVisible = event;
    deps.clock.setTimeout(() => {
      if (pendingVisible === event) event.timer_delay_ms = whole(deps.now() - t);
      settleVisible();
    }, 0);
    if (deps.requestFrame) {
      deps.requestFrame(() => {
        if (pendingVisible === event) event.raf_delay_ms = whole(deps.now() - t);
        settleVisible();
      });
    } else {
      settleVisible();
    }
  }

  function start(): void {
    if (running) return;
    // No interval timer, no heartbeat, no watch (see the head): checked before anything
    // is heard, kept or posted, so such a platform's page is exactly as it was.
    lastBeat = deps.now();
    const stopBeat = deps.every ? deps.every(FREEZE_HEARTBEAT_MS, beat) : null;
    if (!stopBeat) return;
    running = true;
    undo = [stopBeat];
    startedAt = deps.now();
    events = [];
    lastAction = "none";
    lastActionAt = null;
    hiddenAt = deps.hidden() ? startedAt : null;
    resumedAt = null;
    inputsHidden = 0;
    pendingVisible = null;
    lastFlushAt = -Infinity;
    // What a previous page left behind posts first, under its own clock.
    const left = stored();
    if (left && !mayPost()) {
      // Not postable yet (offline, hidden, a read token, config unsaid): the leavings
      // ride at the head of this page's ring until the heartbeat can post them.
      events = [...left.events, ...events].slice(-FREEZE_MAX_EVENTS);
      persist();
    } else if (left) {
      sending = true;
      lastFlushAt = deps.now();
      void deps.send(JSON.stringify({ v: 1, startedAt: left.startedAt, events: left.events })).then((took) => {
        sending = false;
        if (took) persist();
        else {
          // Kept: the leavings ride again at the head of this page's ring.
          events = [...left.events, ...events].slice(-FREEZE_MAX_EVENTS);
          persist();
        }
      }, () => {
        sending = false;
        events = [...left.events, ...events].slice(-FREEZE_MAX_EVENTS);
        persist();
      });
    }
    for (const [type, loaf] of [["longtask", false], ["long-animation-frame", true]] as const) {
      try {
        const unwatch = deps.observe(type, (entries) => noteTasks(entries, loaf));
        if (unwatch) undo.push(unwatch);
      } catch (_error) {
        // No such entries in this browser: the heartbeat alone watches.
      }
    }
    const doc = deps.documentTarget();
    if (doc) {
      doc.addEventListener("visibilitychange", noteVisibility);
      undo.push(() => doc.removeEventListener("visibilitychange", noteVisibility));
      const acts: ReadonlyArray<readonly [string, FreezeAction]> = [
        ["pointerdown", "pointer"], ["keydown", "key"], ["wheel", "wheel"],
        ["touchstart", "touch"], ["scroll", "scroll"],
      ];
      for (const [type, kind] of acts) {
        const hear = (): void => noteInput(kind);
        doc.addEventListener(type, hear, { capture: true, passive: true });
        undo.push(() => doc.removeEventListener(type, hear, true));
      }
    }
  }

  function stop(): void {
    if (!running) return;
    running = false;
    for (const putBack of undo.splice(0)) putBack();
    pendingVisible = null;
    persist();
  }

  return {
    start,
    stop,
    flush: () => flush(true),
    report: () => JSON.stringify({ v: 1, startedAt, events }),
  };
}

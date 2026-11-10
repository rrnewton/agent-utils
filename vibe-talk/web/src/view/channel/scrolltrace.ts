// What moved the lists (`#1144 scrollback-typing-jump`, `#1208 scroll-trace-layout-shift`; `#624
// voice-page-modularity`, new in view/channel): while it runs, every write to either list's place —
// its `scrollTop`, its `scrollTo`/`scrollBy` — then every `scrollIntoView` anywhere and every scroll
// either list reports, with when and from where to where, the code that asked, the focus, the
// composer's place, the viewport's, each list's heights and the first row the reader could see.
// And what moved what the reader was looking at without any scroll at all: a layout shift in the
// viewport (its value, whether a hand was just in it, and its sources' before and after), a row
// replaced for the same message, a selection lost with no hand in it. The last
// [`SCROLL_TRACE_MAX_ENTRIES`] of them, over the last minute of them, are kept; `report` says them
// as JSON for the page to post to the server's log. A qualifying shift or selection loss posts it
// by itself, at most once a minute. Off is wholly off: starting wraps `Element.prototype`,
// stopping puts each piece back as it was.

/** The most scrolls one trace holds. */
export const SCROLL_TRACE_MAX_ENTRIES = 600;
/** How far back, in milliseconds from the newest scroll, one trace holds. */
export const SCROLL_TRACE_WINDOW_MS = 60000;
/** The least while between one trace posting itself and the next, in milliseconds. */
export const SCROLL_TRACE_AUTO_SEND_MS = 60000;
/** How long after the reader's last touch, press or key a move still counts as theirs, in ms. */
export const SCROLL_TRACE_INPUT_QUIET_MS = 500;
/** A layout shift at or above this moved what the reader was looking at (a viewport fraction). */
const SHIFT_MIN = 0.01;

/** The two lists whose places are traced. */
const TRACE_AREAS: readonly string[] = ["scroll-area", "side-scroll"];
/** The longest id a trace carries, in characters: the server's cap. */
const ID_CHARS = 128;
/** The longest anchor row id a trace carries, in characters: the server's cap. */
const ANCHOR_ID_CHARS = 256;
/** The longest stack a trace carries, in characters: the server's cap. */
const STACK_CHARS = 512;

/** The first row the reader could see in an area, and how far down it stood. */
export interface ScrollTraceAnchor {
  readonly id: string;
  readonly offset: number;
}

/** The visual viewport, as far as the trace reads it. */
interface TraceViewport {
  readonly height: number;
  readonly offsetTop?: number;
}

/** What the trace asks of the page. */
export interface ScrollTraceDependencies {
  /** The page's element by id (`leftEl`), or null where the page has none. */
  lookup(id: string): HTMLElement | null;
  /** The element with the focus. */
  activeElement(): Element | null;
  /** The browser's visual viewport, or null where it has none. */
  viewport(): TraceViewport | null;
  /** The first row the reader could see in `area` (the page's `scrollAnchor`, named by row). */
  anchor(area: HTMLElement): ScrollTraceAnchor | null;
  /** `Element.prototype`, whose scroll pieces the trace wraps while it runs. */
  elementPrototype(): Element;
  /** The entries' clock, in milliseconds (`performance.now`). */
  now(): number;
  /** A call stack's raw words. Absent: a new `Error`'s. */
  stack?(): string;
  /** The row containers whose children are watched for a row replaced (absent: the two lists). */
  containers?(): readonly Element[];
  /** Where input and selection changes are heard (the document), or null where there is none. */
  documentTarget?(): EventTarget | null;
  /** The reader's selection now: whether it has collapsed, its length in characters (never its
   * words), and a descriptor of where its anchor stood. Null where there is no selection. */
  selection?(): { readonly collapsed: boolean; readonly length: number; readonly desc: string } | null;
  /** Post a report without being asked (a qualifying shift or selection loss). */
  autoSend?(report: string): void;
}

/** The trace's commands. */
export interface ScrollTrace {
  /** Begin tracing, afresh. Done while running does nothing. */
  start(): void;
  /** Stop, and put every wrapped piece back. What was traced stays, for `report`. */
  stop(): void;
  /** What was traced, as one JSON text: `{v: 1, startedAt, entries}`. */
  report(): string;
}

type TraceKind = "write" | "scroll" | "into" | "shift" | "mutate" | "selection";

interface TraceEntry {
  readonly t: number;
  readonly kind: TraceKind;
  readonly id: string;
  readonly from: number | null;
  readonly to: number | null;
  readonly stack: string;
  readonly focus: string | null;
  readonly composer: { readonly top: number; readonly bottom: number; readonly width: number; readonly height: number } | null;
  readonly vv: { readonly height: number; readonly offsetTop: number } | null;
  readonly scrollHeight: number;
  readonly clientHeight: number;
  readonly anchor: ScrollTraceAnchor | null;
  /** A layout shift's size, as a fraction of the viewport (`shift` only). */
  readonly value?: number;
  /** Whether a hand had just been in a layout shift (`shift` only). */
  readonly input?: boolean;
}

type ScrollFn = (this: Element, ...args: readonly unknown[]) => unknown;

/** `text` on one line and no longer than `chars` characters: what the server takes. */
function capped(text: string, chars: number): string {
  const line = text.replace(/[\u0000-\u001f\u007f]+/gu, " ").trim();
  return Array.from(line).slice(0, chars).join("");
}

/**
 * The first four frames of `raw`, trimmed and on one line. The wrapper takes the stack itself, so
 * its own frame and the taker's come off the front and the words begin with the code that asked.
 */
function trimmedStack(raw: string): string {
  const lines = raw.split("\n").map((line) => line.trim()).filter((line) => line !== "");
  const head = lines[0];
  const frames = head !== undefined && head.startsWith("Error") ? lines.slice(1) : lines;
  const asked = frames.slice(2, 6);
  return capped((asked.length > 0 ? asked : frames.slice(0, 4)).join(" | "), STACK_CHARS);
}

/** Where a `scrollTo`/`scrollBy` call asks to be, from `from`; null where it says nothing. */
function scrollCallTop(args: readonly unknown[], from: number, relative: boolean): number | null {
  const first = args[0];
  if (first !== null && typeof first === "object") {
    const top = (first as { readonly top?: unknown }).top;
    return typeof top === "number" ? (relative ? from + top : top) : from;
  }
  const second = args[1];
  return typeof second === "number" ? (relative ? from + second : second) : null;
}

/** No trace running yet. */
export function createScrollTrace(deps: ScrollTraceDependencies): ScrollTrace {
  let running = false;
  /** A scroll being noted: noting one never notes another (a measure that scrolled would loop). */
  let noting = false;
  let startedAt = deps.now();
  let entries: TraceEntry[] = [];
  /** Putting the wrapped pieces back, newest first; null while stopped. */
  let restore: Array<() => void> | null = null;
  /** Each traced list's place as last noted, for a scroll's `from`. */
  const lastTop = new Map<Element, number>();

  function rawStack(): string {
    return deps.stack ? deps.stack() : new Error().stack || "";
  }

  function traced(node: Element): boolean {
    return TRACE_AREAS.includes(node.id);
  }

  function anchorFor(node: Element, kind: TraceKind): ScrollTraceAnchor | null {
    let area: HTMLElement | null = null;
    if (kind === "into") {
      for (const id of TRACE_AREAS) {
        const found = deps.lookup(id);
        if (found && (found === node || found.contains(node))) {
          area = found;
          break;
        }
      }
    } else {
      area = node as HTMLElement;
    }
    if (!area) return null;
    const anchor = deps.anchor(area);
    return anchor ? { id: capped(anchor.id, ANCHOR_ID_CHARS), offset: Math.round(anchor.offset) } : null;
  }

  /** One node in a line: its kind, its own id, and the message it stands for where it names one. */
  function describe(node: Element | null | undefined): string {
    if (!node) return "gone";
    const id = node.getAttribute ? (node.getAttribute("data-id") || node.getAttribute("data-context-id") || "") : "";
    return capped(`${node.tagName || "?"}#${node.id || ""}${id ? `[${id}]` : ""}`, ID_CHARS);
  }

  function note(kind: TraceKind, node: Element, from: number | null, to: number | null, stack: string,
    extra?: { readonly value?: number; readonly input?: boolean }): TraceEntry | null {
    if (!running || noting) return null;
    noting = true;
    try {
      const t = deps.now();
      const active = deps.activeElement();
      const composerEl = deps.lookup("channel-composer");
      const rect = composerEl ? composerEl.getBoundingClientRect() : null;
      const vv = deps.viewport();
      const entry: TraceEntry = {
        t,
        kind,
        id: capped(node.id || "", ID_CHARS),
        from,
        to,
        stack,
        focus: active && active.id ? capped(active.id, ID_CHARS) : null,
        composer: rect
          ? { top: Math.round(rect.top), bottom: Math.round(rect.bottom), width: Math.round(rect.width), height: Math.round(rect.height) }
          : null,
        vv: vv ? { height: Math.round(vv.height), offsetTop: Math.round(vv.offsetTop || 0) } : null,
        scrollHeight: node.scrollHeight || 0,
        clientHeight: node.clientHeight || 0,
        anchor: anchorFor(node, kind),
        ...(extra && extra.value !== undefined ? { value: extra.value } : {}),
        ...(extra && extra.input !== undefined ? { input: extra.input } : {}),
      };
      entries.push(entry);
      while (entries.length > SCROLL_TRACE_MAX_ENTRIES) entries.shift();
      let oldest = entries[0];
      while (oldest !== undefined && t - oldest.t > SCROLL_TRACE_WINDOW_MS) {
        entries.shift();
        oldest = entries[0];
      }
      if (kind !== "into") lastTop.set(node, node.scrollTop);
      return entry;
    } catch (_error) {
      // Tracing a scroll never breaks the scroll.
      return null;
    } finally {
      noting = false;
    }
  }

  /** When the reader last touched, pressed or keyed, and when the trace last posted itself. */
  let lastInputAt = -Infinity;
  let lastAutoAt = -Infinity;
  /** The selection as last heard of: whether it had collapsed, and where a live one stood. */
  let selectionWas: { collapsed: boolean; desc: string; length: number } = { collapsed: true, desc: "", length: 0 };

  /** A qualifying shift or selection loss posts the minute, at most once its own minute. */
  function maybeAutoSend(): void {
    if (!deps.autoSend || !running) return;
    const now = deps.now();
    if (now - lastAutoAt < SCROLL_TRACE_AUTO_SEND_MS) return;
    lastAutoAt = now;
    try {
      deps.autoSend(JSON.stringify({ v: 1, startedAt, entries }));
    } catch (_error) {
      // A report that cannot go changes nothing on the page.
    }
  }

  /** The reader acting, heard on the document: their moves are theirs from here on. */
  function noteInput(): void {
    lastInputAt = deps.now();
  }

  function noteSelection(): void {
    if (!deps.selection) return;
    const sel = deps.selection();
    if (!sel) return;
    if (!selectionWas.collapsed && sel.collapsed
      && deps.now() - lastInputAt >= SCROLL_TRACE_INPUT_QUIET_MS) {
      const area = deps.lookup("scroll-area");
      if (area) {
        note("selection", area, null, null,
          capped(`was ${selectionWas.desc} len ${selectionWas.length}`, STACK_CHARS));
        maybeAutoSend();
      }
    }
    selectionWas = { collapsed: sel.collapsed, desc: sel.desc, length: sel.length };
  }

  function noteShift(raw: unknown): void {
    const shift = raw as {
      readonly value?: unknown; readonly hadRecentInput?: unknown;
      readonly sources?: readonly { readonly node?: Element | null;
        readonly previousRect?: { x: number; y: number; width: number; height: number };
        readonly currentRect?: { x: number; y: number; width: number; height: number } }[];
    };
    const value = typeof shift.value === "number" ? shift.value : 0;
    const input = Boolean(shift.hadRecentInput);
    const vv = deps.viewport();
    const vvHeight = vv ? vv.height : 0;
    const parts: string[] = [];
    let inViewport = !shift.sources || shift.sources.length === 0;
    for (const source of (shift.sources || []).slice(0, 3)) {
      const r = source.currentRect;
      if (r && r.y + r.height > 0 && r.y < vvHeight) inViewport = true;
      const p = source.previousRect;
      parts.push(`${describe(source.node)} ${p ? `${Math.round(p.x)},${Math.round(p.y)} ${Math.round(p.width)}x${Math.round(p.height)}` : "-"} -> ${r ? `${Math.round(r.x)},${Math.round(r.y)} ${Math.round(r.width)}x${Math.round(r.height)}` : "-"}`);
    }
    const area = deps.lookup("scroll-area");
    if (!area) return;
    note("shift", area, null, null, capped(parts.join(" ; "), STACK_CHARS), { value, input });
    if (value >= SHIFT_MIN && !input && inViewport
      && deps.now() - lastInputAt >= SCROLL_TRACE_INPUT_QUIET_MS) maybeAutoSend();
  }

  function noteMutations(records: readonly { readonly removedNodes: ArrayLike<Element>; readonly addedNodes: ArrayLike<Element> }[],
    container: Element): void {
    const idOf = (node: Element): string => {
      if (!node || typeof node.getAttribute !== "function") return "";
      const own = node.getAttribute("data-id") || node.getAttribute("data-context-id") || "";
      if (own) return own;
      const inner = typeof node.querySelector === "function" ? node.querySelector("[data-id]") : null;
      return inner && typeof inner.getAttribute === "function" ? inner.getAttribute("data-id") || "" : "";
    };
    const removed = new Map<string, string>();
    const added = new Map<string, string>();
    for (const record of records) {
      for (const node of Array.from(record.removedNodes)) {
        const id = idOf(node);
        if (id) removed.set(id, describe(node));
      }
      for (const node of Array.from(record.addedNodes)) {
        const id = idOf(node);
        if (id) added.set(id, describe(node));
      }
    }
    for (const [id, gone] of removed) {
      const back = added.get(id);
      if (back) note("mutate", container, null, null, capped(`${id} removed ${gone} added ${back}`, STACK_CHARS));
    }
  }

  /** Replace `name` on `proto` with `value`, and remember putting it back. Nothing if refused. */
  function wrap(proto: Element, undo: Array<() => void>, name: string, value: unknown): void {
    const held = Object.getOwnPropertyDescriptor(proto, name);
    try {
      Object.defineProperty(proto, name, { configurable: true, writable: true, value });
    } catch (_error) {
      return;
    }
    undo.push(() => {
      try {
        if (held) Object.defineProperty(proto, name, held);
        else delete (proto as unknown as Record<string, unknown>)[name];
      } catch (_error) {
        // What cannot be put back was never replaced above.
      }
    });
  }

  function start(): void {
    if (running) return;
    running = true;
    startedAt = deps.now();
    entries = [];
    lastTop.clear();
    const undo: Array<() => void> = [];
    restore = undo;
    const proto = deps.elementPrototype();

    const topHeld = Object.getOwnPropertyDescriptor(proto, "scrollTop");
    if (topHeld && typeof topHeld.set === "function") {
      const heldGet = topHeld.get;
      const heldSet = topHeld.set;
      try {
        Object.defineProperty(proto, "scrollTop", {
          ...topHeld,
          set(this: Element, value: number): void {
            const stack = trimmedStack(rawStack());
            const from = heldGet ? Number(Reflect.apply(heldGet, this, [])) : this.scrollTop;
            Reflect.apply(heldSet, this, [value]);
            if (traced(this)) note("write", this, from, Number(value), stack);
          },
        });
        undo.push(() => {
          try {
            Object.defineProperty(proto, "scrollTop", topHeld);
          } catch (_error) {
            // As `wrap`'s.
          }
        });
      } catch (_error) {
        // A browser that holds its own fast is traced on its other pieces alone.
      }
    }

    for (const [name, relative] of [["scrollTo", false], ["scrollBy", true]] as const) {
      const held = (proto as unknown as Record<string, unknown>)[name];
      if (typeof held !== "function") continue;
      const original = held as ScrollFn;
      wrap(proto, undo, name, function (this: Element, ...args: unknown[]): unknown {
        const stack = trimmedStack(rawStack());
        const from = this.scrollTop;
        const to = scrollCallTop(args, from, relative);
        const answer = Reflect.apply(original, this, args);
        if (traced(this)) note("write", this, from, to, stack);
        return answer;
      });
    }

    const into = (proto as unknown as Record<string, unknown>).scrollIntoView;
    if (typeof into === "function") {
      const original = into as ScrollFn;
      wrap(proto, undo, "scrollIntoView", function (this: Element, ...args: unknown[]): unknown {
        const stack = trimmedStack(rawStack());
        const answer = Reflect.apply(original, this, args);
        note("into", this, null, null, stack);
        return answer;
      });
    }

    for (const id of TRACE_AREAS) {
      const area = deps.lookup(id);
      if (!area) continue;
      lastTop.set(area, area.scrollTop);
      const onScroll = (): void => {
        const to = area.scrollTop;
        note("scroll", area, lastTop.get(area) ?? to, to, "");
      };
      area.addEventListener("scroll", onScroll, { passive: true });
      undo.push(() => area.removeEventListener("scroll", onScroll));
    }

    lastInputAt = -Infinity;
    lastAutoAt = -Infinity;
    selectionWas = { collapsed: true, desc: "", length: 0 };
    const doc = deps.documentTarget ? deps.documentTarget() : null;
    if (doc) {
      for (const type of ["pointerdown", "keydown", "touchstart", "wheel"]) {
        doc.addEventListener(type, noteInput, { capture: true, passive: true });
        undo.push(() => doc.removeEventListener(type, noteInput, true));
      }
      doc.addEventListener("selectionchange", noteSelection);
      undo.push(() => doc.removeEventListener("selectionchange", noteSelection));
    }

    const shifts = (globalThis as { PerformanceObserver?: unknown }).PerformanceObserver;
    if (typeof shifts === "function") {
      try {
        const observer = new (shifts as new (run: (list: { getEntries(): readonly unknown[] }) => void) => {
          observe(options: { type: string; buffered: boolean }): void; disconnect(): void;
        })((list) => {
          for (const entry of list.getEntries()) noteShift(entry);
        });
        observer.observe({ type: "layout-shift", buffered: true });
        undo.push(() => observer.disconnect());
      } catch (_error) {
        // No layout-shift entries in this browser: the scrolls alone are traced.
      }
    }

    const mutations = (globalThis as { MutationObserver?: unknown }).MutationObserver;
    if (typeof mutations === "function") {
      const containers = deps.containers ? deps.containers()
        : TRACE_AREAS.map((id) => deps.lookup(id)).filter((area) => area !== null);
      for (const container of containers) {
        try {
          const observer = new (mutations as new (run: (records: readonly {
            removedNodes: ArrayLike<Element>; addedNodes: ArrayLike<Element>;
          }[]) => void) => { observe(node: Element, options: { childList: boolean }): void; disconnect(): void;
          })((records) => noteMutations(records, container));
          observer.observe(container, { childList: true });
          undo.push(() => observer.disconnect());
        } catch (_error) {
          // A container that will not be watched is left alone.
        }
      }
    }
  }

  function stop(): void {
    if (!running) return;
    running = false;
    const undo = restore;
    restore = null;
    if (undo) {
      for (let at = undo.length - 1; at >= 0; at -= 1) {
        const putBack = undo[at];
        if (putBack) putBack();
      }
    }
  }

  return {
    start,
    stop,
    report: () => JSON.stringify({ v: 1, startedAt, entries }),
  };
}

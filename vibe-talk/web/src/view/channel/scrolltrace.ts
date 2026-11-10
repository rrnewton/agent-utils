// What moved the lists (`#1144 scrollback-typing-jump`; `#624 voice-page-modularity`, new in
// view/channel): while it runs, every write to either list's place — its `scrollTop`, its
// `scrollTo`/`scrollBy` — then every `scrollIntoView` anywhere and every scroll either list
// reports, with when and from where to where, the code that asked, the focus, the composer's
// place, the viewport's, each list's heights and the first row the reader could see. The last
// [`SCROLL_TRACE_MAX_ENTRIES`] of them, over the last minute of them, are kept; `report` says them
// as JSON for the page to post to the server's log. Off is wholly off: starting wraps
// `Element.prototype`, stopping puts each piece back as it was. Nothing is kept anywhere else,
// and nothing leaves the device until the page sends a report.

/** The most scrolls one trace holds. */
export const SCROLL_TRACE_MAX_ENTRIES = 600;
/** How far back, in milliseconds from the newest scroll, one trace holds. */
export const SCROLL_TRACE_WINDOW_MS = 60000;

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

type TraceKind = "write" | "scroll" | "into";

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

  function note(kind: TraceKind, node: Element, from: number | null, to: number | null, stack: string): void {
    if (!running || noting) return;
    noting = true;
    try {
      const t = deps.now();
      const active = deps.activeElement();
      const composerEl = deps.lookup("channel-composer");
      const rect = composerEl ? composerEl.getBoundingClientRect() : null;
      const vv = deps.viewport();
      entries.push({
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
      });
      while (entries.length > SCROLL_TRACE_MAX_ENTRIES) entries.shift();
      let oldest = entries[0];
      while (oldest !== undefined && t - oldest.t > SCROLL_TRACE_WINDOW_MS) {
        entries.shift();
        oldest = entries[0];
      }
      if (kind !== "into") lastTop.set(node, node.scrollTop);
    } catch (_error) {
      // Tracing a scroll never breaks the scroll.
    } finally {
      noting = false;
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

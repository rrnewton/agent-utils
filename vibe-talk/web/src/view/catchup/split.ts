// `#2263 vibe-talk-discuss-split-pane` (owner request 374): the Discuss panel split into
// panes. The current summary holds still in its own scroller on top; the timeline (outline)
// scrolls on its own below it, at a constant height the reader sets by dragging the boundary
// (the timeline's header row) and can collapse to that one row; the newest follow-up pair
// stands open in its own Latest block between the timeline and the composer, so the composer
// and the latest answer stay in view without the long history above them.
//
// This module owns the timeline pane's size and fold (kept per device through the injected
// keeps, never sent to the server), the boundary's pointer and keyboard handling, the
// collapsed header's count and new-summary hint, and the scroll-owner helpers the panel
// draws through: nothing here writes the summary pane's scroll position, and a timeline
// redraw keeps the reader's row where it stood (owner request 370). The stylesheet lays the
// panes out (22-discuss.css); this code only states the choices on the elements.

/** The least the split needs from an entry of the saved timeline. */
export interface SplitEntry {
  kind: string;
  speaker?: string;
  at_ms?: number;
  created_ms?: number;
}

/**
 * The newest follow-up pair, split off the timeline: the trailing turns of a record whose
 * last entry is a turn (no summary made since). A question and its answer move together;
 * a lone trailing turn moves alone. A record ending on a summary has no Latest pair.
 */
export function splitLatestPair<T extends SplitEntry>(entries: readonly T[]): { history: T[]; latest: T[] } {
  let cut = entries.length;
  const last = entries[cut - 1];
  if (last === undefined || last.kind !== "turn") return { history: entries.slice(), latest: [] };
  cut -= 1;
  const before = entries[cut - 1];
  if (last.speaker !== "you" && before !== undefined && before.kind === "turn" && before.speaker === "you") cut -= 1;
  return { history: entries.slice(0, cut), latest: entries.slice(cut) };
}

/** A stable key for one drawn timeline row, so a redraw can find the row the reader was on. */
export function timelineEntryKey(entry: SplitEntry): string {
  return entry.kind === "summary"
    ? `summary:${entry.created_ms ?? 0}`
    : `turn:${entry.at_ms ?? 0}:${entry.speaker ?? ""}`;
}

/** The timeline body's smallest expanded height, in CSS pixels: one folded row. */
export const TIMELINE_MIN_PX = 48;
/** A boundary released with less body than this collapses the timeline instead. */
export const TIMELINE_COLLAPSE_BELOW_PX = 32;
/** One arrow-key step of the boundary. */
export const TIMELINE_STEP_PX = 24;
/** Movement under this is a tap on the header, not a drag. */
export const TIMELINE_DRAG_SLOP_PX = 6;
/** What a drag leaves the Latest pair at the least: its header and about two lines. */
export const LATEST_KEEP_PX = 72;

/** What a released boundary stands at: collapsed, or this kept height. */
export function releasedTimeline(px: number): { collapsed: boolean; height: number } {
  if (!Number.isFinite(px) || px < TIMELINE_COLLAPSE_BELOW_PX) return { collapsed: true, height: TIMELINE_MIN_PX };
  return { collapsed: false, height: Math.max(TIMELINE_MIN_PX, Math.round(px)) };
}

/**
 * The boundary's keys (the WAI-ARIA window splitter): ArrowUp/ArrowDown move it a step,
 * PageUp/PageDown a quarter of the room, Home to the smallest timeline, End to the tallest,
 * Enter (and Space) collapse or restore. Null for any other key. `room` is the summary
 * pane plus the timeline body, `max` the tallest body the panel can show now.
 */
export function timelineKeyAct(
  key: string,
  height: number,
  max: number,
  room: number,
): { kind: "height"; px: number } | { kind: "toggle" } | null {
  const clamp = (px: number): number => Math.max(TIMELINE_MIN_PX, Math.min(Math.max(TIMELINE_MIN_PX, max), Math.round(px)));
  const page = Math.max(TIMELINE_STEP_PX, Math.round(room / 4));
  switch (key) {
    case "ArrowUp": return { kind: "height", px: clamp(height + TIMELINE_STEP_PX) };
    case "ArrowDown": return { kind: "height", px: clamp(height - TIMELINE_STEP_PX) };
    case "PageUp": return { kind: "height", px: clamp(height + page) };
    case "PageDown": return { kind: "height", px: clamp(height - page) };
    case "Home": return { kind: "height", px: TIMELINE_MIN_PX };
    case "End": return { kind: "height", px: clamp(max) };
    case "Enter":
    case " ": return { kind: "toggle" };
    default: return null;
  }
}

/** The collapsed header's hint: earlier summaries that arrived while it stood collapsed. */
export function timelineNewHint(fresh: number): string {
  if (fresh <= 0) return "";
  return fresh === 1 ? "1 new summary" : `${fresh} new summaries`;
}

/** The element surface the split reads and writes; a host without one of them skips that part. */
interface SplitElement {
  hidden?: boolean;
  textContent: string | null;
  scrollTop?: number;
  scrollHeight?: number;
  clientHeight?: number;
  children?: ArrayLike<unknown>;
  style?: { setProperty?(name: string, value: string): void; removeProperty?(name: string): void };
  setAttribute(name: string, value: string): void;
  getAttribute(name: string): string | null;
  removeAttribute?(name: string): void;
  getBoundingClientRect?(): { top: number; bottom: number; height: number };
  setPointerCapture?(id: number): void;
  closest?(selector: string): unknown;
  querySelector?(selector: string): unknown;
  querySelectorAll?(selector: string): ArrayLike<unknown>;
  open?: boolean;
}

/** What the split asks of the panel; every keep is read on use and may refuse. */
export interface TimelineSplitDependencies {
  maybe(id: string): HTMLElement | null;
  storedHeight?(): number | null;
  keepHeight?(px: number | null): void;
  storedCollapsed?(): boolean | null;
  keepCollapsed?(collapsed: boolean | null): void;
}

export interface TimelineSplit {
  /** Restate the kept height and fold on the panel (no keep). */
  applyStored(): void;
  /** The kept height in force (null: the stylesheet's default share). */
  height(): number | null;
  collapsed(): boolean;
  setHeight(px: number | null, keep: boolean): void;
  setCollapsed(collapsed: boolean, keep: boolean): void;
  /** State the header for `scope`: the row count, and the hint while collapsed. */
  drawHead(scope: string, rows: number, summaries: number): void;
  /** Take the timeline's reading row before a redraw; the returned call puts it back. */
  holdTimelineRow(scope: string): () => void;
  /** A new Latest pair lands on its answer's first text line; null: no Latest pair stands. */
  showLatest(key: string | null): void;
  /** Restate the fold after the sheet or the layout changed (the half-sheet default). */
  restate(): void;
  /** Wire the boundary (header drag, grip keys, fold button). */
  mount(listen: (el: HTMLElement, type: string, handler: (event: never) => void) => void): void;
}

export function createTimelineSplit(deps: TimelineSplitDependencies): TimelineSplit {
  let kept: number | null = null;
  let folded = false;
  // The reader's own fold choice (null: none yet). Without one the default stands: collapsed
  // on a phone's half sheet while a Latest pair stands there (the Latest pair has the room;
  // liaison, 2026-11-25), open everywhere else.
  let chosenFold: boolean | null = null;
  // Where the Latest block was last put by `alignLatest`, so a resize re-aligns it only while
  // the reader has not scrolled it since.
  let latestAligned: number | null = null;
  // The scope the timeline was last drawn for: the first draw of a scope stands at its
  // newest end (next to Latest); every later draw keeps the reader's row.
  let drawnScope: string | null = null;
  let bottomOnExpand = false;
  let latestShown: string | null = null;
  // `#2263` (liaison): earlier summaries seen per scope while the timeline stood open, so a
  // collapsed header can say a new one arrived without opening itself.
  const seenSummaries = new Map<string, number>();
  let headScope: string | null = null;
  let headRows = 0;
  let headSummaries = 0;
  let drag: { startY: number; startPx: number; startScroll: number; max: number; moved: boolean; wasFolded: boolean; px: number } | null = null;

  const el = (id: string): SplitElement | null => deps.maybe(id) as unknown as SplitElement | null;

  function rectHeight(node: SplitElement | null): number | null {
    if (node === null || typeof node.getBoundingClientRect !== "function") return null;
    try {
      const h = node.getBoundingClientRect().height;
      return Number.isFinite(h) ? h : null;
    } catch {
      return null;
    }
  }

  /** The timeline body's height on screen now, or the kept one where nothing is measured. */
  function bodyHeight(): number {
    if (folded) return 0;
    return rectHeight(el("discuss-timeline")) ?? kept ?? TIMELINE_MIN_PX;
  }

  /**
   * The tallest body the panel shows now: the body, plus the summary pane's room past its header
   * row (a dragged boundary is the reader's, so the summary may go down to that; the stylesheet
   * says the same for a kept height), plus what the Latest pair can give while it still shows a
   * few lines.
   */
  function measuredMax(): { max: number; room: number } {
    const body = bodyHeight();
    const pane = rectHeight(el("discuss-current-section"));
    if (pane === null) return { max: 4000, room: body * 3 };
    const floor = rectHeight(el("discuss-current-head")) ?? 44;
    const latestBox = el("discuss-latest");
    const latest = latestBox === null || latestBox.hidden === true ? 0 : (rectHeight(latestBox) ?? 0);
    const spare = Math.max(0, pane - floor) + Math.max(0, latest - LATEST_KEEP_PX);
    return { max: Math.max(TIMELINE_MIN_PX, Math.round(body + spare)), room: body + pane };
  }

  function stateGrip(): void {
    const grip = el("discuss-split");
    if (grip === null) return;
    const px = folded ? 0 : Math.round(bodyHeight());
    grip.setAttribute("aria-valuenow", String(px));
    grip.setAttribute("aria-valuetext", folded ? "Timeline collapsed" : `Timeline ${px} pixels high`);
  }

  function setVar(px: number | null): void {
    const panel = el("discuss-panel");
    if (panel === null) return;
    const style = panel.style;
    if (px === null) {
      panel.removeAttribute?.("data-timeline-size");
      if (style && typeof style.removeProperty === "function") style.removeProperty("--discuss-timeline-height");
    } else {
      panel.setAttribute("data-timeline-size", "kept");
      if (style && typeof style.setProperty === "function") style.setProperty("--discuss-timeline-height", `${Math.round(px)}px`);
    }
  }

  function setHeight(px: number | null, keep: boolean): void {
    kept = px === null || !Number.isFinite(px) ? null : Math.max(TIMELINE_MIN_PX, Math.round(px));
    setVar(kept);
    stateGrip();
    if (keep) {
      try {
        deps.keepHeight?.(kept);
      } catch {
        // Storage refused: the height holds until a reload.
      }
    }
  }

  /** Whether the panel stands as a phone's half sheet (the stylesheet's phone regime). */
  function phoneHalf(): boolean {
    const panel = el("discuss-panel");
    if (panel === null || panel.getAttribute("data-sheet") !== "half") return false;
    try {
      const query = (globalThis as unknown as { matchMedia?: (q: string) => { matches: boolean } }).matchMedia;
      if (typeof query !== "function") return false;
      return !query("(min-width: 900px) and (pointer: fine)").matches;
    } catch {
      return false;
    }
  }

  function defaultFold(): boolean {
    return phoneHalf() && latestShown !== null;
  }

  function drawHeadText(): void {
    const count = el("discuss-timeline-count");
    if (count !== null) count.textContent = headRows > 0 ? ` · ${headRows}` : "";
    const hint = el("discuss-timeline-new");
    if (hint !== null) {
      const seen = headScope === null ? headSummaries : (seenSummaries.get(headScope) ?? headSummaries);
      const text = folded ? timelineNewHint(headSummaries - seen) : "";
      hint.textContent = text;
      hint.hidden = text === "";
    }
  }

  function setCollapsed(collapsed: boolean, keep: boolean): void {
    if (keep) chosenFold = collapsed;
    showFold(collapsed);
    if (keep) {
      try {
        deps.keepCollapsed?.(collapsed);
      } catch {
        // Storage refused: the fold holds until a reload.
      }
    }
  }

  /** The fold in force: the reader's choice, else the default for where the panel stands. */
  function restate(): void {
    const want = chosenFold ?? defaultFold();
    if (want !== folded) showFold(want);
  }

  function showFold(collapsed: boolean): void {
    folded = collapsed;
    const section = el("discuss-timeline-section");
    if (section !== null) section.setAttribute("data-collapsed", collapsed ? "true" : "false");
    const fold = el("discuss-timeline-fold");
    if (fold !== null) {
      fold.setAttribute("aria-expanded", collapsed ? "false" : "true");
      fold.setAttribute("aria-label", collapsed ? "Expand the timeline" : "Collapse the timeline");
    }
    if (!collapsed) {
      // Opened: whatever stands in it now has been seen.
      if (headScope !== null) seenSummaries.set(headScope, headSummaries);
      if (bottomOnExpand) {
        bottomOnExpand = false;
        const box = el("discuss-timeline");
        if (box !== null) alignNewest(box);
      }
    }
    drawHeadText();
    stateGrip();
  }

  function applyStored(): void {
    let px: number | null = null;
    try {
      px = deps.storedHeight?.() ?? null;
    } catch {
      px = null;
    }
    setHeight(px, false);
    try {
      chosenFold = deps.storedCollapsed?.() ?? null;
    } catch {
      chosenFold = null;
    }
    showFold(chosenFold ?? defaultFold());
  }

  function drawHead(scope: string, rows: number, summaries: number): void {
    headScope = scope;
    headRows = rows;
    headSummaries = summaries;
    // A scope first drawn (this load) or drawn open has seen everything it shows.
    if (!folded || !seenSummaries.has(scope)) seenSummaries.set(scope, summaries);
    const section = el("discuss-timeline-section");
    if (section !== null) section.hidden = rows === 0;
    drawHeadText();
    stateGrip();
  }

  function top(node: SplitElement): number | null {
    if (typeof node.getBoundingClientRect !== "function") return null;
    try {
      return node.getBoundingClientRect().top;
    } catch {
      return null;
    }
  }

  /**
   * The timeline's first draw (and its first opening): the newest history next to Latest, with a
   * WHOLE row at the top: the newest row whose top the pane can scroll to, so no row stands
   * with its first line cut off (liaison, 2026-11-25). With nothing measured, the foot.
   */
  function alignNewest(box: SplitElement): void {
    const max = (box.scrollHeight ?? 0) - (box.clientHeight ?? 0);
    const boxTop = top(box);
    if (boxTop === null) {
      if (typeof box.scrollHeight === "number") box.scrollTop = box.scrollHeight;
      return;
    }
    if (max <= 0) {
      box.scrollTop = 0;
      return;
    }
    const base = box.scrollTop ?? 0;
    let target = 0;
    for (const row of Array.from((box.children ?? []) as ArrayLike<SplitElement>)) {
      if (!row.getAttribute?.("data-entry-key")) continue;
      const rowTop = top(row);
      if (rowTop === null) continue;
      const offset = rowTop - boxTop + base;
      if (offset <= max + 0.5) target = offset;
    }
    box.scrollTop = Math.max(0, Math.round(target));
  }

  function holdTimelineRow(scope: string): () => void {
    const box = el("discuss-timeline");
    const first = drawnScope !== scope;
    drawnScope = scope;
    if (box === null) return () => {};
    if (first) {
      return () => {
        if (folded) bottomOnExpand = true;
        else alignNewest(box);
      };
    }
    const rows = (): SplitElement[] => Array.from((box.children ?? []) as ArrayLike<SplitElement>);
    // The folds the reader opened, per row and inside it: a redraw draws every row folded, and
    // a row that closed under the reader would move everything below it.
    const innerFolds = (row: SplitElement): SplitElement[] =>
      Array.from((row.querySelectorAll?.("details") ?? []) as ArrayLike<SplitElement>);
    const folds = new Map<string, { open: boolean; inner: number[] }>();
    for (const row of rows()) {
      const key = row.getAttribute?.("data-entry-key");
      if (!key) continue;
      const inner: number[] = [];
      innerFolds(row).forEach((fold, at) => {
        if (fold.open === true) inner.push(at);
      });
      folds.set(key, { open: row.open === true, inner });
    }
    const reopen = (): void => {
      for (const row of rows()) {
        const kept = folds.get(row.getAttribute?.("data-entry-key") ?? "");
        if (kept === undefined) continue;
        if (kept.open) row.open = true;
        const inner = innerFolds(row);
        for (const at of kept.inner) {
          const fold = inner[at];
          if (fold !== undefined) fold.open = true;
        }
      }
    };
    const boxTop = top(box);
    if (boxTop === null) return reopen;
    const scrollBefore = box.scrollTop ?? 0;
    let anchor: { key: string; offset: number } | null = null;
    for (const row of rows()) {
      const key = row.getAttribute?.("data-entry-key");
      if (!key || typeof row.getBoundingClientRect !== "function") continue;
      const rect = row.getBoundingClientRect();
      if (rect.bottom > boxTop + 1) {
        anchor = { key, offset: rect.top - boxTop };
        break;
      }
    }
    return () => {
      reopen();
      // Restore the scroll the replace may have clamped, then move by the row's own drift.
      box.scrollTop = scrollBefore;
      if (anchor === null) return;
      const held = anchor;
      const again = top(box);
      const row = rows().find((one) => one.getAttribute?.("data-entry-key") === held.key);
      if (again === null || row === undefined) return;
      const rowTop = top(row);
      if (rowTop === null) return;
      const drift = rowTop - again - held.offset;
      if (Math.abs(drift) >= 0.5) box.scrollTop = (box.scrollTop ?? 0) + drift;
    };
  }

  /**
   * A new Latest pair lands on its answer's first BODY text line (its first list item or
   * paragraph), just under the pair's sticky header (which carries the question): the answer
   * is what is new, and a short Latest block (a phone's half sheet, the keyboard up) would
   * otherwise show the labels, a heading and the question just typed. The "Discuss" label and
   * a leading heading scroll off above; an answer with no such block lands under its label; a
   * pair that fits whole shows whole.
   */
  function alignLatest(): void {
    const box = el("discuss-latest");
    if (box === null) return;
    box.scrollTop = 0;
    latestAligned = 0;
    const text = box.querySelector?.(".discuss-agent li, .discuss-agent p") as SplitElement | null | undefined;
    const label = box.querySelector?.(".discuss-agent .discuss-turn-label") as SplitElement | null | undefined;
    const header = box.querySelector?.(".discuss-qa > summary") as SplitElement | null | undefined;
    if (!label || typeof label.getBoundingClientRect !== "function" || typeof box.getBoundingClientRect !== "function") return;
    try {
      const start = text && typeof text.getBoundingClientRect === "function"
        ? text.getBoundingClientRect().top
        : label.getBoundingClientRect().bottom;
      const offset = start - box.getBoundingClientRect().top - (rectHeight(header ?? null) ?? 0);
      if (offset > 0) box.scrollTop = Math.round(offset);
      latestAligned = box.scrollTop ?? 0;
    } catch {
      // Nothing measured: the pair stands from its top.
    }
  }

  function showLatest(key: string | null): void {
    const changed = key !== latestShown;
    latestShown = key;
    restate();
    if (changed && key !== null) alignLatest();
    if (key === null) latestAligned = null;
  }

  /**
   * The Latest block changes height when the keyboard comes or goes, or the panes move: put it
   * back on the answer's first line, unless the reader scrolled it since it was put there.
   */
  function watchLatest(): void {
    const box = el("discuss-latest");
    const Observer = (globalThis as unknown as { ResizeObserver?: new (cb: () => void) => { observe(target: unknown): void } }).ResizeObserver;
    if (box === null || typeof Observer !== "function") return;
    try {
      new Observer(() => {
        if (latestAligned === null || box.hidden === true) return;
        if (Math.abs((box.scrollTop ?? 0) - latestAligned) < 1) alignLatest();
      }).observe(box);
    } catch {
      // No observer: the pair keeps where it was put.
    }
  }

  /** Whether a press lands on the fold-all toggle, which keeps its own click and never drags. */
  function onToggle(target: unknown): boolean {
    const node = target as SplitElement | null;
    if (node === null || typeof node.closest !== "function") return false;
    try {
      return node.closest(".discuss-toggle") !== null;
    } catch {
      return false;
    }
  }

  /**
   * Composing (`data-composing` on the panel while the follow-up composer holds the focus): on a
   * phone the stylesheet sets the Summarize bar aside then. The mark goes when the focus leaves
   * the composer, but never in the middle of a press: a tap elsewhere in the panel takes the
   * focus on its pointerdown, and the bar coming back right then would move the panes' feet
   * under that pointer before its pointerup, so the press would end on something else. So a
   * press in progress finishes first, then the mark goes.
   */
  function mountComposing(listen: (el: HTMLElement, type: string, handler: (event: never) => void) => void): void {
    const panel = deps.maybe("discuss-panel");
    if (panel === null) return;
    const inComposer = (target: unknown): boolean => {
      const node = target as SplitElement | null;
      if (node === null || typeof node.closest !== "function") return false;
      try {
        return node.closest(".discuss-compose") !== null;
      } catch {
        return false;
      }
    };
    let pressing = false;
    let leaving = false;
    const mark = (on: boolean): void => {
      if (on) panel.setAttribute("data-composing", "true");
      else panel.removeAttribute("data-composing");
    };
    listen(panel, "focusin", ((event: FocusEvent) => {
      if (inComposer(event.target)) {
        leaving = false;
        mark(true);
      }
    }) as never);
    listen(panel, "focusout", ((event: FocusEvent) => {
      if (!inComposer(event.target) || inComposer(event.relatedTarget)) return;
      if (pressing) leaving = true;
      else mark(false);
    }) as never);
    listen(panel, "pointerdown", (() => {
      pressing = true;
    }) as never);
    const release = (): void => {
      if (!pressing) return;
      pressing = false;
      if (!leaving) return;
      leaving = false;
      // After this press's click has run (it is dispatched right after pointerup).
      setTimeout(() => {
        const active = (globalThis as { document?: { activeElement?: unknown } }).document?.activeElement;
        if (!inComposer(active)) mark(false);
      }, 0);
    };
    const root = (globalThis as unknown as { document?: HTMLElement }).document;
    if (root !== undefined && typeof (root as { addEventListener?: unknown }).addEventListener === "function") {
      listen(root, "pointerup", release as never);
      listen(root, "pointercancel", release as never);
    }
  }

  function mount(listen: (el: HTMLElement, type: string, handler: (event: never) => void) => void): void {
    mountComposing(listen);
    watchLatest();
    const head = deps.maybe("discuss-timeline-head");
    const fold = deps.maybe("discuss-timeline-fold");
    const grip = deps.maybe("discuss-split");
    let swallowClick = false;
    if (fold !== null) {
      listen(fold, "click", ((event: Event) => {
        if (swallowClick) {
          swallowClick = false;
          if (typeof event.preventDefault === "function") event.preventDefault();
          return;
        }
        setCollapsed(!folded, true);
      }) as never);
    }
    if (head !== null) {
      listen(head, "pointerdown", ((event: PointerEvent) => {
        swallowClick = false;
        if (onToggle(event.target)) return;
        const box = el("discuss-timeline");
        const { max } = measuredMax();
        const startPx = bodyHeight();
        drag = { startY: event.clientY, startPx, startScroll: box?.scrollTop ?? 0, max, moved: false, wasFolded: folded, px: startPx };
      }) as never);
      listen(head, "pointermove", ((event: PointerEvent) => {
        if (drag === null) return;
        const dy = drag.startY - event.clientY;
        if (!drag.moved && Math.abs(dy) < TIMELINE_DRAG_SLOP_PX) return;
        if (!drag.moved) {
          drag.moved = true;
          const holder = head as unknown as SplitElement;
          if (typeof holder.setPointerCapture === "function" && event.pointerId !== undefined) {
            try {
              holder.setPointerCapture(event.pointerId);
            } catch {
              // No pointer to hold: move/up on the header still follow.
            }
          }
          if (folded) setCollapsed(false, false);
        }
        if (typeof event.preventDefault === "function") event.preventDefault();
        const px = Math.max(0, Math.min(drag.max, drag.startPx + dy));
        drag.px = px;
        // Follow the pointer exactly while dragging (below the floor too, so a drag toward
        // collapse is seen); the floor and the collapse are decided on release.
        setVar(Math.max(1, px));
        // The timeline's top edge moves with the boundary: keep its rows where they stand.
        const box = el("discuss-timeline");
        if (box !== null) box.scrollTop = Math.max(0, drag.startScroll - (px - drag.startPx));
        stateGrip();
      }) as never);
      const end = (): void => {
        if (drag === null) return;
        const was = drag;
        drag = null;
        if (!was.moved) return;
        swallowClick = true;
        const released = releasedTimeline(was.px);
        if (released.collapsed) {
          setHeight(kept, false);
          setCollapsed(true, true);
        } else {
          setHeight(released.height, true);
          setCollapsed(false, true);
        }
      };
      listen(head, "pointerup", end as never);
      listen(head, "pointercancel", end as never);
      listen(head, "dblclick", ((event: Event) => {
        if (onToggle(event.target)) return;
        setHeight(null, true);
        setCollapsed(false, true);
      }) as never);
    }
    if (grip !== null) {
      listen(grip, "keydown", ((event: KeyboardEvent) => {
        const { max, room } = measuredMax();
        // Steps go from the kept height (the layout may round the pane a pixel or two under
        // it), so Down then Up comes back to exactly where it was.
        const act = timelineKeyAct(event.key, folded ? 0 : (kept ?? bodyHeight()), max, room);
        if (act === null) return;
        if (typeof event.preventDefault === "function") event.preventDefault();
        if (act.kind === "toggle") {
          setCollapsed(!folded, true);
          return;
        }
        setHeight(act.px, true);
        if (folded) setCollapsed(false, true);
      }) as never);
    }
  }

  return {
    applyStored,
    height: () => kept,
    collapsed: () => folded,
    setHeight,
    setCollapsed,
    drawHead,
    holdTimelineRow,
    showLatest,
    restate,
    mount,
  };
}

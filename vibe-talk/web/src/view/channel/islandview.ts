// Islands in the channel list (`#2426 vibe-talk-sparse-loading-program`, slice R2): the view half of
// core/channels/islands.ts. A far-back reader's island breaks off from the newest window, the
// stretch between is let go of and drawn as a seam that says how much is not loaded, and reaching a
// seam fills it from the side the reader comes from with one id-based read, which never moves the
// row being read (owner rule 370). The newest window always stays, so the newest is a scroll away.
//
// What to let go of, where the gaps stand and how a fill narrows one are decided in core (pure); this
// module measures, draws the seams, and asks the page for its reads and redraws.

import type { Message, TimelineResponse } from "../../contract/index.ts";
import type { ChannelCanon, IslandBudget, ViewGap } from "../../core/channels/index.ts";
import { gapForRun, gapLine, gapPlaces, narrowGap, planBreakOff, planTopTrim, timeOf, withGap } from "../../core/channels/index.ts";

/** Seams stand where the reader can reach them before they fill: this far beyond the band, in px. */
export const GAP_FILL_MARGIN_PX = 600;
/** One fill asks for this many messages past the gap's edge (a page, as a step back asks). */
export const GAP_FILL_PAGE = 50;
/** A break-off waits until what is held passes the budgets by this many rows, so it is not taken back and forth. */
export const BREAK_OFF_SLACK = 25;
/** A seam whose fill failed or brought nothing is not filled again by scrolling for this long (a tap still fills it). */
export const GAP_RETRY_MS = 20000;

/** What the islands ask of the page. */
export interface IslandViewDependencies {
  /** The channel list (`#discord-log`) and its scroller (`#scroll-area`), or null when not drawn. */
  list(): HTMLElement | null;
  area(): HTMLElement | null;
  /** The current channel's store, the view's key in it, and the view ("main", "flat", ...). */
  canon(): ChannelCanon;
  viewKey(): string;
  view(): string;
  /** The current view's rows as drawn, oldest first. */
  shown(): readonly Message[];
  /** The row being read: the jump's landed row, a held filter row, else the first row on screen. */
  readingRow(): Element | null;
  /** The ids of the rows a row element stands for. */
  idsOf(row: Element): string[];
  /** Ids that must stay held: the selection, a jump target, the speaking row. */
  pinned(): ReadonlySet<string>;
  /** Whether a search is open (nothing is let go of while one is: its results are read from what is held). */
  searchOpen(): boolean;
  /** How much one view keeps, or null for no islands (the Threads list, a thread). */
  budget(): IslandBudget | null;
  /** Hold the row being read across `change` with the one scroll owner (`preservingScroll`). */
  holding(change: () => void): void;
  /** Draw the current view again from the store, patching in place (the seams are put back after it). */
  redraw(): void;
  /** One id-based read of the current view (`around`), or null when it failed. */
  readAround(id: string, below: number, above: number): Promise<TimelineResponse | null>;
  /** Fold an around answer into the store, every view, without moving any cover's floor; the view's messages it carried. */
  foldAround(payload: TimelineResponse): Message[];
  /** Let go of these messages from the store (every view). */
  forget(ids: readonly string[]): void;
  /**
   * The rows older than `firstId` (at `firstMs`) went: the view's cover is floored there with more
   * below, and its next step back reads around `firstId` (no cursor reaches below it any more).
   */
  trimTop(firstId: string, firstMs: number): void;
  /** Save the channel's copy on the device, as after any change of what is held. */
  save(): void;
  /** The wall clock, in ms. */
  now(): number;
}

/** The islands' operations for one column. */
export interface IslandView {
  /** After any draw: the seams, where the view's gaps stand. */
  placeGaps(): void;
  /** After a step back, a fill or a landing: let the middle go if a far-back reader holds too much. */
  maybeBreakOff(): boolean;
  /** On scroll: fill a seam the reader is reaching. */
  onScroll(): void;
  /** The current view's gaps. */
  gaps(): readonly ViewGap[];
}

const GAP_ATTR = "data-island-gap";

/** The seams standing in `list`, its own children only. */
function seamsIn(list: Element): HTMLElement[] {
  return [...list.children].filter((node) => (node as HTMLElement).hasAttribute && (node as HTMLElement).hasAttribute(GAP_ATTR)) as HTMLElement[];
}

export function createIslandView(deps: IslandViewDependencies): IslandView {
  const filling = new Set<string>();
  /** When a seam's last fill failed or brought nothing, by key: scrolling waits GAP_RETRY_MS after it. */
  const stalled = new Map<string, number>();
  /** Seams whose last fill failed (an error, or no around answer): they say "· tap to load" (liaison 1126). */
  const failed = new Set<string>();

  const keyOf = (gap: ViewGap): string => `${gap.olderId}|${gap.newerId}`;

  function gapsNow(): readonly ViewGap[] {
    const cover = deps.canon().views.get(deps.viewKey());
    return cover && cover.gaps ? cover.gaps : [];
  }

  function setGaps(gaps: readonly ViewGap[]): void {
    const cover = deps.canon().views.get(deps.viewKey());
    if (!cover) return;
    if (gaps.length) cover.gaps = gaps;
    else delete cover.gaps;
  }

  /** A seam's node, built once per gap and kept while its gap stands. */
  const seams = new Map<string, HTMLElement>();

  function seamFor(gap: ViewGap): HTMLElement {
    const key = keyOf(gap);
    let seam = seams.get(key);
    if (!seam) {
      seam = document.createElement("li");
      seam.setAttribute(GAP_ATTR, key);
      seam.className = "island-gap";
      const line = document.createElement("button");
      line.type = "button";
      line.className = "read-run island-gap-line";
      line.addEventListener("click", (event) => {
        event.stopPropagation();
        const standing = gapsNow().find((one) => keyOf(one) === key);
        if (standing) void fill(standing, sideTowardReader(seam as HTMLElement), true);
      });
      seam.append(line);
      seams.set(key, seam);
    }
    const line = seam.firstElementChild as HTMLElement;
    // An uncounted seam (a link's island, `#2426` R3) says what it is doing without a count.
    const loading = gap.count === null ? "Loading messages…" : `Loading — ${gapLine(gap).replace(" not loaded", "")}`;
    const words = filling.has(key) ? loading : gapLine(gap, failed.has(key));
    if (line.textContent !== words) line.textContent = words;
    seam.setAttribute("aria-busy", filling.has(key) ? "true" : "false");
    return seam;
  }

  function placeGaps(): void {
    const list = deps.list();
    if (!list) return;
    const places = deps.budget() ? gapPlaces(deps.shown(), gapsNow()) : new Map<string, ViewGap>();
    const wanted = new Set<HTMLElement>();
    for (const row of [...list.children]) {
      if ((row as HTMLElement).hasAttribute && (row as HTMLElement).hasAttribute(GAP_ATTR)) continue;
      const ids = deps.idsOf(row);
      const last = ids[ids.length - 1];
      const gap = last !== undefined ? places.get(last) : undefined;
      if (!gap) continue;
      const seam = seamFor(gap);
      wanted.add(seam);
      if (row.nextElementSibling !== seam) list.insertBefore(seam, row.nextElementSibling);
    }
    for (const node of seamsIn(list)) {
      if (!wanted.has(node)) node.remove();
    }
    for (const [key, seam] of seams) if (!wanted.has(seam)) seams.delete(key);
    if (failed.size) {
      const standing = new Set(gapsNow().map(keyOf));
      for (const key of failed) if (!standing.has(key)) failed.delete(key);
    }
  }

  /** The measured height of the rows standing for `ids` in the list, or null when one is not drawn. */
  function measured(ids: ReadonlySet<string>): number | null {
    const list = deps.list();
    if (!list) return null;
    let total = 0;
    let seen = 0;
    for (const row of [...list.children]) {
      const rowIds = deps.idsOf(row);
      if (!rowIds.length || !rowIds.every((id) => ids.has(id))) continue;
      total += (row as HTMLElement).getBoundingClientRect().height;
      seen += rowIds.length;
    }
    return seen === ids.size ? total : null;
  }

  function maybeBreakOff(): boolean {
    const budget = deps.budget();
    const list = deps.list();
    if (!budget || !list || deps.searchOpen()) return false;
    const rows = deps.shown();
    // Nothing to let go of while what is held fits the budgets: no measuring, no pins asked.
    if (rows.length <= budget.newestRows + budget.islandRows + BREAK_OFF_SLACK) return false;
    const anchorRow = deps.readingRow();
    const anchorIds = anchorRow ? deps.idsOf(anchorRow) : [];
    if (!anchorIds.length) return false;
    const held = rows.map((row) => ({ id: String(row.id), ms: timeOf(row, "timestamp") }));
    const anchor = held.findIndex((row) => row.id === anchorIds[0]);
    const pins = new Set([...deps.pinned(), ...anchorIds]);
    const at = anchor < 0 ? null : anchor;
    const runs = planBreakOff(held, at, pins, budget, BREAK_OFF_SLACK);
    const top = planTopTrim(held, at, pins, budget, BREAK_OFF_SLACK);
    if (!runs && top === null) return false;
    const canon = deps.canon();
    let gaps = [...gapsNow()];
    const forget: string[] = [];
    for (const run of runs || []) {
      const ids = new Set(held.slice(run.first, run.last + 1).map((row) => row.id));
      const gap = gapForRun(held, run, measured(ids));
      if (!gap) continue;
      // Every message of the store between the run's edges goes, in any view (a reply between two
      // of Main's rows too), but never a pinned one: the store keeps one continuous piece per island.
      for (const message of canon.messages) {
        const id = String(message.id);
        if (pins.has(id)) continue;
        const ms = timeOf(message, "timestamp");
        if (ids.has(id) || (ms > gap.olderMs && ms < gap.newerMs)) forget.push(id);
      }
      gaps = withGap(gaps, gap);
    }
    const first = top !== null ? held[top] : undefined;
    if (first) {
      // The top trim: every message older than the island's first row, in any view, but no pin (the
      // plan stops at the oldest pinned row, so none is older than `first`). Gaps above it go too.
      for (const message of canon.messages) {
        const id = String(message.id);
        if (!pins.has(id) && timeOf(message, "timestamp") < first.ms) forget.push(id);
      }
      gaps = gaps.filter((gap) => gap.olderMs >= first.ms);
    }
    if (!forget.length) return false;
    const gone = new Set(forget);
    // Another view's cover that lost a row would draw the rest without them: it is read again on
    // entry instead (a view no page has covered is read, views.ts), never shown with a silent hole.
    for (const [key, cover] of canon.views) {
      if (key === deps.viewKey() || key === "threads") continue;
      const lost = canon.messages.some((message) => gone.has(String(message.id)) && coverHolds(key, message, cover.floor, cover.more));
      if (lost) canon.views.delete(key);
    }
    deps.holding(() => {
      deps.forget(forget);
      setGaps(gaps);
      if (first) deps.trimTop(first.id, first.ms);
      deps.redraw();
    });
    deps.save();
    return true;
  }

  /** Whether a view's cover vouched for `message` (its view, at or above its floor). */
  function coverHolds(key: string, message: Message, floor: number | null, more: boolean): boolean {
    const ms = timeOf(message, "timestamp");
    if (more && floor !== null && ms < floor) return false;
    const thread = (message.thread as { id?: unknown; is_root?: unknown } | undefined) || undefined;
    if (key === "flat") return true;
    if (key === "main") return !thread || thread.is_root === true;
    if (key.startsWith("thread:")) return Boolean(thread && String(thread.id) === key.slice("thread:".length));
    return false;
  }

  /** The side a seam is filled from: the side the reader stands on. */
  function sideTowardReader(seam: HTMLElement): "older" | "newer" {
    const area = deps.area();
    if (!area) return "older";
    const box = area.getBoundingClientRect();
    const middle = box.top + (area.clientHeight || 0) / 2;
    const rect = seam.getBoundingClientRect();
    // Below the band's middle: the reader is above it, on its older side.
    return rect.top >= middle ? "older" : "newer";
  }

  async function fill(gap: ViewGap, side: "older" | "newer", asked = false): Promise<void> {
    const key = keyOf(gap);
    if (filling.has(key)) return;
    const stalledAt = stalled.get(key);
    if (!asked && stalledAt !== undefined && deps.now() - stalledAt < GAP_RETRY_MS) return;
    filling.add(key);
    placeGaps();
    let payload: TimelineResponse | null = null;
    const view = deps.viewKey();
    try {
      const anchor = fillAnchor(gap, side);
      payload = side === "older"
        ? await deps.readAround(anchor, 0, GAP_FILL_PAGE)
        : await deps.readAround(anchor, GAP_FILL_PAGE, 0);
    } finally {
      filling.delete(key);
    }
    // The reader moved to another view or channel while it was out: nothing to land here.
    if (deps.viewKey() !== view) return;
    const standing = gapsNow().find((one) => keyOf(one) === key);
    // Only an around answer narrows a gap: a server without the read answers its newest page, which
    // says nothing about this stretch (and would look like the far edge, closing the gap on a hole).
    const around = payload ? (payload as { around?: { held?: unknown; more_below?: unknown; more_above?: unknown } }).around : undefined;
    if (!payload || !around || around.held !== true) failed.add(key);
    else failed.delete(key);
    if (!payload || !standing || !around || around.held !== true) {
      stalled.set(key, deps.now());
      placeGaps();
      return;
    }
    const exhausted = side === "older" ? around.more_above === false : around.more_below === false;
    deps.holding(() => {
      const arrived = deps.foldAround(payload as TimelineResponse);
      const narrowed = narrowGap(standing, side, arrived, exhausted);
      if (narrowed === standing) stalled.set(key, deps.now());
      else stalled.delete(key);
      const rest = gapsNow().filter((one) => keyOf(one) !== key);
      setGaps(narrowed ? withGap(rest, narrowed) : rest);
      deps.redraw();
    });
    deps.save();
    maybeBreakOff();
  }

  /**
   * The message a fill reads around: the gap's edge on `side`, or, where that edge is a thread reply,
   * the nearest top-level message beside it on the same side. Slack's All is Main's stretch with its
   * threads' replies, so the server answers around a reply with the reply alone; around a top-level
   * message it brings the stretch.
   */
  function fillAnchor(gap: ViewGap, side: "older" | "newer"): string {
    const rows = deps.shown();
    const edge = side === "older" ? gap.olderId : gap.newerId;
    const at = rows.findIndex((row) => String(row.id) === edge);
    if (at < 0) return edge;
    const topLevel = (row: Message): boolean => {
      const thread = row.thread as { is_root?: unknown } | null | undefined;
      return !thread || thread.is_root === true;
    };
    if (side === "older") {
      for (let probe = at; probe >= 0; probe -= 1) if (topLevel(rows[probe] as Message)) return String((rows[probe] as Message).id);
    } else {
      for (let probe = at; probe < rows.length; probe += 1) if (topLevel(rows[probe] as Message)) return String((rows[probe] as Message).id);
    }
    return edge;
  }

  /**
   * On scroll: fill a seam the reader is reaching. No frame is asked for (the page's scroll handling
   * asks its one), and nothing is measured while no seam stands, which is nearly always.
   */
  function onScroll(): void {
    if (!deps.budget()) return;
    const list = deps.list();
    const area = deps.area();
    if (!list || !area) return;
    const standing = seamsIn(list);
    if (!standing.length) return;
    const box = area.getBoundingClientRect();
    const top = box.top - GAP_FILL_MARGIN_PX;
    const bottom = box.top + (area.clientHeight || 0) + GAP_FILL_MARGIN_PX;
    for (const seam of standing) {
      const rect = seam.getBoundingClientRect();
      if (rect.bottom < top || rect.top > bottom) continue;
      const key = seam.getAttribute(GAP_ATTR);
      const gap = gapsNow().find((one) => keyOf(one) === key);
      if (gap) void fill(gap, sideTowardReader(seam));
    }
  }

  return { placeGaps, maybeBreakOff, onScroll, gaps: gapsNow };
}

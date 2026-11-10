// Folds, summaries and the chips over the list (`#624 voice-page-modularity`, moved from the legacy
// script into view/channel): which messages are folded and opened, the fold control and the words
// under it, summaries asked for as you scroll, the jump to the newest message, the scroll tools'
// pass over the chips, and ; / : stepping round the ways of showing long messages. Why each behaves
// as it does is told where the legacy script held them; the page's lists, columns, search and
// selection stay the page's and are asked for here.

import { createSummaries, summaryKey } from "../../core/channels/index.ts";
import type { Summaries, SummaryAnswer } from "../../core/channels/index.ts";
import {
  SUMMARY_FAILED_MARK, SUMMARY_MARK, SUMMARY_MODE_OFF, SUMMARY_MODE_ON, SUMMARY_WAITING,
  summaryFailureWords, summaryNoteWords,
} from "../../present/rows/index.ts";
import type { ClockPort } from "../../ports/index.ts";
import { foldedTextLine } from "./fold.ts";
import { rowsBelowReadingPosition } from "./scrollkeep.ts";

/** Past this many characters a message is worth folding (`foldable`); shorter gets no control. */
export const COLLAPSE_OVER_CHARS = 280;

export const FOLD_MORE = "More";
export const FOLD_LESS = "Less";

/** The lists a message with a fold control is drawn in: the call's transcript, and the channel's in both columns. */
export const FOLD_LISTS: readonly string[] = ["transcript", "discord-log", "pinned-log", "side-log", "side-pinned-log"];

/** How far beyond the viewport a row is still worth summarising. */
export const SUMMARY_LOOKAHEAD_PX = 600;

/** Past this distance from the newest, the list is in its scrollback (`renderJumpNewest`). */
export const SCROLLBACK_PX = 64;

/** A row's fold, held on the row itself (`li.foldEntry`). */
export interface FoldEntry {
  li: HTMLElement & { foldEntry?: FoldEntry; searchMarks?: { excerpt?: HTMLElement } | null };
  body: HTMLElement;
  fold: HTMLButtonElement;
  foldText: unknown;
  id: string | null;
  also: string[];
  note: HTMLElement | null;
  mark: HTMLElement | null;
  said: HTMLElement | null;
  elided: HTMLElement | null;
  summaryState?: string;
}

/** A column's record of where its list stands against its newest message. */
export interface ScrollPlace {
  jumpWanted: boolean | number;
  inScrollback: boolean;
}

/** A column's record of its scroll tools' pass. */
export interface ScrollToolsPass {
  held: number;
  owed: boolean;
}

/** A column's pane, as far as the folds and steps read it. */
export interface FoldPane {
  readonly name: string;
  context(): string;
  scroller(): HTMLElement;
  head(): number;
  foot(): number;
}

/** The collapsed-lines count in force, as far as a fold reads it (core/settings owns it). */
export interface CollapsedLinesCount {
  current(): number;
}

/** What the folds, summaries and chips ask of the page. */
export interface FoldingDependencies {
  readonly clock: ClockPort;
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  inOtherColumn(fn: () => void): void;
  eachColumn(fn: () => void): void;
  readerActed(): void;
  preservingScroll(fn: () => void): void;
  threadHue(thread: string): number;
  threadOf(message: unknown): string | null;
  rowMessages(li: Element): unknown[];
  currentScope(): string;
  readonly collapsedLines: CollapsedLinesCount;
  renderSummariesSetting(on: boolean): void;
  keepSummarize(on: boolean): void;
  storedSummarize(): boolean;
  summarizeHeldOn(): boolean;
  api(path: string): Promise<unknown>;
  withThreadQuery(path: string, thread: string | null): string;
  selectedChannelId(): string;
  threadForMessageId(id: string): string | null;
  setStatus(text: string): void;
  view(): string;
  screen(): string;
  searchLinks(): boolean;
  activePlace(): ScrollPlace;
  activeTools(): ScrollToolsPass;
  activeName(): string;
  side(): boolean;
  sideMode(): string;
  sidePane(): FoldPane;
  channelPane(): FoldPane;
  fitOverscroll(area: HTMLElement): void;
  footGap(area: HTMLElement): number;
  bottomSlackPx: number;
  unseenReplies: Record<string, unknown>;
  unseenReply(): { state: string } | null;
  listEmptied(): boolean;
  draftBelowView(): boolean;
  renderJumpBookmark(): void;
  refreshReadingJump(): void;
  visibleList(): Element | null;
  shownChannelList(): Element | null;
  applySearch(): void;
  placeBookmarkLine(): void;
  keepSelection(): void;
  replyReturnRow(): unknown;
  actionMenuOpen(): boolean;
  renderActionMenu(): void;
  renderSendUndo(): void;
  renderBookmarkUndo(): void;
  channelOnScreen(): boolean;
  rowIsRead(li: Element): boolean;
  markedRow(pane: unknown, mark: unknown): HTMLElement | null;
  markOf(pane: unknown): unknown;
  rowInView(pane: unknown, row: HTMLElement): boolean;
  paneRows(pane: unknown): HTMLElement[];
  bringIntoView(pane: unknown, row: HTMLElement): void;
  takeRow(pane: unknown, row: HTMLElement, focus: boolean): void;
  anchorAt(row: HTMLElement): { row: HTMLElement } | null;
  isTextField(node: HTMLElement): boolean;
  nodeWithin(area: HTMLElement, node: HTMLElement): boolean;
  entryState(): { landing: unknown; threadOwed: unknown; threadAwaitsRead: boolean };
  placeFilterAnchor(anchor: { row: HTMLElement }): void;
  filterHold(): { row: HTMLElement; at?: number; offset?: number } | null;
  setFilterHold(hold: { row: HTMLElement; at?: number; offset?: number } | null): void;
  listPlaceTop(area: HTMLElement): number;
  modHeld(event: unknown): boolean;
}

/** The folds', summaries' and chips' commands and questions. */
export interface Folding {
  readonly collapsedLines: CollapsedLinesCount;
  readonly summaryRows: Summaries;
  readonly expandedIds: Set<string>;
  readonly jumpNewestWanted: { voice: boolean | number; discord: boolean | number };
  readonly inScrollback: { voice: boolean; discord: boolean };
  foldsOf(list: Element | null): FoldEntry[];
  setFolded(entry: FoldEntry, folded: boolean): void;
  isFolded(entry: FoldEntry): boolean;
  foldable(li: FoldEntry["li"], meta: Element, body: HTMLElement, text: unknown, messageId?: string | null, alsoIds?: string[]): HTMLButtonElement | null;
  applySummarizeCollapsed(on: boolean): void;
  storedSummarizeCollapsed(): boolean;
  summariesSettingLanded(): void;
  renderSummaries(): void;
  requestVisibleSummaries(limit?: number): void;
  setSummaryMode(on: boolean): void;
  freshScrollPlace(): ScrollPlace;
  freshScrollTools(): ScrollToolsPass;
  setJumpNewest(wanted: boolean, view?: string, arrived?: number | null): void;
  renderJumpNewest(): void;
  renderJumpUnread(): void;
  jumpToUnread(): void;
  visibleFolds(): FoldEntry[];
  holdingScrollTools<T>(draw: () => T): T;
  renderScrollTools(): void;
  setAllFolded(folded: boolean): void;
  foldStepFromKeyboard(way: number, event?: { repeat?: boolean }, target?: EventTarget | null): boolean;
}

/** Everything folded, nothing asked. */
export function createFolding(deps: FoldingDependencies): Folding {
  const el = deps.byId;
  const leftEl = deps.leftById;

  const collapsedLines = deps.collapsedLines;

  function foldsOf(list: Element | null): FoldEntry[] {
    if (!list) return [];
    return ([...list.children] as Array<FoldEntry["li"]>).filter((li) => li.foldEntry).map((li) => li.foldEntry as FoldEntry);
  }

  /** Which messages the reader has OPENED, by message id: the id survives a rebuild of the row. */
  const expandedIds = new Set<string>();

  function summaryThread(entry: FoldEntry): string | null {
    const thread = entry.li.getAttribute("data-unpack-thread") || deps.threadOf(deps.rowMessages(entry.li)[0]);
    return thread && thread !== deps.currentScope() ? thread : null;
  }

  function setFolded(entry: FoldEntry, folded: boolean): void {
    if (entry.id) {
      if (folded) expandedIds.delete(entry.id);
      else expandedIds.add(entry.id);
    }
    entry.li.setAttribute("data-collapsed", folded ? "true" : "false");
    const summarised = folded && summaryRows.mode() && entry.note !== null && entry.summaryState === "ready";
    entry.body.hidden = summarised;
    entry.li.setAttribute("data-summarised", summarised ? "true" : "false");
    const hueThread = summarised ? summaryThread(entry) : null;
    if (hueThread !== null) {
      entry.li.style.setProperty("--summary-hue", String(deps.threadHue(hueThread)));
      entry.li.setAttribute("data-summary-hue", "");
    } else if (entry.li.hasAttribute("data-summary-hue")) {
      entry.li.removeAttribute("data-summary-hue");
      entry.li.style.removeProperty("--summary-hue");
    }
    entry.body.className = folded ? "body clamped" : "body";
    if (entry.note) {
      entry.note.hidden = !(summarised || (summaryRows.mode() && entry.summaryState === "failed"));
    }
    entry.fold.textContent = folded ? FOLD_MORE : FOLD_LESS;
    entry.fold.setAttribute("aria-expanded", folded ? "false" : "true");
    if (entry.elided) entry.elided.hidden = !folded || summarised;
    const marked = entry.li.searchMarks;
    if (marked && marked.excerpt) marked.excerpt.hidden = !folded;
  }

  const isFolded = (entry: FoldEntry): boolean => entry.li.getAttribute("data-collapsed") === "true";

  function foldable(li: FoldEntry["li"], meta: Element, body: HTMLElement, text: unknown, messageId: string | null = null, alsoIds: string[] = []): HTMLButtonElement | null {
    if (String(text === null || text === undefined ? "" : text).length <= COLLAPSE_OVER_CHARS) return null;
    const fold = document.createElement("button");
    fold.className = "fold";
    fold.setAttribute("type", "button");
    const entry: FoldEntry = {
      li, body, fold, foldText: text, id: messageId || null, also: alsoIds,
      note: null, mark: null, said: null, elided: null,
    };
    entry.elided = foldedTextLine(body, text, collapsedLines.current());
    if (entry.elided) li.append(entry.elided);
    if (entry.id) {
      entry.note = document.createElement("div");
      entry.note.className = "summary";
      const mark = document.createElement("span");
      mark.className = "summary-mark";
      mark.textContent = SUMMARY_MARK;
      entry.mark = mark;
      entry.said = document.createElement("span");
      entry.said.className = "summary-text";
      entry.said.textContent = "";
      entry.note.append(mark, entry.said);
      li.append(entry.note);
      applySummaryState(entry);
    }
    li.foldEntry = entry;
    setFolded(entry, !(entry.id !== null && expandedIds.has(entry.id)));
    fold.addEventListener("click", deps.inThisColumn(() => {
      deps.readerActed();
      deps.preservingScroll(() => setFolded(entry, !isFolded(entry)));
      renderScrollTools();
    }));
    meta.append(fold);
    return fold;
  }

  const summaryRows: Summaries = createSummaries({
    clock: deps.clock,
    channel: () => deps.selectedChannelId(),
    threadOf: (id: string) => deps.threadForMessageId(id),
    fetch: (channel: string, id: string, alsoIds: string[], thread: string | null) => {
      const also = alsoIds.length ? `?with=${encodeURIComponent(alsoIds.join(","))}` : "";
      return deps.api(deps.withThreadQuery(
        `/api/v1/channels/${encodeURIComponent(channel)}/messages/${encodeURIComponent(id)}/summary${also}`, thread)) as Promise<SummaryAnswer | null>;
    },
    render: () => scheduleRenderSummaries(),
    failuresReported: (count: number, why: string) => deps.setStatus(summaryFailureWords(count, why)),
    askAgain: () => requestVisibleSummaries(),
  });

  function applySummarizeCollapsed(on: boolean): void {
    const wanted = Boolean(on);
    deps.renderSummariesSetting(wanted);
    deps.keepSummarize(wanted);
    if (wanted !== summaryRows.mode()) setSummaryMode(wanted);
  }

  function storedSummarizeCollapsed(): boolean {
    return deps.storedSummarize();
  }

  function summariesSettingLanded(): void {
    const heldOn = (): boolean => deps.summarizeHeldOn();
    if (!summaryRows.settingLanded(heldOn)) return;
    requestVisibleSummaries();
    deps.inOtherColumn(requestVisibleSummaries);
  }

  function applySummaryState(entry: FoldEntry): void {
    if (!entry.note || !entry.mark || !entry.said) return;
    const held = summaryRows.held(summaryKey(entry as never)) as { failed?: boolean; text?: string | null; why?: unknown } | undefined;
    const failed = held !== undefined && held.failed === true;
    if (held === undefined) {
      entry.summaryState = "waiting";
      entry.said.textContent = SUMMARY_WAITING;
    } else if (failed) {
      entry.summaryState = "failed";
      entry.said.textContent = "";
    } else if (held.text === null) {
      entry.summaryState = "none";
      entry.said.textContent = "";
    } else {
      entry.summaryState = "ready";
      entry.said.textContent = held.text as string;
    }
    entry.mark.textContent = entry.summaryState === "failed" ? SUMMARY_FAILED_MARK : SUMMARY_MARK;
    entry.li.setAttribute("data-summary-failed", failed && summaryRows.mode() ? "true" : "false");
    entry.li.setAttribute("title", failed && summaryRows.mode() ? String(held.why || "") : "");
  }

  /** Whether a summaries redraw is already asked for in the next frame. */
  let summariesDue = false;

  /**
   * `#1210 ui-stability-no-async-shift`. Answers land one fetch at a time, each asking for a
   * redraw: unbatched, ten answers in one frame are ten measure-mutate-correct passes with
   * paints between them, and the reader sees every intermediate height. Asked for here instead,
   * they are one redraw in the next frame — one preservingScroll, one correction — however many
   * landed. In this column, captured now, as the below-count recount is. Where there are no
   * frames to wait for, the redraw runs at once.
   */
  function scheduleRenderSummaries(): void {
    if (summariesDue) return;
    if (typeof requestAnimationFrame !== "function") {
      renderSummaries();
      return;
    }
    summariesDue = true;
    const again = deps.inThisColumn(() => {
      summariesDue = false;
      renderSummaries();
    });
    requestAnimationFrame(() => again());
  }

  function renderSummaries(): void {
    deps.preservingScroll(() => {
      for (const entry of FOLD_LISTS.flatMap((id) => foldsOf(leftEl(id)))) {
        applySummaryState(entry);
        setFolded(entry, isFolded(entry));
      }
      const note = el("summary-note");
      const on = summaryRows.mode();
      note.hidden = !on;
      note.textContent = on ? summaryNoteWords(summaryRows.unavailable(), summaryRows.backend(), summaryRows.times()) : "";
      note.setAttribute("data-unavailable", on && summaryRows.unavailable() ? "true" : "false");
    });
    renderScrollTools();
  }

  function summaryTargets(): FoldEntry[] {
    if (!summaryRows.mode() || deps.view() !== "discord" || deps.searchLinks()) return [];
    if (summaryRows.unavailable()) return [];
    const area = el("scroll-area");
    const top = area.getBoundingClientRect().top - SUMMARY_LOOKAHEAD_PX;
    const bottom = top + (area.clientHeight || 0) + 2 * SUMMARY_LOOKAHEAD_PX;
    return foldsOf(deps.shownChannelList()).filter((entry) => {
      if (!entry.id || entry.li.hasAttribute("data-read-run")) return false;
      const box = entry.li.getBoundingClientRect();
      return box.bottom > top && box.top < bottom;
    });
  }

  function requestVisibleSummaries(limit: number = Infinity): void {
    summaryRows.ask(summaryTargets() as never, limit);
  }

  function setSummaryMode(on: boolean): void {
    const { entered, recheck } = summaryRows.setMode(on);
    if (!entered) return;
    el("summarise").setAttribute("aria-pressed", on ? "true" : "false");
    el("summarise-label").textContent = on ? SUMMARY_MODE_ON : SUMMARY_MODE_OFF;
    deps.eachColumn(renderSummaries);
    requestVisibleSummaries(recheck ? 1 : Infinity);
    if (!recheck) deps.inOtherColumn(requestVisibleSummaries);
  }

  function freshScrollPlace(): ScrollPlace {
    return { jumpWanted: false, inScrollback: false };
  }

  function scrollPlace(): ScrollPlace {
    return deps.activePlace();
  }

  const jumpNewestWanted: { voice: boolean | number; discord: boolean | number } = {
    voice: false,
    get discord(): boolean | number { return scrollPlace().jumpWanted; },
    set discord(value: boolean | number) { scrollPlace().jumpWanted = value; },
  };

  function setJumpNewest(wanted: boolean, view: string = deps.view(), arrived: number | null = 0): void {
    const held = (jumpNewestWanted as Record<string, boolean | number>)[view] as boolean | number;
    if (!wanted) (jumpNewestWanted as Record<string, boolean | number>)[view] = false;
    else if (arrived === null || held === true) (jumpNewestWanted as Record<string, boolean | number>)[view] = true;
    else (jumpNewestWanted as Record<string, boolean | number>)[view] = (typeof held === "number" ? held : 0) + (arrived || 0);
    renderScrollTools();
  }

  const inScrollback: { voice: boolean; discord: boolean } = {
    voice: false,
    get discord(): boolean { return scrollPlace().inScrollback; },
    set discord(value: boolean) { scrollPlace().inScrollback = value; },
  };

  const JUMP_COUNT_MAX = 99;

  /**
   * `#1057 newest-count-chip`. The rows below the reading position in the list this column's jump
   * is over, as last counted: which list (its column, view and context), and the count — null for
   * a list with no rows to count, as the call's transcript, whose turns are no pane's rows, and
   * the Threads cards in the main column. The count is of the pane's rows, the rows the view
   * draws: a reply drawn as its own row in All counts, one folded under its root in Main does
   * not, and a collapsed run of read messages is the one line it stands on screen as.
   */
  let belowHeld: { key: string; count: number | null } | null = null;
  /** Whether a recount of the rows below is already asked for in the next frame. */
  let belowDue = false;
  /** Whether the jump is being drawn again by that recount, which asks for no further one. */
  let belowFresh = false;

  function belowPane(): FoldPane {
    return deps.side() ? deps.sidePane() : deps.channelPane();
  }

  function belowKey(): string {
    return `${deps.activeName()}|${deps.view()}|${belowPane().context()}`;
  }

  function recountBelow(key: string): number | null {
    const pane = belowPane();
    const rows = deps.paneRows(pane);
    if (rows.length === 0) {
      belowHeld = { key, count: null };
      return null;
    }
    // The list window's own edges, not the room clear of what floats over it: a status line
    // coming and going, or the chip row's height moving as its own words change, would move the
    // count under a reader who never moved.
    const area = pane.scroller();
    const head = area.getBoundingClientRect().top;
    const count = rowsBelowReadingPosition(rows, head, head + (area.clientHeight || 0));
    belowHeld = { key, count };
    return count;
  }

  /**
   * Count the rows below again in the next frame, and draw the jump again where the count moved:
   * scroll events arrive oftener than frames, and one count a frame is all a changing number can
   * show. In this column, captured now — a frame later, the page works on another.
   */
  function scheduleBelow(): void {
    if (belowDue) return;
    belowDue = true;
    const again = deps.inThisColumn(() => {
      belowDue = false;
      const before = belowHeld === null ? null : belowHeld.count;
      if (recountBelow(belowKey()) === before) return;
      belowFresh = true;
      try {
        renderJumpNewest();
      } finally {
        belowFresh = false;
      }
    });
    requestAnimationFrame(() => again());
  }

  /**
   * The rows below the reading position for the jump's label: counted now where the list is not
   * the one last counted, held between frames where it is — and, where there are no frames to
   * wait for, counted now there too.
   */
  function belowForJump(): number | null {
    const key = belowKey();
    if (belowHeld === null || belowHeld.key !== key) return recountBelow(key);
    if (belowFresh) return belowHeld.count;
    if (typeof requestAnimationFrame !== "function") return recountBelow(key);
    scheduleBelow();
    return belowHeld.count;
  }

  function renderJumpNewest(): void {
    const button = el("jump-newest");
    let shown = false;
    let unsent: { state: string } | null = null;
    if (deps.screen() === "main") {
      const area = el("scroll-area");
      if (deps.side()) deps.fitOverscroll(area);
      const gap = deps.footGap(area);
      if (gap <= deps.bottomSlackPx) {
        (inScrollback as Record<string, boolean>)[deps.view()] = false;
        (jumpNewestWanted as Record<string, boolean | number>)[deps.view()] = false;
        if ((area.clientHeight || 0) > 0 && deps.view() === "discord") deps.unseenReplies[deps.activeName()] = null;
      } else if (gap > SCROLLBACK_PX) {
        (inScrollback as Record<string, boolean>)[deps.view()] = true;
      }
      shown = !deps.listEmptied() && (Boolean((inScrollback as Record<string, boolean>)[deps.view()]) || (jumpNewestWanted as Record<string, boolean | number>)[deps.view()] !== false);
      if (deps.view() === "discord") unsent = deps.unseenReply();
      if (unsent !== null && !shown) shown = !deps.listEmptied();
    }
    const arrived = (jumpNewestWanted as Record<string, boolean | number>)[deps.view()] as boolean | number;
    const count = typeof arrived === "number" && arrived > 0 ? arrived : 0;
    const many = count > JUMP_COUNT_MAX ? `${JUMP_COUNT_MAX}+` : String(count);
    const failed = unsent !== null && unsent.state === "failed";
    const draft = unsent === null && shown && deps.draftBelowView();
    // `#1057 newest-count-chip`. The offer to go to the newest says how much stands below the
    // reader — "Newest (11)", arrivals first where there are any ("3 new (9)") — on the one
    // control, after the words it already said. An arrival the page could not count (`arrived`
    // true: more arrived than one read holds, `#207 scrollback-jump`) leaves the held rows a
    // floor, said as "New (6+)". A reply of the reader's own or a draft at the foot is another
    // offer, and says neither.
    const below = shown && unsent === null && !draft ? belowForJump() : null;
    const suffix = below !== null && below > 0 ? ` (${below}${arrived === true ? "+" : ""})` : "";
    const belowWords = below !== null && below > 0 ? `${arrived === true ? "at least " : ""}${below} below` : null;
    const label = unsent !== null ? (failed ? "Reply not sent" : "Reply unconfirmed")
      : draft ? "Draft to send" : arrived === false ? `Newest${suffix}` : count ? `${many} new${suffix}` : `New${suffix}`;
    const name = unsent !== null
      ? `Jump to your reply, which ${failed ? "was not sent" : "may not have been posted"}, at the foot of the list`
      : draft
        ? "Jump to the draft waiting to be sent"
        : arrived === false
          ? belowWords === null ? "Jump to the newest message" : `Jump to the newest message, ${belowWords}`
          : `Jump to the newest message, ${count ? `${many} new` : belowWords === null ? "something new below" : "something new"}${belowWords === null ? "" : `, ${belowWords}`}`;
    if (button.hidden === shown) button.hidden = !shown;
    if (el("jump-newest-label").textContent !== label) el("jump-newest-label").textContent = label;
    if (button.getAttribute("aria-label") !== name) button.setAttribute("aria-label", name);
    if (button.hasAttribute("data-arrivals") === (arrived === false)) {
      button.toggleAttribute("data-arrivals", arrived !== false);
    }
    if (button.hasAttribute("data-unsent") !== (unsent !== null)) button.toggleAttribute("data-unsent", unsent !== null);
    deps.renderJumpBookmark();
    deps.refreshReadingJump();
    renderJumpUnread();
  }

  /**
   * `#1167 jump-to-unread-chip`, held as `#1057` holds its count: the newest unread row above
   * the screen, as last measured — which list, scroll position and rows it was measured
   * for, and the row's index and id there. A redraw that changed none of that reads no
   * row's layout again, and no frame is ever asked for: the measure is lazy, made once in
   * the pass that first needs it, and a hidden page is not measured at all (`#226`).
   */
  let unreadHeld: { key: string; index: number; id: string | null } | null = null;

  function unreadKey(rows: HTMLElement[], pane: FoldPane): string {
    const area = pane.scroller();
    return `${deps.activeName()}|${deps.view()}|${pane.context()}|${area.scrollTop}|${area.clientHeight}|${rows.map((row) => `${row.getAttribute("data-id") || ""}:${deps.rowIsRead(row) ? 1 : 0}:${row.hidden ? 1 : 0}:${row.getAttribute("data-collapsed") || ""}:${row.getAttribute("class") || ""}`).join(",")}`;
  }

  /**
   * Measure the newest unread row above the screen, or null: the offer is only for a reader
   * whose whole screen is already read, with unread waiting above it. Row tops do not
   * decrease, so a binary search finds the first row still on screen and only the rows on
   * screen are measured after it; the rows above are asked only whether they are read,
   * which is no layout at all. Every rectangle is read before the chip is written.
   */
  function measureUnreadAbove(rows: HTMLElement[], pane: FoldPane): HTMLElement | null {
    const head = pane.head();
    const foot = pane.foot();
    let lo = 0;
    let hi = rows.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if ((rows[mid] as HTMLElement).getBoundingClientRect().bottom > head) hi = mid;
      else lo = mid + 1;
    }
    if (lo >= rows.length) return null;
    let firstVisible = -1;
    for (let at = lo; at < rows.length; at += 1) {
      const box = (rows[at] as HTMLElement).getBoundingClientRect();
      if (box.top >= foot) break;
      if (box.bottom <= head) continue;
      if (firstVisible < 0) firstVisible = at;
      if (!deps.rowIsRead(rows[at] as HTMLElement)) return null;
    }
    if (firstVisible < 0) return null;
    for (let at = firstVisible - 1; at >= 0; at -= 1) {
      if (!deps.rowIsRead(rows[at] as HTMLElement)) return rows[at] as HTMLElement;
    }
    return null;
  }

  function holdUnread(rows: HTMLElement[], pane: FoldPane, target: HTMLElement | null): void {
    unreadHeld = {
      key: unreadKey(rows, pane),
      index: target ? rows.indexOf(target) : -1,
      id: target ? target.getAttribute("data-id") || null : null,
    };
  }

  function heldUnread(rows: HTMLElement[]): HTMLElement | null {
    if (!unreadHeld || unreadHeld.index < 0) return null;
    const row = rows[unreadHeld.index] as HTMLElement | undefined;
    return row && (row.getAttribute("data-id") || null) === unreadHeld.id ? row : null;
  }

  function unreadAboveTarget(): HTMLElement | null {
    if (deps.screen() !== "main" || deps.view() !== "discord" || deps.listEmptied()) return null;
    const pane = deps.channelPane();
    const rows = deps.paneRows(pane);
    if (rows.length === 0) return null;
    // A hidden page is laid out for nobody: the held answer stands until it is shown
    // again, when the next pass measures where the reader now is (`#226`).
    if (document.hidden) return heldUnread(rows);
    if (unreadHeld && unreadHeld.key === unreadKey(rows, pane)) {
      const held = heldUnread(rows);
      if (held || unreadHeld.index < 0) return held;
    }
    const target = measureUnreadAbove(rows, pane);
    holdUnread(rows, pane, target);
    return target;
  }

  function renderJumpUnread(): void {
    const chip = el("jump-unread");
    const shown = unreadAboveTarget() !== null;
    if (chip.hidden === shown) chip.hidden = !shown;
    const name = "Jump to the newest unread message above";
    if (chip.getAttribute("aria-label") !== name) chip.setAttribute("aria-label", name);
  }

  function jumpToUnread(): void {
    const target = (() => {
      if (deps.screen() !== "main" || deps.view() !== "discord" || deps.listEmptied()) return null;
      const pane = deps.channelPane();
      const rows = deps.paneRows(pane);
      if (rows.length === 0) return null;
      const measured = measureUnreadAbove(rows, pane);
      holdUnread(rows, pane, measured);
      return measured;
    })();
    if (!target) {
      renderJumpUnread();
      return;
    }
    deps.readerActed();
    const pane = deps.channelPane();
    deps.takeRow(pane, target, true);
    deps.bringIntoView(pane, target);
    renderScrollTools();
  }

  function visibleFolds(): FoldEntry[] {
    if (deps.searchLinks()) return [];
    const list = deps.side() && deps.sideMode() !== "thread" ? leftEl("side-cards") : deps.visibleList();
    return foldsOf(list).filter((entry) => !entry.li.hasAttribute("data-read-run"));
  }

  function freshScrollTools(): ScrollToolsPass {
    return { held: 0, owed: false };
  }

  function scrollTools(): ScrollToolsPass {
    return deps.activeTools();
  }

  function holdingScrollTools<T>(draw: () => T): T {
    scrollTools().held += 1;
    try {
      return draw();
    } finally {
      scrollTools().held -= 1;
      if (scrollTools().held === 0 && scrollTools().owed) renderScrollTools();
    }
  }

  function renderScrollTools(): void {
    if (scrollTools().held > 0) {
      scrollTools().owed = true;
      return;
    }
    scrollTools().owed = false;
    // `#1236 documents-reader-layout`. The fold items act on a message list —
    // the channel's, or the call transcript's — and the Documents view shows
    // neither, so none of them is offered there, whichever column this pass
    // is for. `showView` runs this pass on every switch, which is what takes
    // them away entering Documents and offers them again leaving it; the
    // menu's other channel-only items are gated the same way in
    // `renderControlBar` (`reader-toggle`, `autoplay-toggle`), and
    // `summarise` below is already the channel's alone.
    const inDocs = deps.view() === "docs";
    if (deps.side()) {
      if (inDocs) {
        el("collapse-all").hidden = true;
        el("expand-all").hidden = true;
        el("summarise").hidden = true;
        if (deps.actionMenuOpen()) deps.renderActionMenu();
      }
      renderJumpUnread();
      deps.applySearch();
      deps.placeBookmarkLine();
      renderJumpNewest();
      deps.keepSelection();
      return;
    }
    const folds = visibleFolds();
    el("collapse-all").hidden = inDocs || !folds.some((entry) => !isFolded(entry));
    el("expand-all").hidden = inDocs || !folds.some((entry) => isFolded(entry));
    el("summarise").hidden =
      deps.view() !== "discord" || !folds.some((entry) => entry.id !== null);
    (el("summarise") as HTMLButtonElement).disabled = summaryRows.disabledByServer() !== "";
    el("summarise").title = summaryRows.disabledByServer()
      ? `Summaries are unavailable on this server: ${summaryRows.disabledByServer()}`
      : "Show a one-line summary in place of each long message.";
    el("back-to-reply").hidden = deps.view() !== "discord" || deps.replyReturnRow() === null;
    if (deps.actionMenuOpen()) deps.renderActionMenu();
    deps.renderSendUndo();
    deps.renderBookmarkUndo();
    // The unread chip is measured here, before the search pass below measures the reply
    // arrows and writes them: every rectangle of this redraw is read before any of its
    // arrow writes, and the pass in `renderJumpNewest` below then finds the chip held.
    renderJumpUnread();
    deps.applySearch();
    deps.placeBookmarkLine();
    renderJumpNewest();
    deps.keepSelection();
  }

  function setAllFolded(folded: boolean): void {
    deps.readerActed();
    deps.preservingScroll(() => {
      for (const entry of visibleFolds()) setFolded(entry, folded);
    });
    if (deps.channelOnScreen()) foldSteps.set(foldStepKey(), folded ? "collapse" : "expand");
    renderScrollTools();
  }

  const FOLD_STEPS: readonly string[] = ["expand", "unread", "collapse"];
  const FOLD_STEP_WORDS: Record<string, string> = {
    expand: "Long messages: all expanded", unread: "Long messages: unread expanded, read collapsed",
    collapse: "Long messages: all collapsed",
  };
  const FOLD_STEP_NONE = "No long messages here to expand or collapse.";
  const foldSteps = new Map<string, string>();

  function foldStepKey(): string {
    const pane = deps.side() ? deps.sidePane() : deps.channelPane();
    return `${pane.name}|${pane.context()}`;
  }

  const openAtStep = (step: string, entry: FoldEntry): boolean =>
    step === "expand" || (step === "unread" && !deps.rowIsRead(entry.li));
  const stepChangesAny = (step: string, folds: FoldEntry[]): boolean =>
    folds.some((entry) => isFolded(entry) === openAtStep(step, entry));

  function nextFoldStep(from: string | null, way: number, folds: FoldEntry[]): string {
    const after = (step: string): string => FOLD_STEPS[(FOLD_STEPS.indexOf(step) + way + FOLD_STEPS.length) % FOLD_STEPS.length] as string;
    if (from !== null) return after(from);
    let step = way > 0 ? "expand" : "collapse";
    for (let tried = 1; tried < FOLD_STEPS.length && !stepChangesAny(step, folds); tried += 1) step = after(step);
    return step;
  }

  function foldStepAnchor(): { row: HTMLElement } | null {
    const pane = deps.side() ? deps.sidePane() : deps.channelPane();
    const chosen = deps.markedRow(pane, deps.markOf(pane));
    const row = chosen && deps.rowInView(pane, chosen) ? chosen : deps.paneRows(pane).find((one) => deps.rowInView(pane, one));
    return row ? deps.anchorAt(row) : null;
  }

  function foldStepField(target: EventTarget | null | undefined): HTMLElement | null {
    const node = (target || null) as HTMLElement | null;
    const pane = deps.side() ? deps.sidePane() : deps.channelPane();
    const area = pane.scroller();
    if (!node || !deps.isTextField(node) || !deps.nodeWithin(area, node)) return null;
    const box = node.getBoundingClientRect();
    const foot = area.getBoundingClientRect().top + (area.clientHeight || 0);
    return box.bottom > pane.head() && box.top < foot ? node : null;
  }

  function applyFoldStep(step: string, folds: FoldEntry[], field: HTMLElement | null = null): void {
    deps.readerActed();
    if (field) {
      deps.entryState().landing = null;
      deps.entryState().threadOwed = null;
      deps.entryState().threadAwaitsRead = false;
      const area = (deps.side() ? deps.sidePane() : deps.channelPane()).scroller();
      const top = field.getBoundingClientRect().top;
      for (const entry of folds) setFolded(entry, !openAtStep(step, entry));
      area.scrollTop += field.getBoundingClientRect().top - top;
      renderScrollTools();
      return;
    }
    const anchor = foldStepAnchor();
    for (const entry of folds) setFolded(entry, !openAtStep(step, entry));
    if (anchor) {
      deps.placeFilterAnchor(anchor);
      const row = anchor.row;
      const head = (deps.side() ? deps.sidePane() : deps.channelPane()).head();
      const hold = deps.filterHold();
      if (hold !== null && hold.row === row && row.getBoundingClientRect().bottom <= head) {
        const area = el("scroll-area");
        area.scrollTop += row.getBoundingClientRect().top - head;
        deps.setFilterHold({
          ...hold, at: area.scrollTop,
          offset: row.getBoundingClientRect().top - deps.listPlaceTop(area),
        });
      }
    }
    renderScrollTools();
  }

  function foldStepFromKeyboard(way: number, event?: { repeat?: boolean }, target: EventTarget | null = null): boolean {
    if (!deps.channelOnScreen()) return false;
    if (event && event.repeat) return deps.modHeld(event);
    const folds = visibleFolds();
    if (folds.length === 0) {
      deps.setStatus(FOLD_STEP_NONE);
      return true;
    }
    const key = foldStepKey();
    const step = nextFoldStep(foldSteps.get(key) || null, way, folds);
    applyFoldStep(step, folds, foldStepField(target));
    foldSteps.set(key, step);
    deps.setStatus(FOLD_STEP_WORDS[step] as string);
    return true;
  }

  return {
    collapsedLines, summaryRows, expandedIds, jumpNewestWanted, inScrollback,
    foldsOf, setFolded, isFolded, foldable, applySummarizeCollapsed, storedSummarizeCollapsed,
    summariesSettingLanded, renderSummaries, requestVisibleSummaries, setSummaryMode,
    freshScrollPlace, freshScrollTools, setJumpNewest, renderJumpNewest, renderJumpUnread, jumpToUnread, visibleFolds,
    holdingScrollTools, renderScrollTools, setAllFolded, foldStepFromKeyboard,
  };
}

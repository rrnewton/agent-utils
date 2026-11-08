// The reading bookmark's line as the DOM draws it (`#420 bookmark-clear`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): made once on its row and kept, as a collapsed run's
// line is, and moved to the row the bookmark stands on. Where that is, is present/rows' `bookmarkSpot`;
// what the X does, the page's.

import type { Message } from "../../contract/index.ts";
import { childByClass } from "../dom/index.ts";

/** A row as the bookmark's line reads and writes it: the element, and the lines it keeps on itself. */
export type BookmarkedRow = HTMLElement & { bookmarkLine?: HTMLElement | null; seamLine?: Node | null };

/** Where the line stands: on `row`, above it or below it. */
export interface BookmarkPlace {
  readonly row: BookmarkedRow;
  readonly place: string;
}

/** What the line asks of the page. */
export interface BookmarkLinesDependencies {
  /** The X's name and title. */
  readonly clearName: string;
  /**
   * The listener for a press of `clear`, the X on `row`'s line, made as the line is made (so it belongs
   * to the column the row was drawn in): take the bookmark away.
   */
  clearListener(row: BookmarkedRow, clear: HTMLElement): (event: Event) => void;
}

/** The bookmark's line: made on a row, and drawn where it stands. */
export interface BookmarkLines {
  /** The line on `row`, made once and kept on it. */
  bookmarkLineOf(row: BookmarkedRow): HTMLElement;
  /** The line taken off every row of `drawn` but `target`'s, and put on `target`'s row, as it says. */
  drawBookmarkLine(drawn: readonly BookmarkedRow[], target: BookmarkPlace | null): void;
}

/** The bookmark's line, its X handed to the page. */
export function createBookmarkLines(deps: BookmarkLinesDependencies): BookmarkLines {
  /**
   * The line on `row`, made once and kept on it, as a collapsed run's is (`readRunLine`): the word, a
   * separator to a screen reader, and the X at the line's end that takes the bookmark away (`#420
   * bookmark-clear`). The X is a sibling of the word, never inside a separator, whose children a
   * screen reader does not reach. It acts in the column the row was drawn in.
   */
  const bookmarkLineOf = (row: BookmarkedRow): HTMLElement => {
    if (row.bookmarkLine) return row.bookmarkLine;
    const line = document.createElement("div");
    line.className = "bookmark-line";
    const word = document.createElement("span");
    word.className = "bookmark-word";
    word.setAttribute("role", "separator");
    word.setAttribute("aria-label", "Bookmark");
    word.textContent = "Bookmark";
    const clear = document.createElement("button");
    clear.className = "bookmark-clear";
    clear.setAttribute("type", "button");
    clear.setAttribute("aria-label", deps.clearName);
    clear.setAttribute("title", deps.clearName);
    const mark = document.createElement("span");
    mark.className = "bookmark-clear-mark";
    mark.setAttribute("aria-hidden", "true");
    clear.append(mark);
    clear.addEventListener("click", deps.clearListener(row, clear));
    line.append(word, clear);
    row.bookmarkLine = line;
    return line;
  };

  /** The row's ⋯ item says what it will do: take the line away on the row right under it. */
  const word = (row: BookmarkedRow, above: boolean): void => {
    const item = childByClass(row, "row-bookmark-button");
    const said = above ? "Remove bookmark" : "Bookmark above";
    if (item && item.textContent !== said) item.textContent = said;
  };

  const drawBookmarkLine = (drawn: readonly BookmarkedRow[], target: BookmarkPlace | null): void => {
    for (const row of drawn) {
      if (target && row === target.row) continue;
      row.removeAttribute("data-bookmark");
      if (row.bookmarkLine && row.bookmarkLine.parentNode === row) row.removeChild(row.bookmarkLine);
      word(row, false);
    }
    if (!target) return;
    target.row.setAttribute("data-bookmark", target.place);
    word(target.row, target.place === "above");
    const line = bookmarkLineOf(target.row);
    if (line.parentNode !== target.row) {
      const row = target.row;
      const first = row.firstChild;
      // After a line between rows drawn above it (`#250 date-separators`): the bookmark is the nearer.
      // Put in, not the row's children written again, which would take them all out of the document
      // and back and lose the focus or the selection in any of them (see `placeSeam`).
      row.insertBefore(line, first !== null && first === row.seamLine ? first.nextSibling : first);
    }
  };

  return { bookmarkLineOf, drawBookmarkLine };
}

// The reading bookmark itself (`#240 reading-bookmark`, `#420 bookmark-clear`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): each channel's line
// above one message, kept on the server, its chip, its slides on a Done, and its Undo. The
// held state and writes are core/annotations'; where the line goes is present/rows'.

import {
  anchorOfMessage, createBookmarkUndo, createBookmarkWrites, createBookmarks,
  createLegacyMarker, isBookmarkState, sameAnchor,
} from "../../core/annotations/index.ts";
import { bookmarkSpot } from "../../present/rows/index.ts";
import { idsOf, rowMessages, rowNewestMs, rowSentMs, rowShown } from "./rows.ts";

/** A bookmark anchor, as the server holds it. */
export interface BookmarkAnchorLike {
  message_id?: unknown;
  thread_id?: unknown;
  timestamp?: unknown;
  place?: string;
}

/** What the reading bookmark asks of the page. */
export interface ReadingBookmarkDependencies {
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  bumpEpoch(): void;
  deviceStorage(): unknown;
  queueWrite(write: () => Promise<unknown>): unknown;
  putBookmark(channel: string, body: unknown): Promise<unknown>;
  removeBookmark(channel: string, revision: unknown): Promise<unknown>;
  isBookmarkAnswer(body: unknown): boolean;
  currentChannelId(): string | null;
  eachColumn(fn: () => void): void;
  renderScrollTools(): void;
  side(): boolean;
  activeName(): string;
  inColumn<T>(column: unknown, fn: () => T): T;
  mainColumn(): unknown;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  sentEpochOf(payload: unknown): number;
  saveChannelScope(): void;
  setStatus(text: string): void;
  showError(text: string): void;
  pinsOnly(): boolean;
  readRunsSuspended(): boolean;
  threading(): boolean;
  selectionView(): string;
  selectionThreadId(): unknown;
  hidingRead(): boolean;
  todoHoldsFrom(at: number, channel: string | null): boolean;
  fromGatheredThread(row: Element): boolean;
  preservingScroll(fn: () => void): void;
  screen(): string;
  view(): string;
  paneHead(): number;
  takeRow(pane: unknown, row: HTMLElement, ring: boolean): void;
  channelPane(): unknown;
  sidePane(): unknown;
  replyLandingGapPx(): number;
  threadListOnScreen(): boolean;
  readerActed(): void;
  seekMessage(id: string, land: () => boolean, miss: () => boolean): Promise<void>;
  guardQuietly<T>(fn: () => T): (...args: unknown[]) => Promise<unknown>;
  channelOnScreen(): boolean;
  keyContext(target: unknown): { row: HTMLElement | null };
  rowForAct(target: unknown): { pane: unknown; row: HTMLElement | null; took: boolean };
  drawnBy(): boolean;
  anchorOfRow(row: HTMLElement, place: string): BookmarkAnchorLike | null;
  nextUndoAt(): number;
  stillToDo(message: Message, done?: Set<string>): boolean;
  archived(): Set<string>;
  dismissed(): Set<string>;
  heldMessages(): Message[];
  heldViews(): Map<string, { more: boolean; floor: number | null }>;
  viewKey(view: string): string;
  inView(message: Message, view: string, thread: string | null): boolean;
  threadOf(message: unknown): string | null;
  timeOf(message: Message, field: string): number;
  currentChannel(): string;
  activeName(): string;
  rowHolding(id: string): Element | null;
  clock(): unknown;
}

/** The reading bookmark's commands and questions. */
export interface ReadingBookmark {
  heldBookmark(channel?: string | null): { anchor: BookmarkAnchorLike | null; known?: boolean } | null;
  bookmarkAnchor(channel?: string | null): BookmarkAnchorLike | null;
  forgetBookmarks(): void;
  noteBookmark(payload: unknown): void;
  bookmarkToSave(channel: string): unknown;
  restoreSavedBookmark(channel: string, saved: unknown): void;
  stageBookmark(channel: string, anchor: BookmarkAnchorLike | null): unknown;
  sendBookmark(move: unknown, options?: unknown): Promise<string>;
  placeBookmarkLine(): void;
  renderJumpBookmark(): void;
  toggleBookmarkAt(row: HTMLElement | null): void;
  bookmarkFromKeyboard(target: unknown): boolean;
  bookmarkJumpFromKeyboard(): boolean;
  singleDoneBump(ids: string[]): { channel: string; from: unknown; to: unknown } | null;
  throughBump(row: Element): { channel: string; from: unknown; to: unknown } | null;
  undoBookmarkBump(bump: unknown, restoring: Promise<unknown>): Promise<void>;
  jumpToBookmark(): Promise<void>;
  removeBookmark(channel: string | null): void;
  undoBookmarkRemoval(): void;
  renderBookmarkUndo(): void;
  endBookmarkUndo(): void;
  bookmarkAbove(row: HTMLElement): boolean;
  bookmarkUndosCurrent(): { channel: string } | null;
  forgetDrawn(): void;
}

const BOOKMARK_CLEAR_NAME = "Remove bookmark";
const NO_BOOKMARK_IN_THREADS = "No bookmarks in thread view.";

/** No bookmark held, no line drawn. */
export function createReadingBookmark(deps: ReadingBookmarkDependencies): ReadingBookmark {
  const el = deps.byId;
  const leftEl = deps.leftById;

  const bookmarkStore = createBookmarks({ bumpEpoch: () => deps.bumpEpoch() } as never) as {
    held(channel: string): { anchor: BookmarkAnchorLike | null; known?: boolean } | null;
    forget(): void;
    note(channel: string, bookmark: unknown, epoch: number): boolean;
    toSave(channel: string): unknown;
    restoreSaved(channel: string, saved: unknown): void;
    stage(channel: string, anchor: BookmarkAnchorLike | null): unknown;
    writing(channel: string): boolean;
  };
  const bookmarkDrawn: { main: { row: BookmarkedRow; place: string } | null; side: { row: BookmarkedRow; place: string } | null } = { main: null, side: null };
  const legacyMarker = createLegacyMarker({ storage: deps.deviceStorage() } as never) as {
    id(): string | null; noteRead(): void; exhausted(): boolean; drop(): void;
  };

  function heldBookmark(channel: string | null = deps.currentChannelId()): { anchor: BookmarkAnchorLike | null; known?: boolean } | null {
    return bookmarkStore.held(String(channel));
  }

  function bookmarkAnchor(channel: string | null = deps.currentChannelId()): BookmarkAnchorLike | null {
    const held = heldBookmark(channel);
    return held ? held.anchor : null;
  }

  function forgetBookmarks(): void {
    bookmarkStore.forget();
    bookmarkDrawn.main = null;
    bookmarkDrawn.side = null;
    endBookmarkUndo();
  }

  function redrawBookmark(): void {
    deps.eachColumn(deps.renderScrollTools);
  }

  function noteBookmark(payload: unknown): void {
    const held = payload as { bookmark?: unknown; channel?: { id?: unknown } } | null;
    if (!held || !(isBookmarkState as (value: unknown) => boolean)(held.bookmark)) return;
    legacyMarker.noteRead();
    const channel = String((held.channel && held.channel.id) || deps.currentChannelId());
    const moved = bookmarkStore.note(channel, held.bookmark, deps.sentEpochOf(payload));
    if (moved && channel === deps.currentChannelId()) redrawBookmark();
  }

  function bookmarkToSave(channel: string): unknown {
    return bookmarkStore.toSave(channel);
  }

  function restoreSavedBookmark(channel: string, saved: unknown): void {
    bookmarkStore.restoreSaved(channel, saved);
  }

  function stageBookmark(channel: string, anchor: BookmarkAnchorLike | null): unknown {
    return bookmarkStore.stage(channel, anchor);
  }

  const bookmarkWrites = createBookmarkWrites({
    bookmarks: bookmarkStore,
    queue: (write: () => Promise<unknown>) => deps.queueWrite(write),
    put: (channel: string, body: unknown) => deps.putBookmark(channel, body),
    remove: (channel: string, revision: unknown) => deps.removeBookmark(channel, revision),
    isAnswer: (body: unknown) => deps.isBookmarkAnswer(body),
    channel: () => deps.currentChannelId(),
    anchor: (channel: string) => bookmarkAnchor(channel),
    same: (one: unknown, other: unknown) => (sameAnchor as (a: unknown, b: unknown) => boolean)(one, other),
    redraw: () => redrawBookmark(),
    save: () => { if (!deps.side()) deps.saveChannelScope(); },
    status: (text: string) => deps.setStatus(text),
    error: (text: string) => deps.showError(text),
  } as never) as { send(move: unknown, options?: unknown): Promise<string> };

  function sendBookmark(move: unknown, options: unknown = {}): Promise<string> {
    return bookmarkWrites.send(move, options);
  }

  function bookmarkListRefusal(anchor: BookmarkAnchorLike): string {
    if (deps.pinsOnly()) return "The bookmark is not in Pinned.";
    if (deps.readRunsSuspended()) return "Close the search to see the bookmark.";
    if (!deps.threading()) return "";
    if (deps.selectionView() === "main" || deps.selectionView() === "flat") return "";
    if (deps.selectionView() === "thread") {
      return anchor.thread_id && String(anchor.thread_id) === String(deps.selectionThreadId())
        ? "" : "Your bookmark is not in this thread.";
    }
    return "The bookmark is in Main and All.";
  }

  function todoWindowHolds(at: number): boolean {
    return deps.hidingRead() && !deps.threading() && deps.todoHoldsFrom(at, deps.currentChannelId());
  }

  function bookmarkTarget(): { row: BookmarkedRow; place: string } | null {
    const anchor = bookmarkAnchor();
    if (!anchor || bookmarkListRefusal(anchor) !== "") return null;
    const rows = ([...el("discord-log").children] as BookmarkedRow[])
      .filter((row) => row.hasAttribute("data-id") && !row.hasAttribute("data-card") && !deps.fromGatheredThread(row));
    const spot = bookmarkSpot(rows.map((row) => ({
      ids: idsOf(row), sentMs: rowSentMs(row), newestMs: rowNewestMs(row), shown: rowShown(row),
    })) as never, anchor as never, todoWindowHolds as never) as { index: number; place: string } | null;
    return spot ? { row: rows[spot.index] as BookmarkedRow, place: spot.place } : null;
  }

  function placeBookmarkLine(): void {
    const target = bookmarkTarget();
    const list = el("discord-log");
    const drawn = ([...list.children] as BookmarkedRow[]).filter((row) => row.hasAttribute("data-bookmark"));
    const settled = target && drawn.length === 1 && drawn[0] === target.row &&
      target.row.getAttribute("data-bookmark") === target.place && target.row.bookmarkLine &&
      target.row.bookmarkLine.parentNode === target.row;
    (bookmarkDrawn as Record<string, unknown>)[deps.activeName()] = target;
    if (!deps.side()) adoptLegacyMarker();
    if (settled || (!target && drawn.length === 0)) return;
    const apply = (): void => drawBookmarkLine(drawn, target);
    if (!deps.side() && deps.screen() === "main" && deps.view() === "discord") deps.preservingScroll(apply);
    else apply();
  }

  function dropLegacyMarker(): void {
    legacyMarker.drop();
  }

  function adoptLegacyMarker(): void {
    if (legacyMarker.id() === null) return;
    const channel = deps.currentChannelId();
    const held = heldBookmark(channel);
    if (!held || !held.known || bookmarkStore.writing(channel as string)) return;
    const row = deps.rowHolding(legacyMarker.id() as string);
    if (!row) {
      if (legacyMarker.exhausted()) dropLegacyMarker();
      return;
    }
    const id = legacyMarker.id() as string;
    dropLegacyMarker();
    if (held.anchor) return;
    const messages = rowMessages(row as HTMLElement);
    const at = Math.max(0, messages.findIndex((message) => String(message.id) === id));
    const message = messages[at];
    if (!message) return;
    const move = stageBookmark(channel as string, anchorOfMessage(message as never, "above" as never) as BookmarkAnchorLike);
    redrawBookmark();
    deps.guardQuietly(() => sendBookmark(move, { conditional: true, quiet: true }))();
  }

  function renderJumpBookmark(): void {
    if (deps.side()) return;
    const chip = leftEl("jump-bookmark");
    let direction: "up" | "down" | null = null;
    const anchor = bookmarkAnchor();
    if (deps.screen() === "main" && deps.view() === "discord" && anchor && bookmarkListRefusal(anchor) === "" &&
        !bookmarksOffScreen()) {
      const target = bookmarkDrawn.main;
      if (!target) {
        direction = [...el("discord-log").children].some((row) => row.hasAttribute("data-id")) ? "up" : null;
      } else if (target.row.bookmarkLine) {
        const area = leftEl("scroll-area");
        const box = target.row.bookmarkLine.getBoundingClientRect();
        const top = deps.paneHead();
        const bottom = area.getBoundingClientRect().top + (area.clientHeight || 0);
        if (box.bottom <= top) direction = "up";
        else if (box.top >= bottom) direction = "down";
      }
    }
    if (chip.hidden === (direction !== null)) chip.hidden = direction === null;
    if (direction === null) return;
    if (chip.getAttribute("data-direction") !== direction) {
      chip.setAttribute("data-direction", direction);
      chip.setAttribute("aria-label", `Jump to your bookmark, ${direction === "up" ? "above" : "below"}`);
    }
  }

  function landOnBookmark(): boolean {
    if (deps.side()) return deps.inColumn(deps.mainColumn(), landOnBookmark);
    const target = bookmarkDrawn.main;
    if (!target || !target.row.bookmarkLine || target.row.parentNode !== el("discord-log")) return false;
    const area = el("scroll-area");
    const line = target.row.bookmarkLine;
    area.scrollTop += line.getBoundingClientRect().top - deps.paneHead() - deps.replyLandingGapPx();
    deps.takeRow(deps.channelPane(), target.row, true);
    deps.renderScrollTools();
    deps.setStatus(target.place === "below" ? "Bookmark: below the newest message." : "Back at your bookmark.");
    return true;
  }

  function rowAmongCards(row: HTMLElement | null): boolean {
    const list = row ? row.parentNode : null;
    return Boolean(row) && (row?.hasAttribute("data-card") || list === leftEl("side-cards") || list === el("thread-list"));
  }

  const bookmarksOffScreen = (): boolean => deps.threadListOnScreen() ||
    (deps.view() === "discord" && !deps.side() && deps.threading() && deps.selectionView() === "threads");

  async function jumpToBookmark(): Promise<void> {
    if (bookmarksOffScreen()) {
      deps.setStatus(NO_BOOKMARK_IN_THREADS);
      return;
    }
    if (deps.side()) return deps.inColumn(deps.mainColumn(), jumpToBookmark);
    deps.readerActed();
    const anchor = bookmarkAnchor();
    if (!anchor) {
      deps.setStatus("No bookmark here. Shift+B sets one.");
      return;
    }
    const refusal = bookmarkListRefusal(anchor);
    if (refusal) {
      deps.setStatus(refusal);
      return;
    }
    if (landOnBookmark()) return;
    await deps.seekMessage(String(anchor.message_id), landOnBookmark, () => false);
  }

  function bookmarkJumpFromKeyboard(): boolean {
    if (bookmarksOffScreen()) {
      deps.setStatus(NO_BOOKMARK_IN_THREADS);
      return true;
    }
    if (deps.side()) return deps.inColumn(deps.mainColumn(), bookmarkJumpFromKeyboard);
    if (!deps.channelOnScreen()) return false;
    deps.guardQuietly(jumpToBookmark)();
    return true;
  }

  function bookmarkAbove(row: HTMLElement): boolean {
    const target = bookmarkTarget();
    return Boolean(target) && (target as { place: string }).place === "above" && (target as { row: BookmarkedRow }).row === row;
  }

  function toggleBookmarkAt(row: HTMLElement | null): void {
    const channel = deps.currentChannelId();
    if (!channel || !row) return;
    if (rowAmongCards(row) || bookmarksOffScreen()) {
      deps.setStatus(NO_BOOKMARK_IN_THREADS);
      return;
    }
    if (row.parentNode === el("discord-log") && bookmarkAbove(row)) {
      removeBookmark(channel);
      return;
    }
    const anchor = deps.anchorOfRow(row, "above");
    if (!anchor) return;
    const move = stageBookmark(channel, anchor);
    const undo = bookmarkUndos.current() as { channel: string } | null;
    if (undo && undo.channel === channel) endBookmarkUndo();
    redrawBookmark();
    deps.setStatus("Bookmarked. b comes back here.");
    deps.guardQuietly(() => sendBookmark(move))();
  }

  const { drawBookmarkLine } = createBookmarkLines({
    clearName: BOOKMARK_CLEAR_NAME,
    clearListener: (row: BookmarkedRow, clear: HTMLElement) => deps.inThisColumn((event: Event) => {
      if (event) event.stopPropagation();
      const focused = document.activeElement === clear;
      removeBookmark(deps.currentChannelId());
      if (focused && deps.drawnBy() && row.parentNode === el("discord-log") && rowShown(row)) {
        deps.takeRow(deps.side() ? deps.sidePane() : deps.channelPane(), row, true);
      }
    }),
  });

  const bookmarkUndos = createBookmarkUndo({ clock: deps.clock(), ended: () => renderBookmarkUndo() } as never) as {
    current(): { channel: string; anchor: BookmarkAnchorLike; token: unknown } | null;
    offer(entry: unknown): void;
    end(): void;
  };

  function removeBookmark(channel: string | null): void {
    const before = bookmarkAnchor(channel);
    if (!channel || !before) return;
    const move = stageBookmark(channel, null) as { token: unknown };
    bookmarkUndos.offer({ channel, anchor: before, token: move.token, at: deps.nextUndoAt() });
    redrawBookmark();
    renderBookmarkUndo();
    deps.setStatus("Bookmark removed. Undo puts it back.");
    deps.guardQuietly(async () => {
      const outcome = await sendBookmark(move);
      const undo = bookmarkUndos.current();
      if (outcome !== "saved" && undo && undo.token === move.token) endBookmarkUndo();
    })();
  }

  function endBookmarkUndo(): void {
    bookmarkUndos.end();
  }

  function renderBookmarkUndo(): void {
    const chip = leftEl("undo-bookmark");
    const undo = bookmarkUndos.current();
    const shown = undo !== null && deps.view() === "discord" && undo.channel === deps.currentChannelId();
    if (chip.hidden === shown) chip.hidden = !shown;
  }

  function undoBookmarkRemoval(): void {
    const undo = bookmarkUndos.current();
    if (!undo) return;
    endBookmarkUndo();
    if (bookmarkAnchor(undo.channel)) {
      deps.setStatus("A bookmark was set since; it stays.");
      return;
    }
    const move = stageBookmark(undo.channel, undo.anchor);
    redrawBookmark();
    deps.setStatus("Bookmark put back.");
    deps.guardQuietly(() => sendBookmark(move, { conditional: true }))();
  }

  function bookmarkFromKeyboard(target: unknown): boolean {
    const { row: under } = deps.keyContext(target);
    if (bookmarksOffScreen() || rowAmongCards(under)) {
      deps.setStatus(NO_BOOKMARK_IN_THREADS);
      return true;
    }
    const { pane, row, took } = deps.rowForAct(target);
    if (!pane || !row) return took;
    toggleBookmarkAt(row);
    return true;
  }

  function rowReadWith(row: HTMLElement, done: Set<string>): boolean {
    const held = rowMessages(row);
    return held.length > 0 && held.every((message) => !deps.stillToDo(message, done));
  }

  function slideTarget(rows: HTMLElement[], at: number, done: Set<string>): BookmarkAnchorLike | null {
    const next = rows.slice(at + 1).find((row) => !rowReadWith(row, done));
    return next ? deps.anchorOfRow(next, "above") : deps.anchorOfRow(rows[rows.length - 1] as HTMLElement, "below");
  }

  function mainHeldFrom(): number {
    let from = Infinity;
    for (const key of [deps.viewKey("main"), deps.viewKey("flat")]) {
      const cover = deps.heldViews().get(key);
      if (!cover) continue;
      if (!cover.more) return -Infinity;
      if (cover.floor !== null) from = Math.min(from, cover.floor);
    }
    return from;
  }

  function heldBackByMain(to: BookmarkAnchorLike | null, target: { row: BookmarkedRow }, rows: HTMLElement[], done: Set<string>): BookmarkAnchorLike | null {
    if (!to || !deps.threading() || deps.selectionView() !== "thread") return to;
    if (deps.currentChannel() !== deps.currentChannelId()) return null;
    const anchor = bookmarkAnchor();
    const times = [Date.parse(String(anchor ? anchor.timestamp : "")), rowSentMs(target.row)].filter(Number.isFinite);
    const until = Date.parse(String(to.timestamp));
    if (times.length === 0 || !Number.isFinite(until)) return null;
    const from = Math.min(...times);
    if (from < mainHeldFrom()) return null;
    const thread = String(deps.selectionThreadId());
    const mine = new Set(rows.flatMap((row) => idsOf(row)));
    const read = new Set([...deps.dismissed(), ...done]);
    const passed = (at: number): boolean => at >= from && (to.place === "below" ? at <= until : at < until);
    const stop = deps.heldMessages().find((message) => deps.inView(message, "main", null) &&
      deps.threadOf(message) !== thread && !mine.has(String(message.id)) &&
      passed(deps.timeOf(message, "timestamp")) && deps.stillToDo(message, read));
    return stop ? (anchorOfMessage as (m: unknown, place: string) => BookmarkAnchorLike)(stop, "above") : to;
  }

  function singleDoneBump(ids: string[]): { channel: string; from: unknown; to: unknown } | null {
    const channel = deps.currentChannelId();
    const target = bookmarkTarget();
    if (!channel || !target || target.place !== "above") return null;
    const marking = new Set(ids.map(String));
    const done = new Set([...deps.archived(), ...marking]);
    if (!idsOf(target.row).some((id) => marking.has(id)) || !rowReadWith(target.row, done)) return null;
    const rows = ([...el("discord-log").children] as HTMLElement[]).filter((row) => row.hasAttribute("data-id") && rowShown(row));
    const at = rows.indexOf(target.row);
    if (at < 0) return null;
    const to = heldBackByMain(slideTarget(rows, at, done), target, rows, done);
    return to ? { channel, from: bookmarkAnchor(channel), to } : null;
  }

  function throughBump(row: Element): { channel: string; from: unknown; to: unknown } | null {
    const channel = deps.currentChannelId();
    const list = el("discord-log");
    const target = bookmarkTarget();
    if (!channel || !target || row.parentNode !== list) return null;
    const rows = ([...list.children] as HTMLElement[])
      .filter((one) => one.hasAttribute("data-id") && rowShown(one) && !deps.fromGatheredThread(one));
    const at = rows.indexOf(row as HTMLElement);
    const line = target.place === "above" ? rows.indexOf(target.row) : rows.length;
    if (at < 0 || line < 0 || line > at) return null;
    const all = [...list.children];
    const done = new Set([...deps.archived(), ...all.slice(0, all.indexOf(row) + 1).flatMap((one) => idsOf(one as Element))]);
    const to = heldBackByMain(slideTarget(rows, at, done), target, rows, done);
    return to ? { channel, from: bookmarkAnchor(channel), to } : null;
  }

  async function undoBookmarkBump(bump: unknown, restoring: Promise<unknown>): Promise<void> {
    const held = bump as unknown as { channel: string; from: BookmarkAnchorLike | null; to: BookmarkAnchorLike };
    const back = (sameAnchor as (a: unknown, b: unknown) => boolean)(bookmarkAnchor(held.channel), held.to) ? stageBookmark(held.channel, held.from) : null;
    if (back) redrawBookmark();
    let restored = false;
    try {
      await restoring;
      restored = true;
    } catch (error) {
      restored = Boolean(error && (error as { landed?: number }).landed as number > 0);
      throw error;
    } finally {
      if (back) await sendBookmark(back, { conditional: true, after: Promise.resolve(restored), quiet: true });
    }
  }

  return {
    heldBookmark, bookmarkAnchor, forgetBookmarks, noteBookmark, bookmarkToSave,
    restoreSavedBookmark, stageBookmark, sendBookmark, placeBookmarkLine, renderJumpBookmark,
    toggleBookmarkAt, bookmarkFromKeyboard, bookmarkJumpFromKeyboard, singleDoneBump,
    throughBump, undoBookmarkBump, removeBookmark, undoBookmarkRemoval, renderBookmarkUndo,
    endBookmarkUndo, bookmarkAbove, jumpToBookmark, bookmarkUndosCurrent: () => bookmarkUndos.current(),
    forgetDrawn: () => { bookmarkDrawn.main = null; bookmarkDrawn.side = null; },
  };
}

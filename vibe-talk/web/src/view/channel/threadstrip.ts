// A thread's strip in All, and selecting a thread there (`#299 all-thread-strip`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): the strip on each
// row of All, the selection it makes, and Main brought up to date with its threads' replies.
// Why each behaves as it does is told where the legacy script held them. The unanswered-thread
// group stays with the page until the reply-counts move.

import type { Message } from "../../contract/index.ts";
import { threadOf } from "../../core/channels/index.ts";
import { idsOf, rowMessages } from "./rows.ts";

/** A row carrying its strip. */
type StripRow = HTMLElement & { threadStrip?: HTMLElement | null };

/** What the strip asks of the page. */
export interface ThreadStripDependencies {
  byId(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  inColumn<T>(column: unknown, fn: () => T): T;
  mainColumn(): unknown;
  side(): boolean;
  selection(): { view: string };
  selectedChannelId(): string;
  heldMessages(): Message[];
  threadHue(id: unknown): string;
  sayRowShortcut(el: Element, name: string): void;
  setStatus(text: string): void;
  threadName(choice: unknown): string;
  knownThreadChoice(id: string, summary: unknown): unknown;
  channelOnScreen(): boolean;
  layoutThreadList(): boolean;
  pinsOnly(): boolean;
  rowForAct(target: unknown): { row: HTMLElement | null; took: boolean };
  threading(): boolean;
  hidingRead(): boolean;
  readFilterKeeps(): (message: Message) => boolean;
  renderCachedTimeline(): void;
  renderChannelRows(): void;
  hiddenEmpty(message: Message): boolean;
  drawnIds(): string[];
  fromGatheredThread(row: Element): boolean;
  canonChannel(): string;
  timelineMessages(): Message[];
}

/** The strip's commands and questions. */
export interface ThreadStrip {
  allThreadSelected(): string | null;
  drawingAll(): boolean;
  markListView(): void;
  paintThreadStrip(row: HTMLElement, id: string | null): void;
  paintAllThreadFocus(): void;
  toggleAllThreadFocus(id: string): void;
  clearAllThreadFocus(): boolean;
  threadStripFromKeyboard(target: unknown): boolean;
  syncUnreadReplies(): void;
}

/** No thread selected in All. */
export function createThreadStrip(deps: ThreadStripDependencies): ThreadStrip {
  const el = deps.byId;

  const allThreadFocus = new Map<string, string>();

  function allThreadSelected(): string | null {
    return allThreadFocus.get(String(deps.selectedChannelId())) || null;
  }

  function drawingAll(): boolean {
    return !deps.side() && deps.selection().view === "flat";
  }

  function markListView(): void {
    const list = el("discord-log");
    if (list.getAttribute("data-view") !== deps.selection().view) list.setAttribute("data-view", deps.selection().view);
  }

  function paintThreadStrip(row: HTMLElement, id: string | null): void {
    const held = (row as StripRow).threadStrip || null;
    if (id === null) {
      if (held && !(held as HTMLElement).hidden) (held as HTMLElement).hidden = true;
      if (row.hasAttribute("data-thread-strip")) row.removeAttribute("data-thread-strip");
      return;
    }
    const strip = held || threadStrip(row);
    const hue = String(deps.threadHue(id));
    if (strip.style.getPropertyValue("--thread-hue") !== hue) strip.style.setProperty("--thread-hue", hue);
    if (strip.hidden) strip.hidden = false;
    if (!row.hasAttribute("data-thread-strip")) row.setAttribute("data-thread-strip", "");
  }

  function threadStrip(row: HTMLElement): HTMLElement {
    const strip = document.createElement("button");
    strip.className = "thread-strip";
    strip.setAttribute("type", "button");
    strip.setAttribute("aria-label", "Select this thread");
    strip.setAttribute("aria-pressed", "false");
    strip.setAttribute("title", "Select this thread: the messages not in it dim. Again, or Esc, lets go");
    deps.sayRowShortcut(strip, "thread-strip");
    strip.addEventListener("click", deps.inThisColumn(() => {
      const id = threadOf(rowMessages(row)[0] as never);
      if (id) toggleAllThreadFocus(id);
    }));
    const first = row.firstChild as HTMLElement | null;
    const arrow = first && String(first.className || "").split(/\s+/).includes("reply-jump") ? first : null;
    row.insertBefore(strip, arrow ? arrow.nextSibling : first);
    (row as StripRow).threadStrip = strip;
    return strip;
  }

  function paintAllThreadFocus(): void {
    if (deps.side()) return;
    const list = el("discord-log");
    const channel = String(deps.selectedChannelId());
    let id = deps.selection().view === "flat" ? allThreadSelected() : null;
    if (id !== null && deps.heldMessages().length >= 0 && deps.canonChannel() === channel && !deps.heldMessages().some((message) => threadOf(message as never) === id)) {
      allThreadFocus.delete(channel);
      id = null;
    }
    if (id === null) {
      if (list.hasAttribute("data-focus-thread")) list.removeAttribute("data-focus-thread");
    } else if (list.getAttribute("data-focus-thread") !== id) {
      list.setAttribute("data-focus-thread", id);
    }
    for (const row of [...list.children] as HTMLElement[]) {
      const held = rowMessages(row);
      const within = id !== null && held.some((message) => threadOf(message as never) === id);
      if (row.hasAttribute("data-thread-in") !== within) {
        if (within) row.setAttribute("data-thread-in", "");
        else row.removeAttribute("data-thread-in");
      }
      const strip = (row as StripRow).threadStrip as HTMLElement | null | undefined;
      if (!strip) continue;
      const pressed = id !== null && held.length > 0 && threadOf(held[0] as never) === id ? "true" : "false";
      if (strip.getAttribute("aria-pressed") !== pressed) strip.setAttribute("aria-pressed", pressed);
    }
  }

  function toggleAllThreadFocus(id: string): void {
    const channel = String(deps.selectedChannelId());
    const letting = allThreadFocus.get(channel) === id;
    if (letting) allThreadFocus.delete(channel);
    else allThreadFocus.set(channel, id);
    paintAllThreadFocus();
    if (letting) {
      deps.setStatus("Thread selection cleared.");
      return;
    }
    const rows = [...el("discord-log").children] as HTMLElement[];
    const here = rows.flatMap(idsOf).length;
    const mine = rows.filter((row) => row.hasAttribute("data-thread-in")).flatMap(idsOf).length;
    deps.setStatus(`Thread selected: ${deps.threadName(deps.knownThreadChoice(id, null))} — ${mine} of ${here} messages here. Esc clears it.`);
  }

  function clearAllThreadFocus(): boolean {
    return deps.inColumn(deps.mainColumn(), () => {
      if (!deps.channelOnScreen() || deps.layoutThreadList() || deps.pinsOnly() || deps.selection().view !== "flat") return false;
      const channel = String(deps.selectedChannelId());
      if (!allThreadFocus.has(channel)) return false;
      allThreadFocus.delete(channel);
      paintAllThreadFocus();
      deps.setStatus("Thread selection cleared.");
      return true;
    });
  }

  function threadStripFromKeyboard(target: unknown): boolean {
    const { row, took } = deps.rowForAct(target);
    const strip = row ? (row as StripRow).threadStrip : null;
    if (!strip || (strip as HTMLElement).hidden) return took;
    (strip as HTMLElement).click();
    return true;
  }

  function syncUnreadReplies(): void {
    if (!deps.threading() || deps.selection().view !== "main" || deps.canonChannel() !== String(deps.selectedChannelId())) return;
    if (deps.hidingRead()) {
      const keeps = deps.readFilterKeeps();
      const wanted = deps.timelineMessages().filter((message) => !deps.hiddenEmpty(message) && keeps(message))
        .map((message) => String(message.id)).sort().join(" ");
      const drawn = [...el("discord-log").children].filter((row) => !deps.fromGatheredThread(row)).flatMap(idsOf).sort().join(" ");
      if (wanted !== drawn) {
        deps.renderCachedTimeline();
        return;
      }
    }
    deps.renderChannelRows();
  }

  return {
    allThreadSelected, drawingAll, markListView, paintThreadStrip, paintAllThreadFocus,
    toggleAllThreadFocus, clearAllThreadFocus, threadStripFromKeyboard, syncUnreadReplies,
  };
}

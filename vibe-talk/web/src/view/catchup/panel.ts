// The Discuss panel's DOM (`catchup-side-agent`): catch-up over the channel or thread
// on screen. Opening asks nothing of the model (`discuss-summarize-button`): it reads
// the scope's unread preview from the cache and offers "Summarize last [N] unread
// since [HH:MM]" — one bound with two linked ends — and only that press POSTs. While
// a request is in flight nothing starts another. The answer's model text is set as
// text only, never as markup, and the footer — which unread messages the summary
// covers, a jump link for each, and Mark read / Mark unread over exactly the
// server's markable ids — is drawn from the structured fields, never parsed out of
// the model's words. Construction does nothing: no element is looked up and nothing
// is fetched until `mount` wires the controls and `openWith` is called.
//
// The answer's wire types are the generated contract's (`CatchupResponse` and its
// parts); the panel narrows the decoded body to the fields it reads.

import { createListening } from "../dom/index.ts";
import type { MessageLinkRef } from "../../foundation/index.ts";
import type {
  CatchupFooterLink as WireFooterLink,
  CatchupStamp as WireStamp,
  CatchupWindow as WireWindow,
} from "../../contract/index.ts";

/** What one catch-up asks for: a thread, or the channel in `view` when `thread_id` is null. */
export interface CatchupRequestBody {
  thread_id: string | null;
  view: "flat" | "main";
  /** The bound's start (Unix ms): only messages at/after it are covered. */
  since_ms: number | null;
}

/** One unread message in a preview, oldest first in its list. */
export interface CatchupUnreadRef {
  id: string;
  at_ms: number | null;
  label: string;
}

/** A scope's shape before any model is asked, narrowed to what the bound needs. */
export interface CatchupPreview {
  window: CatchupWindow | null;
  unread: CatchupUnreadRef[];
}

/** The window the server read, as far as the panel reports it. */
export type CatchupWindow = Pick<WireWindow, "count" | "unread_count">;

/** One covered message the footer links to. */
export type CatchupFooterLink = WireFooterLink;

/** The server's timing and size stamp; token counts are null unless really reported. */
export type CatchupStamp = WireStamp;

/** One catch-up answer, narrowed to the fields the panel reads. */
export interface CatchupAnswer {
  window: CatchupWindow | null;
  unread_ids: string[];
  markable_ids: string[];
  footer_links: CatchupFooterLink[];
  text: string;
  stamp: CatchupStamp | null;
}

/** Where a catch-up is aimed: the parent channel always, and a thread where one is open. */
export interface DiscussScope {
  channelId: string;
  threadId: string | null;
  view: "flat" | "main";
  label: string;
}

/** What the panel asks of the page. Every function is read on use, never at construction. */
export interface DiscussPanelDependencies {
  byId(id: string): HTMLElement;
  /** POST one catch-up for `channelId`; answers the decoded body as `unknown`. */
  catchupPost(channelId: string, body: CatchupRequestBody): Promise<unknown>;
  /** GET the scope's unread preview for `channelId`; spends no model call. */
  catchupPreview(channelId: string, scope: DiscussScope): Promise<unknown>;
  /** A request started or settled, so the page can disable its entry points too. */
  onBusy?(busy: boolean): unknown;
  /** The chat service `channelId` is read through, for a footer's jump refs. */
  serviceName(channelId: string): string;
  /** Jump to the message a footer link names. */
  onJump(ref: MessageLinkRef): unknown;
  /**
   * Mark messages read / put them back, through the page's Done acts: those own the
   * ≤1000-id batching (`writeInBatches`), so the panel hands over the whole list.
   */
  dismissMessages(body: { messages: readonly string[] }): Promise<unknown>;
  restoreMessages(ids: readonly string[]): Promise<unknown>;
  showError?(text: string): unknown;
  setStatus?(text: string): unknown;
  /**
   * `#1344 discuss-panel-resize`. The layout choices kept on this device, handed in where
   * the page keeps them (the device settings, as the two-column splitter is handed its
   * split); absent in a host that keeps nothing, where the wider default stands for the
   * session. Every one is read on use, and a refused keep leaves the choice standing
   * until a reload.
   */
  storedWidth?(): number;
  keepWidth?(px: number): void;
  /** A width this page will apply: the device setting's clamp, handed in. */
  clampWidth?(value: unknown): number;
  /** The width a double-click (and no choice yet) restores. */
  defaultWidth?: number;
  /** Whether the phone sheet stands at full height, as last left on this device. */
  storedSheetFull?(): boolean;
  keepSheetFull?(full: boolean): void;
  /** `#1354 discuss-layout-modes`. The panel's layout mode, as last left on this device. */
  storedMode?(): DiscussLayoutMode;
  keepMode?(mode: DiscussLayoutMode): void;
}

/** `#1354 discuss-layout-modes`. How the panel stands: the resizable overlay, a docked
 * column beside the list, or maximized over the main area. */
export type DiscussLayoutMode = "overlay" | "docked" | "maximized";

/** The panel, once created. */
export interface DiscussPanel {
  /** Wire the panel's own controls (close, retry, the mark buttons); idempotent. */
  mount(): { unmount(): void };
  /** Show the panel for `scope` and read its preview; no model is asked. */
  openWith(scope: DiscussScope): void;
  /** Hide the panel; a late answer for it is then ignored. */
  close(): void;
  isOpen(): boolean;
  /** Rewrite the scope line; nothing else moves. */
  setScopeLabel(label: string): void;
  /** Forget any answer still on its way (the scope changed under it). */
  invalidate(): void;
  /** `#1344`. The overlay width in force, in CSS pixels (the wider default until dragged). */
  width(): number;
  /** `#1344`. Put a width on the panel and keep it on the device. */
  setWidth(px: unknown): void;
  /** `#1344`. Whether the phone sheet stands at full height. */
  sheetFull(): boolean;
  /** `#1344`. Stand the phone sheet at full or half height and keep the choice. */
  setSheetFull(full: boolean): void;
  /** `#1354`. The layout mode in force. */
  mode(): DiscussLayoutMode;
  /** `#1354`. Stand the panel in `mode` and keep the choice on the device. */
  setMode(mode: DiscussLayoutMode): void;
}

/** Why Discuss cannot work on a deployment with no direct LLM summarizer, said once. */
export const DISCUSS_UNAVAILABLE_HINT = "Discuss needs a direct LLM summarizer ([summaries.llm])";

/**
 * Whether a catch-up failure is the deployment refusing for want of a direct LLM
 * summarizer (503 `discuss_not_available_on_this_backend`, or the summarizer's own
 * `summarizer_not_configured`), as opposed to one attempt going wrong: the refusal
 * will still be true on a retry, so none is offered.
 */
export function isDiscussUnavailable(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const record = error as { status?: unknown; code?: unknown; message?: unknown };
  if (record.code === "discuss_not_available_on_this_backend") return true;
  if (record.code === "summarizer_not_configured") return true;
  if (record.status === 503 && typeof record.message === "string" && record.message.includes("summarizer")) return true;
  return typeof record.message === "string" && record.message.includes("summarizer_not_configured");
}

function numberOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function stringsOf(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string");
}

/** Narrow an answer body to the fields the panel reads; an empty answer for any other shape. */
export function asCatchupAnswer(value: unknown): CatchupAnswer {
  const empty: CatchupAnswer = { window: null, unread_ids: [], markable_ids: [], footer_links: [], text: "", stamp: null };
  if (typeof value !== "object" || value === null) return empty;
  const record = value as Record<string, unknown>;
  const rawWindow = typeof record.window === "object" && record.window !== null ? (record.window as Record<string, unknown>) : null;
  const rawStamp = typeof record.stamp === "object" && record.stamp !== null ? (record.stamp as Record<string, unknown>) : null;
  const links: CatchupFooterLink[] = [];
  if (Array.isArray(record.footer_links)) {
    for (const entry of record.footer_links) {
      if (typeof entry !== "object" || entry === null) continue;
      const link = entry as Record<string, unknown>;
      if (typeof link.id !== "string" || link.id === "") continue;
      links.push({
        id: link.id,
        label: typeof link.label === "string" ? link.label : link.id,
        thread_id: typeof link.thread_id === "string" ? link.thread_id : null,
      });
    }
  }
  return {
    window: rawWindow === null ? null : {
      count: numberOrNull(rawWindow.count) ?? 0,
      unread_count: numberOrNull(rawWindow.unread_count) ?? 0,
    },
    unread_ids: stringsOf(record.unread_ids),
    markable_ids: stringsOf(record.markable_ids),
    footer_links: links,
    text: typeof record.text === "string" ? record.text : "",
    stamp: rawStamp === null ? null : {
      context_ms: numberOrNull(rawStamp.context_ms) ?? 0,
      model_ms: numberOrNull(rawStamp.model_ms) ?? 0,
      first_token_ms: numberOrNull(rawStamp.first_token_ms),
      total_ms: numberOrNull(rawStamp.total_ms) ?? 0,
      input_tokens: numberOrNull(rawStamp.input_tokens),
      output_tokens: numberOrNull(rawStamp.output_tokens),
      input_chars: numberOrNull(rawStamp.input_chars) ?? 0,
      output_chars: numberOrNull(rawStamp.output_chars) ?? 0,
      unknown_term_count: numberOrNull(rawStamp.unknown_term_count) ?? 0,
    },
  };
}

/** Narrow a preview body to the unread set the bound chooses over. */
export function asCatchupPreview(value: unknown): CatchupPreview {
  const empty: CatchupPreview = { window: null, unread: [] };
  if (typeof value !== "object" || value === null) return empty;
  const record = value as Record<string, unknown>;
  const rawWindow = typeof record.window === "object" && record.window !== null ? (record.window as Record<string, unknown>) : null;
  const unread: CatchupUnreadRef[] = [];
  if (Array.isArray(record.unread)) {
    for (const entry of record.unread) {
      if (typeof entry !== "object" || entry === null) continue;
      const ref = entry as Record<string, unknown>;
      if (typeof ref.id !== "string" || ref.id === "") continue;
      unread.push({
        id: ref.id,
        at_ms: numberOrNull(ref.at_ms),
        label: typeof ref.label === "string" ? ref.label : ref.id,
      });
    }
  }
  return {
    window: rawWindow === null ? null : {
      count: numberOrNull(rawWindow.count) ?? 0,
      unread_count: numberOrNull(rawWindow.unread_count) ?? 0,
    },
    unread,
  };
}

/** The local "HH:MM" of an instant, for the bound's time end. */
export function hhmm(atMs: number): string {
  const date = new Date(atMs);
  const pad = (part: number): string => String(part).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * The bound for "last `n` unread": how many, and the instant the POST bounds at
 * (the Nth-newest unread's own time, so the server covers exactly those N).
 */
export function boundForCount(unread: readonly CatchupUnreadRef[], n: number): { count: number; sinceMs: number | null } {
  if (unread.length === 0) return { count: 0, sinceMs: null };
  const count = Math.min(Math.max(Math.trunc(n) || 1, 1), unread.length);
  return { count, sinceMs: unread.at(-count)?.at_ms ?? null };
}

/** How many unread a "since HH:MM" covers: those whose local time-of-day is at/after it. */
export function countSince(unread: readonly CatchupUnreadRef[], time: string): number {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
  if (match === null) return unread.length;
  const wanted = Number(match[1]) * 60 + Number(match[2]);
  let count = 0;
  for (const ref of unread) {
    if (ref.at_ms === null) {
      count += 1;
      continue;
    }
    const date = new Date(ref.at_ms);
    if (date.getHours() * 60 + date.getMinutes() >= wanted) count += 1;
  }
  return count;
}

/**
 * The debug line for one answer: `cache X ms · model Y ms · total Z ms · in A · out B`.
 * Sizes are tokens where the gateway really reported them and characters where it did
 * not, and the unit is always named, so a char count never reads as a token count.
 */
export function formatStampLine(stamp: CatchupStamp | null): string {
  if (stamp === null) return "";
  const parts: string[] = [
    `cache ${Math.round(stamp.context_ms)} ms`,
    `model ${Math.round(stamp.model_ms)} ms`,
    `total ${Math.round(stamp.total_ms)} ms`,
  ];
  const size = (tokens: number | null, chars: number): string => tokens !== null ? `${tokens} tokens` : `${chars} chars`;
  parts.push(`in ${size(stamp.input_tokens, stamp.input_chars)}`);
  parts.push(`out ${size(stamp.output_tokens, stamp.output_chars)}`);
  return parts.join(" · ");
}

/** A Discuss panel over the shell's `#discuss-panel`, idle until opened. */
export function createDiscussPanel(deps: DiscussPanelDependencies): DiscussPanel {
  const listening = createListening();
  let mounted = false;
  let generation = 0;
  let lastScope: DiscussScope | null = null;
  let lastAnswer: CatchupAnswer | null = null;
  let marking = false;
  // `#1344 discuss-panel-resize`. The layout in force, kept here rather than read back out
  // of the DOM (a drag would otherwise parse its own output on every move, as the column
  // splitter's split is kept). The stylesheet decides where each applies — the width only
  // on a desk, the sheet height only on a phone — and this code only ever sets the choice.
  // Resizing the overlay never reflows the list and never touches a scroll position: the
  // panel floats outside #scroll-area, so no reader place needs keeping here (#1251).
  const FALLBACK_DEFAULT_WIDTH = 560;
  const FALLBACK_MIN_WIDTH = 320;
  const FALLBACK_MAX_WIDTH = 960;
  let width = FALLBACK_DEFAULT_WIDTH;
  let sheetFull = false;
  let layoutMode: DiscussLayoutMode = "overlay";
  let drag: { startX: number; startWidth: number } | null = null;

  function panel(): HTMLElement {
    return deps.byId("discuss-panel");
  }

  function maybe(id: string): HTMLElement | null {
    try {
      return deps.byId(id);
    } catch {
      return null;
    }
  }

  function clampWidth(value: unknown): number {
    if (deps.clampWidth) return deps.clampWidth(value);
    const px = Math.round(Number(value));
    return Number.isFinite(px) ? Math.min(FALLBACK_MAX_WIDTH, Math.max(FALLBACK_MIN_WIDTH, px)) : FALLBACK_DEFAULT_WIDTH;
  }

  function defaultWidth(): number {
    return deps.defaultWidth ?? FALLBACK_DEFAULT_WIDTH;
  }

  /** Put `px` on the panel as `--discuss-width`; kept on the device only when `keep`. */
  function applyWidth(px: unknown, keep: boolean): number {
    width = clampWidth(px);
    const style = (panel() as HTMLElement).style as { setProperty?: (name: string, value: string) => void } | undefined;
    if (style && typeof style.setProperty === "function") style.setProperty("--discuss-width", `${width}px`);
    // The docked column is sized by the main screen's grid, which reads the same choice there.
    const screen = maybe("screen-main");
    const screenStyle = screen ? (screen as HTMLElement).style as { setProperty?: (name: string, value: string) => void } | undefined : undefined;
    if (screenStyle && typeof screenStyle.setProperty === "function") screenStyle.setProperty("--discuss-width", `${width}px`);
    const handle = maybe("discuss-resize");
    if (handle) {
      handle.setAttribute("aria-valuenow", String(width));
      handle.setAttribute("aria-valuetext", `Discuss panel ${width} pixels wide`);
    }
    if (keep) {
      try {
        deps.keepWidth?.(width);
      } catch {
        // Storage refused: the width holds until a reload, as a refused split does.
      }
    }
    return width;
  }

  /** Stand the phone sheet at full or half height; kept on the device only when `keep`. */
  function applySheet(full: boolean, keep: boolean): boolean {
    sheetFull = full;
    panel().setAttribute("data-sheet", full ? "full" : "half");
    const toggle = maybe("discuss-sheet-toggle");
    if (toggle) {
      toggle.textContent = full ? "Restore" : "Maximize";
      toggle.setAttribute("aria-expanded", full ? "true" : "false");
    }
    if (keep) {
      try {
        deps.keepSheetFull?.(full);
      } catch {
        // Storage refused: the height holds until a reload.
      }
    }
    return sheetFull;
  }

  /**
   * `#1354 discuss-layout-modes`. Stand the panel in `mode`; kept on the device only when
   * `keep`. Docked is drawn by the main screen's grid — the desktop two-column machinery
   * (`#229`): the panel takes the side column's place beside the list, so this only names
   * the mode on both elements and the stylesheet lays it out; no scroll position is ever
   * written here, the scroll owner's stability hold keeping the reader's place across
   * the reflow (#1251). On a phone the stylesheet gives docked no column, so the sheet
   * stands as it did.
   */
  function applyMode(mode: DiscussLayoutMode, keep: boolean): DiscussLayoutMode {
    layoutMode = mode;
    panel().setAttribute("data-mode", mode);
    const screen = maybe("screen-main");
    if (screen) {
      if (mode === "overlay") screen.removeAttribute("data-discuss");
      else screen.setAttribute("data-discuss", mode);
    }
    for (const each of ["overlay", "docked", "maximized"] as const) {
      const button = maybe(`discuss-mode-${each}`);
      if (button) button.setAttribute("aria-pressed", each === mode ? "true" : "false");
    }
    if (keep) {
      try {
        deps.keepMode?.(mode);
      } catch {
        // Storage refused: the mode holds until a reload.
      }
    }
    return layoutMode;
  }

  /** Restate the kept layout on the panel: the stored choices, else the defaults. */
  function applyStoredLayout(): void {
    let storedWidth: number | undefined;
    try {
      storedWidth = deps.storedWidth?.();
    } catch {
      storedWidth = undefined;
    }
    applyWidth(storedWidth === undefined ? defaultWidth() : storedWidth, false);
    let storedFull = false;
    try {
      storedFull = deps.storedSheetFull?.() ?? false;
    } catch {
      storedFull = false;
    }
    applySheet(storedFull, false);
    let storedMode: DiscussLayoutMode = "overlay";
    try {
      storedMode = deps.storedMode?.() ?? "overlay";
    } catch {
      storedMode = "overlay";
    }
    applyMode(storedMode === "docked" || storedMode === "maximized" ? storedMode : "overlay", false);
  }

  function isOpen(): boolean {
    try {
      return !panel().hidden;
    } catch {
      return false;
    }
  }

  function setOpen(open: boolean): void {
    panel().hidden = !open;
    try {
      deps.byId("discuss-open").setAttribute("aria-expanded", open ? "true" : "false");
    } catch {
      // A host without the bar button still opens and closes.
    }
  }

  /** Draw the footer's covered list and its mark buttons from one answer. */
  function drawFooter(answer: CatchupAnswer, scope: DiscussScope): void {
    const covered = answer.unread_ids.length;
    const footer = deps.byId("discuss-footer");
    footer.hidden = covered === 0;
    if (covered === 0) return;
    deps.byId("discuss-covers").textContent = `This summary covers ${covered} unread messages:`;
    const list = deps.byId("discuss-links");
    list.replaceChildren();
    for (const link of answer.footer_links) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "discuss-link";
      button.textContent = link.label;
      listening.listen(button, "click", () => {
        deps.onJump({
          service: deps.serviceName(scope.channelId),
          channel: scope.channelId,
          message: link.id,
          thread: link.thread_id,
        });
      });
      item.appendChild(button);
      list.appendChild(item);
    }
    const unmarkable = deps.byId("discuss-unmarkable");
    if (answer.unread_ids.length !== answer.markable_ids.length) {
      unmarkable.hidden = false;
      unmarkable.textContent = `${answer.unread_ids.length - answer.markable_ids.length} can't be marked here`;
    } else {
      unmarkable.hidden = true;
      unmarkable.textContent = "";
    }
  }

  let previewUnread: CatchupUnreadRef[] = [];
  let boundCount = 0;
  let lastSinceMs: number | null = null;
  let pending = false;

  /** Say whether a request is in flight, here and on the page's entry points. */
  function setPending(on: boolean): void {
    pending = on;
    deps.onBusy?.(on);
    drawBound();
  }

  /** Draw the Summarize bound: its button, and the two linked ends of it. */
  function drawBound(): void {
    const button = deps.byId("discuss-summarize") as HTMLButtonElement;
    const count = deps.byId("discuss-count") as HTMLInputElement;
    const since = deps.byId("discuss-since") as HTMLInputElement;
    const none = previewUnread.length === 0;
    button.disabled = pending || none;
    count.disabled = pending || none;
    since.disabled = pending || none;
    if (none) {
      button.textContent = "Summarize";
      return;
    }
    const bound = boundForCount(previewUnread, boundCount || previewUnread.length);
    boundCount = bound.count;
    count.value = String(bound.count);
    count.max = String(previewUnread.length);
    since.value = bound.sinceMs === null ? "" : hhmm(bound.sinceMs);
    button.textContent = `Summarize last ${bound.count} unread since ${since.value || "—"}`;
  }

  /** Read the scope's preview and offer its bound; no model is asked. */
  function previewFor(scope: DiscussScope): void {
    generation += 1;
    const asked = generation;
    lastScope = scope;
    deps.byId("discuss-scope").textContent = scope.label;
    deps.byId("discuss-status").textContent = "Checking what to summarize…";
    void deps.catchupPreview(scope.channelId, scope).then(
      (value) => {
        if (asked !== generation || !isOpen()) return;
        const preview = asCatchupPreview(value);
        previewUnread = preview.unread;
        boundCount = preview.unread.length;
        if (previewUnread.length === 0) {
          deps.byId("discuss-status").textContent = "Nothing unread here.";
        } else {
          deps.byId("discuss-status").textContent = `${previewUnread.length} unread ready.`;
        }
        drawBound();
      },
      (error: unknown) => {
        if (asked !== generation || !isOpen()) return;
        previewUnread = [];
        drawBound();
        deps.byId("discuss-status").textContent = error instanceof Error ? error.message : String(error);
      },
    );
  }

  /** One catch-up for `scope` at the chosen bound; nothing starts another meanwhile. */
  function fetchFor(scope: DiscussScope, sinceMs: number | null): void {
    if (pending) return;
    lastSinceMs = sinceMs;
    generation += 1;
    const asked = generation;
    lastScope = scope;
    lastAnswer = null;
    deps.byId("discuss-scope").textContent = scope.label;
    deps.byId("discuss-status").textContent = "Reading…";
    deps.byId("discuss-answer").textContent = "";
    deps.byId("discuss-footer").hidden = true;
    deps.byId("discuss-stamp").textContent = "";
    deps.byId("discuss-stamp").hidden = true;
    deps.byId("discuss-retry").hidden = true;
    setPending(true);
    const body: CatchupRequestBody = { thread_id: scope.threadId, view: scope.view, since_ms: sinceMs };
    void deps.catchupPost(scope.channelId, body).then(
      (value) => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        const answer = asCatchupAnswer(value);
        lastAnswer = answer;
        const unread = answer.window?.unread_count ?? answer.unread_ids.length;
        const count = answer.window?.count ?? answer.unread_ids.length;
        deps.byId("discuss-status").textContent = `Read ${unread} unread of ${count}`;
        deps.byId("discuss-answer").textContent = answer.text;
        drawFooter(answer, scope);
        const stampLine = formatStampLine(answer.stamp);
        deps.byId("discuss-stamp").textContent = stampLine;
        deps.byId("discuss-stamp").hidden = stampLine === "";
      },
      (error: unknown) => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        // A deployment with no summarizer refuses the same way every time: say the
        // hint and offer no retry, where a failed attempt gets its error and one.
        if (isDiscussUnavailable(error)) {
          deps.byId("discuss-status").textContent = DISCUSS_UNAVAILABLE_HINT;
          deps.byId("discuss-retry").hidden = true;
          deps.showError?.(DISCUSS_UNAVAILABLE_HINT);
          return;
        }
        const message = error instanceof Error ? error.message : String(error);
        deps.byId("discuss-status").textContent = message;
        deps.byId("discuss-retry").hidden = false;
        deps.showError?.(message);
      },
    );
  }

  /** Mark read, or put back, exactly the last answer's markable ids. */
  function mark(read: boolean): void {
    if (lastAnswer === null || marking) return;
    const ids = [...lastAnswer.markable_ids];
    if (ids.length === 0) return;
    marking = true;
    const readButton = deps.byId("discuss-mark-read") as HTMLButtonElement;
    const unreadButton = deps.byId("discuss-mark-unread") as HTMLButtonElement;
    readButton.disabled = true;
    unreadButton.disabled = true;
    const run = read ? deps.dismissMessages({ messages: ids }) : deps.restoreMessages(ids);
    void run.then(
      () => deps.setStatus?.(read ? "Marked read." : "Marked unread."),
      (error: unknown) => deps.showError?.(error instanceof Error ? error.message : String(error)),
    ).finally(() => {
      marking = false;
      readButton.disabled = false;
      unreadButton.disabled = false;
    });
  }

  /** The resize handle's keys: Left widens (the panel stands on the right), Right narrows. */
  const widthKeys: { readonly [key: string]: (() => number) | undefined } = {
    ArrowLeft: () => width + 16,
    ArrowRight: () => width - 16,
    PageUp: () => width + 64,
    PageDown: () => width - 64,
    Home: () => clampWidth(0),
    End: () => clampWidth(100000),
  };

  return {
    mount() {
      if (!mounted) {
        mounted = true;
        // The kept layout stands before anything opens, so a reopen is where it was left.
        try {
          applyStoredLayout();
        } catch {
          // A host without the panel yet draws it at the first open.
        }
        listening.listen(deps.byId("discuss-close"), "click", () => {
          generation += 1;
          if (pending) setPending(false);
          setOpen(false);
        });
        listening.listen(deps.byId("discuss-retry"), "click", () => {
          if (lastScope !== null) fetchFor(lastScope, lastSinceMs);
        });
        listening.listen(deps.byId("discuss-summarize"), "click", () => {
          if (pending || lastScope === null || previewUnread.length === 0) return;
          fetchFor(lastScope, boundForCount(previewUnread, boundCount).sinceMs);
        });
        const countInput = deps.byId("discuss-count") as HTMLInputElement;
        listening.listen(countInput, "input", () => {
          boundCount = boundForCount(previewUnread, Number(countInput.value)).count;
          drawBound();
        });
        const sinceInput = deps.byId("discuss-since") as HTMLInputElement;
        listening.listen(sinceInput, "input", () => {
          boundCount = countSince(previewUnread, sinceInput.value);
          drawBound();
        });
        listening.listen(deps.byId("discuss-mark-read"), "click", () => mark(true));
        listening.listen(deps.byId("discuss-mark-unread"), "click", () => mark(false));
        const toggle = maybe("discuss-sheet-toggle");
        if (toggle) {
          listening.listen(toggle, "click", () => applySheet(!sheetFull, true));
        }
        // `#1354`: the mode control beside Close, and the panel's keys as the
        // outline panel's are: Escape away, focus home on open (below).
        for (const each of ["overlay", "docked", "maximized"] as const) {
          const button = maybe(`discuss-mode-${each}`);
          if (button) listening.listen(button, "click", () => applyMode(each, true));
        }
        listening.listen(panel(), "keydown", (event: KeyboardEvent) => {
          if (event.key !== "Escape") return;
          if (typeof event.preventDefault === "function") event.preventDefault();
          generation += 1;
          setOpen(false);
          const opener = maybe("discuss-open") as (HTMLElement & { focus?: (options?: unknown) => void }) | null;
          if (opener && typeof opener.focus === "function") opener.focus({ preventScroll: true });
        });
        const handle = maybe("discuss-resize");
        if (handle) {
          listening.listen(handle, "pointerdown", (event: PointerEvent) => {
            drag = { startX: event.clientX, startWidth: width };
            const holder = handle as HTMLElement & { setPointerCapture?: (id: number) => void };
            if (typeof holder.setPointerCapture === "function" && event.pointerId !== undefined) {
              try {
                holder.setPointerCapture(event.pointerId);
              } catch {
                // No pointer to hold: the move/up on the handle still follow a short drag.
              }
            }
            if (typeof event.preventDefault === "function") event.preventDefault();
          });
          listening.listen(handle, "pointermove", (event: PointerEvent) => {
            if (!drag) return;
            // Anchored right: dragging left widens, stored once at the end (up), as the split is.
            applyWidth(drag.startWidth + (drag.startX - event.clientX), false);
          });
          const endDrag = (): void => {
            if (!drag) return;
            drag = null;
            applyWidth(width, true);
          };
          listening.listen(handle, "pointerup", endDrag);
          listening.listen(handle, "pointercancel", endDrag);
          listening.listen(handle, "keydown", (event: KeyboardEvent) => {
            const next = widthKeys[event.key];
            if (!next) return;
            if (typeof event.preventDefault === "function") event.preventDefault();
            applyWidth(next(), true);
          });
          listening.listen(handle, "dblclick", () => applyWidth(defaultWidth(), true));
        }
      }
      return {
        unmount() {
          mounted = false;
          drag = null;
          listening.unmount();
        },
      };
    },
    openWith(scope: DiscussScope) {
      try {
        applyStoredLayout();
      } catch {
        // No panel in this host yet: the open below says so, as before.
      }
      setOpen(true);
      // Focus home on open, as the other panels put it: Close, without scrolling.
      const closer = maybe("discuss-close") as (HTMLElement & { focus?: (options?: unknown) => void }) | null;
      if (closer && typeof closer.focus === "function") closer.focus({ preventScroll: true });
      previewFor(scope);
    },
    close() {
      generation += 1;
      if (pending) setPending(false);
      setOpen(false);
    },
    isOpen,
    setScopeLabel(label: string) {
      deps.byId("discuss-scope").textContent = label;
    },
    invalidate() {
      generation += 1;
    },
    width: () => width,
    setWidth(px: unknown) {
      applyWidth(px, true);
    },
    sheetFull: () => sheetFull,
    setSheetFull(full: boolean) {
      applySheet(full, true);
    },
    mode: () => layoutMode,
    setMode(mode: DiscussLayoutMode) {
      applyMode(mode, true);
    },
  };
}

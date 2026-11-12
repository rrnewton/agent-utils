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

import { createListening, insertSanitizedHtml, setClassWord } from "../dom/index.ts";
import { createComposerKeys } from "../composer/index.ts";
import { deepLinkRefFrom } from "../../foundation/index.ts";
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
  cited_ids: string[];
  footer_links: CatchupFooterLink[];
  /** The cited subset (`#1350`), narrowed here for the record's restore. */
  cited_ids?: string[];
  text: string;
  /** The answer as the server's sanitized render (`#1349`); empty draws `text` plain. */
  text_html: string;
  /** Terms the answer said it did not understand (`#1351`), drawn collapsed. */
  unknown_terms: string[];
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
  /** GET the scope's saved Discuss record for `channelId`; spends no model call. */
  catchupRecord(channelId: string, scope: DiscussScope): Promise<unknown>;
  /** POST one follow-up for `channelId`; answers the appended turns as `unknown`. */
  catchupFollowup(channelId: string, scope: DiscussScope, text: string): Promise<unknown>;
  /** Whether a bare Enter sends (the reader's "Enter to send"), for the composer keys. */
  enterSends?(): boolean;
  /** A request started or settled, so the page can disable its entry points too. */
  onBusy?(busy: boolean): unknown;
  /** The chat service `channelId` is read through, for a footer's jump refs. */
  serviceName(channelId: string): string;
  /** Jump to the message a footer link names. */
  onJump(ref: MessageLinkRef): unknown;
  /**
   * `#1395 discuss-scope-binding`. Route one citation tap from the answer:
   * the ref it names and whether the server marked it out-of-scope
   * (`scope=out` in its address). When present, `answerTap` — the single
   * tap-routing function — hands every citation here and the view decides
   * (binding.ts) whether the panel stays; absent, citations jump as before.
   */
  onLinkTap?(ref: MessageLinkRef, outOfScope: boolean): unknown;
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
  /**
   * `#1395 discuss-scope-binding`. The answer on screen and its scope, for a
   * history entry or a same-scope reopen; null before any answer lands.
   */
  snapshot(): { scope: DiscussScope; answer: CatchupAnswer } | null;
  /**
   * `#1395 discuss-scope-binding`. Show `answer` for `scope` again with no
   * fetch at all — no preview, no model call (Back-restore, same-scope
   * reopen). The `#1345` record restore supersedes this where it exists.
   */
  restoreAnswer(scope: DiscussScope, answer: CatchupAnswer): void;
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
  const empty: CatchupAnswer = { window: null, unread_ids: [], markable_ids: [], cited_ids: [], footer_links: [], text: "", text_html: "", unknown_terms: [], stamp: null };
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
    cited_ids: stringsOf(record.cited_ids),
    footer_links: links,
    text: typeof record.text === "string" ? record.text : "",
    text_html: typeof record.text_html === "string" ? record.text_html : "",
    unknown_terms: stringsOf(record.unknown_terms),
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

/** One timeline entry under a saved summary, narrowed for drawing. */
export type DiscussEntryShape =
  | { kind: "turn"; speaker: string; text: string; text_html: string; at_ms: number }
  | { kind: "summary"; answer: CatchupAnswer; created_ms: number };

/** A saved Discuss record, narrowed to what reopening draws. */
export interface CatchupRecordShape {
  exists: boolean;
  summary: CatchupAnswer | null;
  created_ms: number;
  timeline: DiscussEntryShape[];
  staleness: { new_unread: number; marked_read: number } | null;
}

/** Narrow one saved summary (record fields) to an answer the panel draws. */
function asSavedAnswer(value: unknown): { answer: CatchupAnswer; created_ms: number } | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const rawWindow = typeof record.window === "object" && record.window !== null ? (record.window as Record<string, unknown>) : null;
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
  const rawStamp = typeof record.stamp === "object" && record.stamp !== null ? (record.stamp as Record<string, unknown>) : null;
  return {
    answer: {
      window: rawWindow === null ? null : {
        count: numberOrNull(rawWindow.count) ?? 0,
        unread_count: numberOrNull(rawWindow.unread_count) ?? 0,
      },
      unread_ids: stringsOf(record.covered_ids),
      markable_ids: stringsOf(record.markable_ids),
      footer_links: links,
      cited_ids: stringsOf(record.cited_ids),
      text: typeof record.text === "string" ? record.text : "",
      text_html: typeof record.text_html === "string" ? record.text_html : "",
      unknown_terms: stringsOf(record.unknown_terms),
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
    },
    created_ms: numberOrNull(record.created_ms) ?? 0,
  };
}

/** Narrow a record body to what reopening draws. */
export function asCatchupRecord(value: unknown): CatchupRecordShape {
  const empty: CatchupRecordShape = { exists: false, summary: null, created_ms: 0, timeline: [], staleness: null };
  if (typeof value !== "object" || value === null) return empty;
  const record = value as Record<string, unknown>;
  const saved = asSavedAnswer(record.current_summary);
  const timeline: DiscussEntryShape[] = [];
  if (Array.isArray(record.timeline)) {
    for (const entry of record.timeline) {
      if (typeof entry !== "object" || entry === null) continue;
      const item = entry as Record<string, unknown>;
      if (item.kind === "summary") {
        const old = asSavedAnswer(item.summary);
        if (old !== null) timeline.push({ kind: "summary", answer: old.answer, created_ms: old.created_ms });
      } else if (item.kind === "turn" && typeof item.turn === "object" && item.turn !== null) {
        const turn = item.turn as Record<string, unknown>;
        timeline.push({
          kind: "turn",
          speaker: typeof turn.speaker === "string" ? turn.speaker : "agent",
          text: typeof turn.text === "string" ? turn.text : "",
          text_html: typeof turn.text_html === "string" ? turn.text_html : "",
          at_ms: numberOrNull(turn.at_ms) ?? 0,
        });
      }
    }
  }
  const rawStale = typeof record.staleness === "object" && record.staleness !== null ? (record.staleness as Record<string, unknown>) : null;
  return {
    exists: record.exists === true,
    summary: saved?.answer ?? null,
    created_ms: saved?.created_ms ?? 0,
    timeline,
    staleness: rawStale === null ? null : {
      new_unread: numberOrNull(rawStale.new_unread) ?? 0,
      marked_read: numberOrNull(rawStale.marked_read) ?? 0,
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

  /**
   * `#1383 discuss-header-icons`. The header's two layout toggles, stated from the
   * layout in force: dock is pressed while docked, fullscreen while maximized on
   * a desk or full-height on a phone. Their words name the act a press performs.
   */
  function drawHeaderToggles(): void {
    const dock = maybe("discuss-dock-toggle");
    if (dock) {
      const docked = layoutMode === "docked";
      dock.setAttribute("aria-pressed", docked ? "true" : "false");
      dock.setAttribute("aria-label", docked ? "Float panel" : "Dock panel");
      dock.setAttribute("title", docked ? "Float panel" : "Dock panel");
    }
    const full = maybe("discuss-full-toggle");
    if (full) {
      const deskFull = layoutMode === "maximized";
      full.setAttribute("aria-pressed", deskFull || sheetFull ? "true" : "false");
      full.setAttribute("aria-label", deskFull || sheetFull ? "Exit full screen" : "Full screen");
      full.setAttribute("title", deskFull || sheetFull ? "Exit full screen" : "Full screen");
    }
  }

  /** Whether this device draws the desk layouts (the stylesheet's 900px break). */
  function deskLayout(): boolean {
    try {
      const width = document.documentElement?.clientWidth ?? 0;
      return width === 0 || width >= 900;
    } catch {
      return true;
    }
  }

  /** The mode fullscreen restores to, remembered when fullscreen is entered. */
  let beforeFull: DiscussLayoutMode = "overlay";

  /** Stand the phone sheet at full or half height; kept on the device only when `keep`. */
  function applySheet(full: boolean, keep: boolean): boolean {
    sheetFull = full;
    panel().setAttribute("data-sheet", full ? "full" : "half");
    drawHeaderToggles();
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
  /** Name the layout on `#screen-main` only while the panel stands open in it: a
   * closed panel reserves no column, whatever mode is remembered for reopening. */
  function syncScreenMode(): void {
    const screen = maybe("screen-main");
    if (!screen) return;
    if (!isOpen() || layoutMode === "overlay") screen.removeAttribute("data-discuss");
    else screen.setAttribute("data-discuss", layoutMode);
  }

  function applyMode(mode: DiscussLayoutMode, keep: boolean): DiscussLayoutMode {
    layoutMode = mode;
    panel().setAttribute("data-mode", mode);
    syncScreenMode();
    drawHeaderToggles();
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
    // Every exit (close(), the X, Escape, the bar toggle) lands here, so the
    // screen's docked/maximized mark follows openness from this one place.
    syncScreenMode();
    try {
      deps.byId("discuss-open").setAttribute("aria-expanded", open ? "true" : "false");
    } catch {
      // A host without the bar button still opens and closes.
    }
  }

  /**
   * Draw one answer (`discuss-markdown-render`): the server's sanitized render as
   * markup when it sent one, the shaped text as text otherwise. Model text reaches
   * innerHTML only through `text_html` — never directly.
   */
  function drawAnswer(answer: CatchupAnswer): void {
    const box = deps.byId("discuss-answer");
    box.replaceChildren();
    if (answer.text_html !== "") {
      setClassWord(box, "discuss-answer--plain", false);
      insertSanitizedHtml(box, answer.text_html);
    } else {
      setClassWord(box, "discuss-answer--plain", true);
      box.textContent = answer.text;
    }
  }

  /** The terms the answer did not understand, collapsed (`#1351`). */
  function drawTerms(answer: CatchupAnswer): void {
    const box = deps.byId("discuss-terms");
    box.hidden = answer.unknown_terms.length === 0;
    if (answer.unknown_terms.length === 0) return;
    deps.byId("discuss-terms-summary").textContent =
      `Terms I didn't understand (${answer.unknown_terms.length})`;
    const list = deps.byId("discuss-terms-list");
    list.replaceChildren();
    for (const term of answer.unknown_terms) {
      const item = document.createElement("li");
      item.textContent = term;
      list.appendChild(item);
    }
  }

  /**
   * A tap inside the answer (`#1395 discuss-scope-binding`): the single
   * tap-routing function. An in-app citation link is routed — through the
   * view's binding decision when wired, as a plain jump otherwise — and
   * anything else goes on. The server marks a cache-resolved citation
   * outside the window with `scope=out` in its address; that flag, and
   * only that, makes the tap out-of-scope.
   */
  function answerTap(event: Event): void {
    let node: unknown = event.target;
    while (node !== null && node !== undefined) {
      const el = node as HTMLElement;
      if (typeof el.tagName === "string" && el.tagName.toLowerCase() === "a") {
        const href = el.getAttribute("href") ?? "";
        const queryAt = href.indexOf("?");
        if (!href.startsWith("http") && queryAt >= 0) {
          const params = new URLSearchParams(href.slice(queryAt + 1));
          const ref = deepLinkRefFrom(params.get("m") ?? "");
          if (ref !== null) {
            event.preventDefault();
            const linkRef: MessageLinkRef = {
              service: ref.provider,
              channel: ref.channel,
              message: ref.message,
              thread: ref.thread,
            };
            if (deps.onLinkTap) deps.onLinkTap(linkRef, params.get("scope") === "out");
            else deps.onJump(linkRef);
          }
        }
        return;
      }
      if (el === (deps.byId("discuss-answer") as unknown)) return;
      node = (el as { parentNode?: unknown }).parentNode;
    }
  }

  /** The staleness banner over a saved summary (`discuss-summary-staleness-update`). */
  function drawStale(shape: CatchupRecordShape): void {
    const stale = shape.staleness;
    const box = deps.byId("discuss-stale");
    const staleNow = stale !== null && (stale.new_unread > 0 || stale.marked_read > 0);
    box.hidden = !staleNow || shape.summary === null;
    if (!staleNow || stale === null) return;
    deps.byId("discuss-stale-text").textContent =
      `${stale.new_unread} new unread · ${stale.marked_read} marked read since summary`;
  }

  /** The timeline under a saved summary: turns, and superseded summaries collapsed. */
  function drawTimeline(entries: DiscussEntryShape[], scope: DiscussScope): void {
    const box = deps.byId("discuss-timeline");
    box.replaceChildren();
    for (const entry of entries) {
      if (entry.kind === "turn") {
        const line = document.createElement("p");
        line.className = entry.speaker === "you" ? "discuss-turn discuss-you" : "discuss-turn discuss-agent";
        if (entry.text_html !== "") insertSanitizedHtml(line, entry.text_html);
        else line.textContent = entry.text;
        box.appendChild(line);
      } else {
        const details = document.createElement("details");
        details.className = "discuss-old";
        const summary = document.createElement("summary");
        const count = entry.answer.window?.unread_count ?? entry.answer.unread_ids.length;
        summary.textContent = `Earlier summary · ${entry.created_ms > 0 ? hhmm(entry.created_ms) : "—"} · ${count} unread`;
        details.appendChild(summary);
        const body = document.createElement("div");
        if (entry.answer.text_html !== "") insertSanitizedHtml(body, entry.answer.text_html);
        else body.textContent = entry.answer.text;
        details.appendChild(body);
        box.appendChild(details);
      }
    }
    void scope;
  }

  /** Send the composer's question: one follow-up, appended to the timeline. */
  function sendFollowup(): void {
    if (pending || lastScope === null || lastAnswer === null) return;
    const box = deps.byId("discuss-followup") as HTMLTextAreaElement;
    const text = box.value.trim();
    if (text === "") return;
    generation += 1;
    const asked = generation;
    const scope = lastScope;
    setPending(true);
    deps.byId("discuss-status").textContent = "Reading…";
    void deps.catchupFollowup(scope.channelId, scope, text).then(
      () => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        box.value = "";
        deps.byId("discuss-status").textContent = "Answered below.";
        recordFor(scope, true);
      },
      (error: unknown) => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        deps.byId("discuss-status").textContent = error instanceof Error ? error.message : String(error);
        deps.byId("discuss-retry").hidden = true;
      },
    );
  }

  /** Read the saved record and draw it; reopening asks nothing of the model. */
  function recordFor(scope: DiscussScope, quiet = false): void {
    const asked = generation;
    void deps.catchupRecord(scope.channelId, scope).then(
      (value) => {
        if (asked !== generation || !isOpen()) return;
        const shape = asCatchupRecord(value);
        if (shape.summary !== null) {
          lastAnswer = shape.summary;
          drawAnswer(shape.summary);
          drawTerms(shape.summary);
          drawFooter(shape.summary, scope);
          const stampLine = formatStampLine(shape.summary.stamp);
          deps.byId("discuss-stamp").textContent = stampLine;
          deps.byId("discuss-stamp").hidden = stampLine === "";
          if (!quiet) {
            deps.byId("discuss-status").textContent =
              `Saved summary · ${shape.created_ms > 0 ? hhmm(shape.created_ms) : "—"}`;
          }
        }
        drawStale(shape);
        drawTimeline(shape.timeline, scope);
      },
      () => {
        // No record to draw (or none readable): the preview's offer stands.
      },
    );
  }

  /** Draw the footer's covered list and its mark buttons from one answer. */
  function drawFooter(answer: CatchupAnswer, scope: DiscussScope): void {
    const covered = answer.unread_ids.length;
    const footer = deps.byId("discuss-footer");
    footer.hidden = covered === 0;
    if (covered === 0) return;
    deps.byId("discuss-covers").textContent = "Other messages processed:";
    const cited = new Set(answer.cited_ids);
    const list = deps.byId("discuss-links");
    list.replaceChildren();
    for (const link of answer.footer_links) {
      if (cited.has(link.id)) continue;
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "discuss-link";
      button.textContent = link.label.split(" · ")[0] ?? link.label;
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
    // `#1382`: the button says only "Summarize"; the line beside it carries the bound.
    button.textContent = "Summarize";
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
        drawAnswer(answer);
        drawTerms(answer);
        drawFooter(answer, scope);
        const stampLine = formatStampLine(answer.stamp);
        deps.byId("discuss-stamp").textContent = stampLine;
        deps.byId("discuss-stamp").hidden = stampLine === "";
        deps.byId("discuss-stale").hidden = true;
        recordFor(scope, true);
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
        // The main composer's keys (`discuss-followup-composer`): the same Enter.
        const composerKeys = createComposerKeys({
          now: () => Date.now(),
          enterSends: () => deps.enterSends?.() ?? true,
        });
        const followupBox = deps.byId("discuss-followup");
        listening.listen(followupBox, "keydown", (event) => {
          composerKeys.onKey(event as never, () => sendFollowup(), followupBox);
        });
        listening.listen(followupBox, "beforeinput", (event) => composerKeys.noteInsertion(followupBox, event));
        listening.listen(followupBox, "input", (event) => composerKeys.noteInsertion(followupBox, event));
        listening.listen(deps.byId("discuss-send"), "click", () => sendFollowup());
        listening.listen(deps.byId("discuss-update"), "click", () => {
          if (pending || lastScope === null) return;
          fetchFor(lastScope, null);
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
        listening.listen(deps.byId("discuss-answer"), "click", (event) => answerTap(event));
        listening.listen(deps.byId("discuss-mark-read"), "click", () => mark(true));
        listening.listen(deps.byId("discuss-mark-unread"), "click", () => mark(false));
        // `#1383`: the header's two layout toggles beside Close. Dock switches
        // overlay/docked (leaving fullscreen first); fullscreen switches
        // maximized on a desk and the sheet's full height on a phone.
        const dockToggle = maybe("discuss-dock-toggle");
        if (dockToggle) {
          listening.listen(dockToggle, "click", () => {
            applyMode(layoutMode === "docked" ? "overlay" : "docked", true);
          });
        }
        const fullToggle = maybe("discuss-full-toggle");
        if (fullToggle) {
          listening.listen(fullToggle, "click", () => {
            if (!deskLayout()) {
              applySheet(!sheetFull, true);
            } else if (layoutMode === "maximized") {
              applyMode(beforeFull, true);
            } else {
              beforeFull = layoutMode;
              applyMode("maximized", true);
            }
          });
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
      recordFor(scope);
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
    snapshot() {
      if (lastScope === null || lastAnswer === null) return null;
      return { scope: lastScope, answer: lastAnswer };
    },
    restoreAnswer(scope: DiscussScope, answer: CatchupAnswer) {
      generation += 1;
      if (pending) setPending(false);
      lastScope = scope;
      lastAnswer = answer;
      try {
        applyStoredLayout();
      } catch {
        // No panel in this host yet: the open below says so, as before.
      }
      setOpen(true);
      deps.byId("discuss-scope").textContent = scope.label;
      const unread = answer.window?.unread_count ?? answer.unread_ids.length;
      const count = answer.window?.count ?? answer.unread_ids.length;
      deps.byId("discuss-status").textContent = `Read ${unread} unread of ${count}`;
      drawAnswer(answer);
      drawTerms(answer);
      drawFooter(answer, scope);
      const stampLine = formatStampLine(answer.stamp);
      deps.byId("discuss-stamp").textContent = stampLine;
      deps.byId("discuss-stamp").hidden = stampLine === "";
      deps.byId("discuss-retry").hidden = true;
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

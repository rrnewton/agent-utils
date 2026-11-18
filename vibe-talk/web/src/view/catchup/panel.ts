// The Discuss panel's DOM (`catchup-side-agent`): catch-up over the channel or thread
// on screen. Opening asks nothing of the model (`discuss-summarize-button`): it reads
// the scope's unread preview from the cache and offers "[N] unread, [K] hours
// since [HH:MM]" beside Summarize — one bound with three linked ends (`#1524`,
// K rounded up from the start to now) — and only that press POSTs. While
// a request is in flight nothing starts another. The answer's model text is set as
// text only, never as markup, and the footer — which unread messages the summary
// covers, a jump link for each, and Mark read / Mark unread over exactly the
// server's markable ids — is drawn from the structured fields, never parsed out of
// the model's words. Construction does nothing: no element is looked up and nothing
// is fetched until `mount` wires the controls and `openWith` is called.
//
// The answer's wire types are the generated contract's (`CatchupResponse` and its
// parts); the panel narrows the decoded body to the fields it reads.

import { createListening, foldSectionsByHeading, insertSanitizedHtml, openSectionKeys, setAllSections, setClassWord } from "../dom/index.ts";
import { applyStrikes } from "./strikes.ts";
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
export type CatchupWindow = Pick<WireWindow, "count" | "unread_count"> & {
  /** Whether older messages exist outside the window (`#1761`). */
  truncated?: boolean;
  /** Whether fetching those older messages stopped refused (`#1761`). */
  fetch_blocked?: boolean;
};

/** One covered message the footer links to. */
export type CatchupFooterLink = WireFooterLink;

/** The server's timing and size stamp; token counts are null unless really reported. */
export type CatchupStamp = WireStamp;

/** One catch-up answer, narrowed to the fields the panel reads. */
export interface CatchupAnswer {
  window: CatchupWindow | null;
  unread_ids: string[];
  markable_ids: string[];
  /** The cited subset (`#1350`), narrowed here for the record's restore. */
  cited_ids: string[];
  /** The latest Update's cited ids (`#1388` landing 2); absent draws none fresh. */
  fresh_ids: string[];
  footer_links: CatchupFooterLink[];
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

/** `#1811`. The panel title for one scope: main channel, or thread slug. */
export function discussTitle(scope: DiscussScope): string {
  if (scope.threadId === null) return "Discuss: main channel";
  // `#1841`: an unnamed thread scope is still a thread, never the channel.
  return scope.label ? `Discuss: thread ${scope.label}` : "Discuss: thread";
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
  /** POST one incremental Update for `channelId` (`#1388` landing 2); answers the merged summary. */
  catchupUpdate?(channelId: string, scope: DiscussScope): Promise<unknown>;
  /**
   * `#1388` landing 1. Whether message `id` is read NOW, from the page's read
   * set; absent, nothing is ever struck. Read on every strike recompute.
   */
  isReadNow?(id: string): boolean;
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
  /** `#1552`. The docked pane's height (viewport %), as last left on this device. */
  storedDockedHeight?(): number;
  keepDockedHeight?(pct: number): void;
  /** A docked height this page will apply: the device setting's clamp, handed in. */
  clampDockedHeight?(value: unknown): number;
  /** The docked height a double-click (and no choice yet) restores. */
  defaultDockedHeight?: number;
  /** Whether the phone sheet stands at full height, as last left on this device. */
  storedSheetFull?(): boolean;
  keepSheetFull?(full: boolean): void;
  /** `#1354 discuss-layout-modes`. The panel's layout mode, as last left on this device. */
  storedMode?(): DiscussLayoutMode;
  keepMode?(mode: DiscussLayoutMode): void;
  /**
   * `#1497 discuss-fold-state-persist`. The one kept fold entry on this device
   * and its keep, handed in where the page keeps them (the device settings, as
   * the layout choices are); absent in a host that keeps nothing, where folds
   * then last only while the answer stands. Read on use.
   */
  storedFolds?(): DiscussFoldState | null;
  keepFolds?(state: DiscussFoldState): void;
  /**
   * `#1842 vibe-talk-discuss-default-bound-since-last-summary`. The covered
   * ranges kept per scope on this device (keyed by the panel's scope key),
   * so a scope's default bound survives a refresh; absent, coverage lasts
   * only while the panel stands.
   */
  storedCoverage?(key: string): DiscussCoverage | null;
  keepCoverage?(key: string, coverage: DiscussCoverage): void;
  /**
   * `#1821 vibe-talk-discuss-state-persists`. Openness changed through the one
   * `setOpen`, so the page can keep it on the device; absent, nothing is kept.
   */
  onOpenChange?(open: boolean): unknown;
  /**
   * `#1501 discuss-read-aloud-parity`. Read one summary aloud through the
   * page's shared read-aloud owner, as the Documents reader does (`#1039`):
   * one call for the whole summary, its blocks as `texts`, an optional
   * from-block for a tap, and the owner's block report for the light.
   * Absent in a host with no owner, where Read and tap-to-read stay inert.
   */
  readDocument?(doc: {
    key: string;
    title?: string;
    texts: readonly string[];
    kinds?: readonly string[];
    onReadingBlock?: (block: number | null, element?: number | null) => unknown;
  }, from?: { part: number; block: number | null }): unknown;
  /** Stop the reading of summary `key`, where that summary's is the one playing; no other. */
  stopSpeaking?(key: string): unknown;
  /** `#1524 discuss-bound-hours-control`. Now, for the hours end of the bound; injected in tests. */
  now?(): number;
}

/**
 * `#1497 discuss-fold-state-persist`. The folds kept for one saved summary:
 * its record (scope plus summary generation) and the open sections' keys.
 * One entry only — a keep for a new summary replaces it whole.
 */
export interface DiscussFoldState {
  record: string;
  open: string[];
}

/** `#1842`. One covered range of one scope: message times [start, end]. */
export interface DiscussCoverageRange {
  start: number;
  end: number;
}

/** `#1842`. The ranges one scope's summaries have covered, oldest first. */
export interface DiscussCoverage {
  ranges: DiscussCoverageRange[];
}

/**
 * `#1501 discuss-read-aloud-parity`. The shared owner's key for one saved
 * summary: its record identity (`#1497`, scope plus `created_ms`), named as
 * a Discuss read so a stop can scope to it alone.
 */
export function discussReadKey(scope: DiscussScope, createdMs: number): string {
  return `discuss:${scope.channelId}|${scope.threadId ?? ""}|${createdMs}`;
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
  /** `#1811`. Re-aim an open panel at `scope`: fresh preview and record, no model call. */
  rescope(scope: DiscussScope): void;
  /** Hide the panel; a late answer for it is then ignored. */
  close(): void;
  isOpen(): boolean;
  /** Rewrite the scope line; nothing else moves. */
  setScopeLabel(label: string): void;
  /** `#1811`. Rewrite scope line and panel title from one scope. */
  setScope(scope: DiscussScope): void;
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
  /** `#1552`. The docked pane's height in force, a viewport percentage. */
  dockedHeight(): number;
  /** `#1552`. Put a docked height on the pane and keep it on the device. */
  setDockedHeight(pct: unknown): void;
  /** `#1344`. Whether the phone sheet stands at full height. */
  sheetFull(): boolean;
  /** `#1344`. Stand the phone sheet at full or half height and keep the choice. */
  setSheetFull(full: boolean): void;
  /** `#1354`. The layout mode in force. */
  mode(): DiscussLayoutMode;
  /** `#1354`. Stand the panel in `mode` and keep the choice on the device. */
  setMode(mode: DiscussLayoutMode): void;
  /**
   * `#1586 discuss-full-drops-to-half-on-link`. Leave full screen so a
   * linked message can land in the list: a full phone sheet stands at
   * half (its kept docked height untouched), a maximized desk panel
   * returns to the mode it was maximized from. Kept on the device like
   * the full-screen toggle keeps it, so the maximize button stands it
   * full again. A no-op at partial or closed.
   */
  dropToPartialForLink(): void;
  /** `#1388` landing 1. Restate the strike from the read set now (a mark landed elsewhere). */
  refreshStrikes(): void;
  /**
   * `#1531 discuss-top-bar-slim-fixed`. Read the summary on screen aloud
   * whole, through the shared owner (the bottom toolbar's Read path).
   * True when a summary was handed to the owner (or its read already
   * stands); false when the panel is closed or has no summary yet.
   */
  readSummary(): boolean;
}

/** Why Discuss cannot work on a deployment with no direct LLM summarizer, said once. */
export const DISCUSS_UNAVAILABLE_HINT = "Discuss needs a direct LLM summarizer ([summaries.llm])";

/** Why Discuss cannot work over Documents (`#1812`): its scope is the channel or thread on screen. */
export const DISCUSS_DOCUMENTS_HINT = "Discuss is not available in Documents";

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

/**
 * `text` with its timestamp citations removed, for the device voice:
 * the Markdown form `[(09:51 ↩)](<address>)` in a raw answer and the
 * plain rendered `(09:51 ↩)` / `(09:51 ↗)` in a drawn block. Mirrors
 * speakable.rs `drop_timestamp_citations` (`#1823`).
 */
export function withoutTimestampCitations(text: string): string {
  return text
    .replace(/\[\(\d{1,2}:\d{2}(?::\d{2})?\s*[↩↗]\)\]\([^)]*\)/g, "")
    .replace(/\(\d{1,2}:\d{2}(?::\d{2})?\s*[↩↗]\)/g, "")
    .replace(/ +([.,;:!?])/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
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
  const empty: CatchupAnswer = { window: null, unread_ids: [], markable_ids: [], cited_ids: [], fresh_ids: [], footer_links: [], text: "", text_html: "", unknown_terms: [], stamp: null };
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
      truncated: rawWindow.truncated === true,
      fetch_blocked: rawWindow.fetch_blocked === true,
    },
    unread_ids: stringsOf(record.unread_ids),
    markable_ids: stringsOf(record.markable_ids),
    cited_ids: stringsOf(record.cited_ids),
    fresh_ids: stringsOf(record.fresh_ids),
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
        truncated: rawWindow.truncated === true,
        fetch_blocked: rawWindow.fetch_blocked === true,
      },
      unread_ids: stringsOf(record.covered_ids),
      markable_ids: stringsOf(record.markable_ids),
      footer_links: links,
      cited_ids: stringsOf(record.cited_ids),
      fresh_ids: stringsOf(record.fresh_ids),
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
      truncated: rawWindow.truncated === true,
      fetch_blocked: rawWindow.fetch_blocked === true,
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

/** The fixed English day names, as `dayLabel` (present/time) keeps them. */
const SUMMARY_MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const SUMMARY_WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/** The most days back a label names by weekday alone, as the rows do (`#729`). */
const SUMMARY_WEEKDAY_DAYS = 6;

/** Calendar days from `at`'s day to `now`'s, from the local fields (a clock-change day is one day). */
function summaryDaysBefore(at: Date, now: Date): number {
  const day = (when: Date): number => Date.UTC(when.getFullYear(), when.getMonth(), when.getDate());
  return Math.round((day(now) - day(at)) / 86_400_000);
}

/**
 * `#1787`. When a summary was made, as its label says it: the message rows'
 * rule (`#729`) — today's is the time alone, one to six days back adds the
 * weekday, anything older names the month and day (and the year when it is
 * not this year) — in the locale time the rows show. Kept here, not imported
 * from present/time: this package may not import it (`web/architecture.mjs`),
 * as `hhmm` above is kept here too. "Nov 16, 3:42 PM", never a bare time of
 * day a previous day's summary could hide behind.
 */
export function summaryTimeLabel(createdMs: number, nowMs: number): string {
  const at = new Date(createdMs);
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const days = summaryDaysBefore(at, new Date(nowMs));
  if (days === 0) return time;
  if (days >= 1 && days <= SUMMARY_WEEKDAY_DAYS) return `${SUMMARY_WEEKDAYS[at.getDay()]} ${time}`;
  const year = at.getFullYear() === new Date(nowMs).getFullYear() ? "" : `, ${at.getFullYear()}`;
  return `${SUMMARY_MONTHS[at.getMonth()]} ${at.getDate()}${year}, ${time}`;
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

/** One hour in milliseconds, for the bound's hours end (`#1524`). */
export const HOUR_MS = 3_600_000;

/**
 * `#1524 discuss-bound-hours-control`. Whole hours from `sinceMs` to `nowMs`,
 * rounded up, at least 1: K is how far back the bound reaches, stated coarsely
 * enough that a small clock move never makes it jitter.
 */
export function hoursSinceStart(sinceMs: number | null, nowMs: number): number {
  if (sinceMs === null || !Number.isFinite(sinceMs) || !Number.isFinite(nowMs)) return 1;
  return Math.max(1, Math.ceil((nowMs - sinceMs) / HOUR_MS));
}

/**
 * `#1817 vibe-talk-summarize-inputs-free-typing`. Hours from `sinceMs` to
 * `nowMs`, floored to one decimal place (6.81 -> 6.8): the dependent-hours
 * display when the count end owns the bound. Never rounded up, so a typed
 * count never pushes the hours past the message it actually reaches.
 */
export function hoursFloor1(sinceMs: number | null, nowMs: number): number {
  if (sinceMs === null || !Number.isFinite(sinceMs) || !Number.isFinite(nowMs)) return 0;
  const raw = (nowMs - sinceMs) / HOUR_MS;
  if (!Number.isFinite(raw) || raw <= 0) return 0;
  return Math.floor(raw * 10) / 10;
}

/** The dependent-hours text: floored to one decimal, always one decimal shown. */
export function formatHoursFloor1(sinceMs: number | null, nowMs: number): string {
  return hoursFloor1(sinceMs, nowMs).toFixed(1);
}

/** How many unread fall at/after `nowMs - hours` (unknown times count as in). */
export function countForHours(unread: readonly CatchupUnreadRef[], hours: number, nowMs: number): number {
  // `#1817`: hours are a free variable as typed (fractions included); no
  // whole-hour truncation and no minimum of 1.
  const typed = Number(hours);
  const start = nowMs - (Number.isFinite(typed) && typed > 0 ? typed : 0) * HOUR_MS;
  let count = 0;
  for (const ref of unread) if (ref.at_ms === null || ref.at_ms >= start) count += 1;
  return count;
}

/**
 * The bound for "last `hours` hours" (`#1817`): the start is `now - K hours`
 * exactly as typed. K is a completely free variable: it is never restated,
 * clamped to the oldest unread, or forced to cover at least one message —
 * a window with no unread in it is a valid bound whose count is 0.
 */
export function boundForHours(
  unread: readonly CatchupUnreadRef[],
  hours: number,
  nowMs: number,
): { count: number; sinceMs: number | null; hours: number } {
  const typed = Number(hours);
  const wanted = Number.isFinite(typed) && typed > 0 ? typed : 1;
  if (unread.length === 0) return { count: 0, sinceMs: null, hours: wanted };
  const start = nowMs - wanted * HOUR_MS;
  return { count: countForHours(unread, wanted, nowMs), sinceMs: start, hours: wanted };
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
  // `#1841 vibe-talk-discuss-title-and-body-match-scope`: one Discuss state
  // per scope. The answer each scope last showed, keyed by channel plus
  // thread (main is its own scope), so switching scope restores that
  // scope's own summary and never another scope's.
  const scopeAnswers = new Map<string, { scope: DiscussScope; answer: CatchupAnswer; readCreatedMs: number }>();

  function scopeKeyOf(scope: DiscussScope): string {
    return `${scope.channelId}\0${scope.threadId ?? ""}`;
  }

  function stashScopeAnswer(): void {
    if (lastScope !== null && lastAnswer !== null) {
      scopeAnswers.set(scopeKeyOf(lastScope), { scope: lastScope, answer: lastAnswer, readCreatedMs });
    }
  }

  /** Blank the body for a scope with nothing of its own yet. */
  function clearScopeBody(): void {
    deps.byId("discuss-answer").textContent = "";
    deps.byId("discuss-footer").hidden = true;
    deps.byId("discuss-stamp").textContent = "";
    deps.byId("discuss-stamp").hidden = true;
    deps.byId("discuss-retry").hidden = true;
    deps.byId("discuss-stale").hidden = true;
    const terms = maybe("discuss-terms");
    if (terms) terms.hidden = true;
    const termsList = maybe("discuss-terms-list");
    if (termsList) termsList.textContent = "";
    const timeline = maybe("discuss-timeline");
    if (timeline) timeline.textContent = "";
  }

  // `#1842`: the covered ranges per scope, in memory over the device's kept
  // copy, so the next default bound starts where the last summary ended.
  const coverageByScope = new Map<string, DiscussCoverage>();

  function coverageKeyOf(scope: DiscussScope): string {
    return `${scope.channelId}:${scope.threadId ?? "main"}`;
  }

  function coverageOf(scope: DiscussScope): DiscussCoverage {
    const key = coverageKeyOf(scope);
    const held = coverageByScope.get(key);
    if (held) return held;
    let stored: DiscussCoverage = { ranges: [] };
    try {
      const raw = deps.storedCoverage?.(key) ?? null;
      if (raw && Array.isArray(raw.ranges)) {
        stored = { ranges: raw.ranges.filter((r) => r && Number.isFinite(r.start) && Number.isFinite(r.end) && r.end >= r.start) };
      }
    } catch { /* a refused read is no coverage */ }
    coverageByScope.set(key, stored);
    return stored;
  }

  function lastCoveredEnd(scope: DiscussScope): number | null {
    const cov = coverageOf(scope);
    return cov.ranges.length ? Math.max(...cov.ranges.map((r) => r.end)) : null;
  }

  /** Record one summary's covered range for its scope and keep it. */
  function recordCoverage(scope: DiscussScope, startMs: number | null, endMs: number): void {
    if (!Number.isFinite(endMs) || endMs <= 0) return;
    const cov = coverageOf(scope);
    const range = { start: startMs !== null && Number.isFinite(startMs) && startMs <= endMs ? startMs : endMs, end: endMs };
    const merged: DiscussCoverageRange[] = [];
    for (const held of [...cov.ranges, range].sort((a, b) => a.start - b.start)) {
      const last = merged.at(-1);
      if (last && held.start <= last.end) last.end = Math.max(last.end, held.end);
      else merged.push({ ...held });
    }
    cov.ranges = merged;
    try { deps.keepCoverage?.(coverageKeyOf(scope), { ranges: merged }); } catch { /* storage refused: memory holds it */ }
  }

  /** Show `scope`'s own stashed answer, or a blank body; true when shown. */
  function showStashedScope(scope: DiscussScope): boolean {
    const cached = scopeAnswers.get(scopeKeyOf(scope)) ?? null;
    lastScope = scope;
    lastAnswer = cached?.answer ?? null;
    readCreatedMs = cached?.readCreatedMs ?? 0;
    if (cached === null) {
      clearScopeBody();
      return false;
    }
    drawAnswer(cached.answer, null);
    drawTerms(cached.answer);
    drawFooter(cached.answer, scope);
    const stampLine = formatStampLine(cached.answer.stamp);
    deps.byId("discuss-stamp").textContent = stampLine;
    deps.byId("discuss-stamp").hidden = stampLine === "";
    deps.byId("discuss-retry").hidden = true;
    deps.byId("discuss-stale").hidden = true;
    return true;
  }
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
  // `#1552 discuss-docked-height-drag`. The docked pane's height, a viewport
  // percentage; the stylesheet's grid row reads it as `--discuss-docked-height`.
  const FALLBACK_DEFAULT_DOCKED_HEIGHT = 50;
  const FALLBACK_MIN_DOCKED_HEIGHT = 20;
  const FALLBACK_MAX_DOCKED_HEIGHT = 85;
  let dockedHeight = FALLBACK_DEFAULT_DOCKED_HEIGHT;
  let heightDrag: { startY: number; startPct: number; moved: boolean } | null = null;
  let sheetFull = false;
  let layoutMode: DiscussLayoutMode = "overlay";
  let drag: { startX: number; startWidth: number } | null = null;
  // `#1497 discuss-fold-state-persist`. The record the drawn summary answers
  // to (its scope and generation, `recordKeyFor`), the sections standing open
  // in it, and the last record a saved draw named — an in-session restore of
  // that same summary answers to it again. Null while a fresh Summarize's
  // answer stands before its saved record lands: all collapsed, nothing kept.
  let foldRecord: string | null = null;
  let foldOpen: ReadonlySet<string> = new Set();
  let lastFoldRecord: string | null = null;
  // `#1501 discuss-read-aloud-parity`. The read standing: its key, its drawn
  // blocks' texts, and the block lit now. Generation 0 until the saved
  // record lands and names the summary's `created_ms`.
  let readCreatedMs = 0;
  let readKey: string | null = null;
  let litElement: HTMLElement | null = null;
  // `#1531`: whether this summary's read stands (started, not yet reported
  // ended), so the toolbar's Read pauses it instead of restarting it.
  let summaryReadActive = false;

  /** A saved summary's record: its scope, as the server keys it, plus its generation. */
  function recordKeyFor(scope: DiscussScope, createdMs: number): string {
    return `${scope.channelId}|${scope.threadId ?? ""}|${createdMs}`;
  }

  /** The folds kept for `record`; a stale or unknown entry is no folds. */
  function keptFoldsFor(record: string): ReadonlySet<string> {
    try {
      const state = deps.storedFolds?.() ?? null;
      if (state === null || state.record !== record) return new Set();
      return new Set(state.open.filter((key) => typeof key === "string"));
    } catch {
      return new Set();
    }
  }

  /** The block each drawn piece is, as the Documents reader walks a body. */
  const READING_BLOCK_TAGS = new Set(["p", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "pre", "table"]);

  /**
   * `#1501`. The answer's drawn blocks in order, walked through folds and
   * lists as `documentBlocks` walks a document: a fold's heading and each
   * block under it are blocks; the collapse bar is not.
   */
  function discussBlocks(box: HTMLElement): HTMLElement[] {
    const found: HTMLElement[] = [];
    const walk = (node: Element): void => {
      for (const kid of Array.from(node.children ?? [])) {
        const el = kid as HTMLElement;
        const tag = String(el.tagName || "").toLowerCase();
        if (el.className === "discuss-collapse-bar") continue;
        if (tag === "li") {
          found.push(el);
          walk(el);
          continue;
        }
        if (READING_BLOCK_TAGS.has(tag)) {
          found.push(el);
          continue;
        }
        walk(el);
      }
    };
    walk(box as unknown as Element);
    return found;
  }

  /** Take the reading light off whichever block holds it. */
  function clearReadingLight(): void {
    if (litElement !== null && typeof litElement.removeAttribute === "function") {
      litElement.removeAttribute("data-reading-block");
    }
    litElement = null;
  }

  /** The owner's block report: light the block being read, kept in view. */
  function onReadingBlock(block: number | null, element?: number | null): void {
    if (block === null) {
      summaryReadActive = false;
      clearReadingLight();
      return;
    }
    let box: HTMLElement;
    try {
      box = deps.byId("discuss-answer");
    } catch {
      return;
    }
    if (!box) return;
    summaryReadActive = true;
    const blocks = discussBlocks(box);
    const named = typeof element === "number" && Number.isInteger(element) ? element : block;
    const at = block >= 0 && block < blocks.length ? block : named;
    clearReadingLight();
    if (at < 0 || at >= blocks.length) return;
    const target = blocks[at] as HTMLElement;
    target.setAttribute("data-reading-block", "current");
    litElement = target;
    if (typeof target.scrollIntoView === "function") target.scrollIntoView({ block: "center" });
  }

  /** Stop this summary's own reading, and only its own. */
  function stopOwnReading(): void {
    summaryReadActive = false;
    if (readKey !== null) deps.stopSpeaking?.(readKey);
    clearReadingLight();
  }

  /**
   * `#1501`. Hand the summary on screen to the shared owner in one call:
   * ALL its drawn blocks (the Documents choice — folds never cut a read),
   * from `fromBlock` where a tap named one.
   */
  function readAloud(fromBlock: number | null = null): void {
    if (!deps.readDocument || lastScope === null || lastAnswer === null) return;
    let box: HTMLElement;
    try {
      box = deps.byId("discuss-answer");
    } catch {
      return;
    }
    const blocks = discussBlocks(box);
    // Kept one per drawn block, never filtered: a tap's index and the
    // light's index are the drawn blocks' own indices.
    // `#1823 vibe-talk-tts-skip-timestamps-and-auto-link-ids`: a drawn
    // block's timestamp citation (`(09:51 ↩)`, `(09:51 ↗)`) is not said,
    // as the server's speech preparation drops it (speakable.rs).
    let texts = blocks.map((el) => withoutTimestampCitations(String(el.textContent ?? "")));
    if (texts.length === 0 || texts.every((text) => text === "")) {
      texts = lastAnswer.text.split(/\n\s*\n/).map((chunk) => withoutTimestampCitations(chunk)).filter((chunk) => chunk !== "");
    }
    if (texts.length === 0) {
      deps.setStatus?.("Nothing to read yet.");
      return;
    }
    summaryReadActive = true;
    readKey = discussReadKey(lastScope, readCreatedMs);
    // `#1505`: the drawn blocks' kinds, in the owner's vocabulary, aligned
    // with `texts` so speak index and highlight index are one index.
    const kindOf = (el: HTMLElement): string => {
      const tag = String(el.tagName || "").toLowerCase();
      if (tag === "li") return "item";
      if (tag === "blockquote") return "quote";
      if (tag === "pre") return "code";
      if (tag === "table") return "table";
      return tag.startsWith("h") ? "heading" : "paragraph";
    };
    const kinds = blocks.length === texts.length ? blocks.map(kindOf) : undefined;
    const doc = { key: readKey, title: lastScope.label, texts, ...(kinds ? { kinds } : {}), onReadingBlock };
    void Promise.resolve(
      fromBlock === null ? deps.readDocument(doc) : deps.readDocument(doc, { part: 0, block: fromBlock }),
    ).catch(() => undefined);
  }

  /**
   * `#1531 discuss-top-bar-slim-fixed`. The bottom toolbar's Read, for the
   * summary on screen: the top bar's old one-tap whole-summary read, now
   * on the page's own reader controls (Stop and pace are the shared
   * owner's, beside it). No summary, a closed panel, this summary already
   * reading, or another read standing (its strip up) starts nothing.
   */
  function readSummary(): boolean {
    if (!isOpen() || lastScope === null || lastAnswer === null || !deps.readDocument) return false;
    if (summaryReadActive) return true;
    const steer = maybe("read-steer");
    if (steer !== null && !steer.hidden) return false;
    readAloud();
    return summaryReadActive;
  }

  /** Keep the answer box's open sections for the record they stand in. */
  function keepFoldsNow(box: HTMLElement): void {
    if (foldRecord === null) return;
    foldOpen = new Set(openSectionKeys(box as never));
    try {
      deps.keepFolds?.({ record: foldRecord, open: [...foldOpen] });
    } catch {
      // Storage refused: the folds hold until the answer is redrawn.
    }
  }

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

  /** `#1811`. Name the scope in the panel title; a host without one skips it. */
  function setTitle(scope: DiscussScope): void {
    const title = maybe("discuss-title");
    if (title) title.textContent = discussTitle(scope);
  }

  function clampWidth(value: unknown): number {
    if (deps.clampWidth) return deps.clampWidth(value);
    const px = Math.round(Number(value));
    return Number.isFinite(px) ? Math.min(FALLBACK_MAX_WIDTH, Math.max(FALLBACK_MIN_WIDTH, px)) : FALLBACK_DEFAULT_WIDTH;
  }

  function defaultWidth(): number {
    return deps.defaultWidth ?? FALLBACK_DEFAULT_WIDTH;
  }

  function clampDockedHeight(value: unknown): number {
    if (deps.clampDockedHeight) return deps.clampDockedHeight(value);
    const pct = Math.round(Number(value));
    return Number.isFinite(pct)
      ? Math.min(FALLBACK_MAX_DOCKED_HEIGHT, Math.max(FALLBACK_MIN_DOCKED_HEIGHT, pct))
      : FALLBACK_DEFAULT_DOCKED_HEIGHT;
  }

  function defaultDockedHeight(): number {
    return deps.defaultDockedHeight ?? FALLBACK_DEFAULT_DOCKED_HEIGHT;
  }

  /** Snap near half and near the ends, on release only so a drag never jumps. */
  function snapDockedHeight(pct: number): number {
    const clamped = clampDockedHeight(pct);
    if (Math.abs(clamped - FALLBACK_DEFAULT_DOCKED_HEIGHT) <= 3) return FALLBACK_DEFAULT_DOCKED_HEIGHT;
    if (clamped - FALLBACK_MIN_DOCKED_HEIGHT <= 2) return FALLBACK_MIN_DOCKED_HEIGHT;
    if (FALLBACK_MAX_DOCKED_HEIGHT - clamped <= 2) return FALLBACK_MAX_DOCKED_HEIGHT;
    return clamped;
  }

  /** The viewport height a percentage is taken of; a phone's 844 where unknown. */
  function viewportHeight(): number {
    try {
      const h = document.documentElement?.clientHeight ?? 0;
      return h > 0 ? h : 844;
    } catch {
      return 844;
    }
  }

  /**
   * `#1552`. Keep/read the docked height where the page keeps nothing handed
   * in: the same device key the device setting owns, read directly, so the
   * choice still sticks across a reload. A refused store holds for the session.
   */
  const DOCKED_HEIGHT_KEY = "vibe-talk.voice.discuss-docked-height";
  function fallbackStoredDockedHeight(): number | undefined {
    if (deps.storedDockedHeight) return undefined;
    try {
      const raw = (globalThis as { localStorage?: { getItem(k: string): string | null } }).localStorage?.getItem(DOCKED_HEIGHT_KEY);
      return raw === null || raw === undefined ? undefined : clampDockedHeight(raw);
    } catch {
      return undefined;
    }
  }
  function fallbackKeepDockedHeight(): void {
    if (deps.keepDockedHeight) {
      deps.keepDockedHeight(dockedHeight);
      return;
    }
    try {
      (globalThis as { localStorage?: { setItem(k: string, v: string): void } }).localStorage?.setItem(DOCKED_HEIGHT_KEY, String(dockedHeight));
    } catch {
      // Storage refused: the height holds until a reload.
    }
  }

  /** Put `pct` on the pane as `--discuss-docked-height`; kept only when `keep`. */
  function applyDockedHeight(pct: unknown, keep: boolean): number {
    dockedHeight = clampDockedHeight(pct);
    const value = `${dockedHeight}dvh`;
    const style = (panel() as HTMLElement).style as { setProperty?: (n: string, v: string) => void } | undefined;
    if (style && typeof style.setProperty === "function") style.setProperty("--discuss-docked-height", value);
    // `#1813`: the channel pane's foot (22-discuss.css) reads the height at the root.
    document.documentElement?.style.setProperty("--discuss-docked-height", value);
    const screen = maybe("screen-main");
    const screenStyle = screen ? (screen as HTMLElement).style as { setProperty?: (n: string, v: string) => void } | undefined : undefined;
    if (screenStyle && typeof screenStyle.setProperty === "function") screenStyle.setProperty("--discuss-docked-height", value);
    const head = maybe("discuss-head");
    if (head) {
      head.setAttribute("aria-valuenow", String(dockedHeight));
      head.setAttribute("aria-valuetext", `Discuss pane ${dockedHeight} percent of the screen high`);
    }
    if (keep) {
      try {
        fallbackKeepDockedHeight();
      } catch {
        // Storage refused: the height holds until a reload.
      }
    }
    return dockedHeight;
  }

  /** Put `px` on the panel as `--discuss-width`; kept on the device only when `keep`. */
  function applyWidth(px: unknown, keep: boolean): number {
    width = clampWidth(px);
    const style = (panel() as HTMLElement).style as { setProperty?: (name: string, value: string) => void } | undefined;
    if (style && typeof style.setProperty === "function") style.setProperty("--discuss-width", `${width}px`);
    // `#1813`: the channel pane's end (22-discuss.css) reads the width at the root, where
    // the overlays anchored to #frame-body can inherit it.
    document.documentElement?.style.setProperty("--discuss-width", `${width}px`);
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

  /**
   * `#1586`. The one exit from full screen for a message link, shared by
   * every staying jump: the sheet half stands in the #1526 docked row at
   * the height already on the pane (`--discuss-docked-height`, #1552),
   * and a maximized panel exits as its full-screen button exits
   * (`beforeFull`). Runs synchronously on the tap, before the jump, so
   * the landing's scroll-into-view is measured at the partial height.
   */
  function dropToPartialForLink(): void {
    if (!isOpen()) return;
    if (sheetFull) applySheet(false, true);
    if (layoutMode === "maximized") applyMode(beforeFull, true);
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
    let storedDocked: number | undefined;
    try {
      storedDocked = deps.storedDockedHeight?.() ?? fallbackStoredDockedHeight();
    } catch {
      storedDocked = undefined;
    }
    applyDockedHeight(storedDocked === undefined ? defaultDockedHeight() : storedDocked, false);
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
    // `#1821`: every open and close is kept on the device from this one place.
    try {
      deps.onOpenChange?.(open);
    } catch {
      // Storage refused: openness holds until a reload, as the layout does.
    }
  }

  /**
   * `#1388` landing 1. Restate the strike over the drawn answer from the
   * page's read set now: a bullet or section whose cited messages are all
   * read is struck (still readable, tappable, foldable); an uncited bullet
   * never is. Class words only — read-aloud still reads struck bullets.
   */
  function restrike(): void {
    if (lastAnswer === null) return;
    const isRead = (id: string): boolean => {
      try {
        return deps.isReadNow?.(id) === true;
      } catch {
        return false;
      }
    };
    try {
      applyStrikes(deps.byId("discuss-answer") as never, isRead, new Set(lastAnswer.fresh_ids));
    } catch {
      // No answer box in this host: nothing to strike.
    }
  }

  /**
   * Draw one answer (`discuss-markdown-render`): the server's sanitized render as
   * markup when it sent one, the shaped text as text otherwise. Model text reaches
   * innerHTML only through `text_html` — never directly.
   */
  function drawAnswer(answer: CatchupAnswer, record: string | null): void {
    // `#1497`: a different record than the one standing starts from its own
    // kept folds (none for a fresh summary); the same record redrawn keeps
    // the folds as they stand.
    if (record !== foldRecord) {
      foldRecord = record;
      foldOpen = record === null ? new Set() : keptFoldsFor(record);
    }
    if (record !== null) lastFoldRecord = record;
    const box = deps.byId("discuss-answer");
    box.replaceChildren();
    if (answer.text_html !== "") {
      setClassWord(box, "discuss-answer--plain", false);
      insertSanitizedHtml(box, answer.text_html);
      // `#1387 discuss-collapsible-sections`: workstream sections fold, all
      // collapsed, with one Expand/Collapse-all bar at the top of the answer.
      const folds = foldSectionsByHeading(box as never, (tag) => document.createElement(tag) as never, foldOpen);
      if (folds > 0) {
        for (const kid of Array.from(box.children)) {
          if ((kid as HTMLElement).tagName === "DETAILS") {
            kid.addEventListener("toggle", () => keepFoldsNow(box));
          }
        }
        const bar = document.createElement("div");
        bar.className = "discuss-collapse-bar";
        const expand = document.createElement("button");
        expand.type = "button";
        expand.textContent = "Expand all";
        expand.addEventListener("click", () => {
          setAllSections(box as never, true);
          keepFoldsNow(box);
        });
        const collapse = document.createElement("button");
        collapse.type = "button";
        collapse.textContent = "Collapse all";
        collapse.addEventListener("click", () => {
          setAllSections(box as never, false);
          keepFoldsNow(box);
        });
        bar.append(expand, collapse);
        box.prepend(bar);
      }
    } else {
      setClassWord(box, "discuss-answer--plain", true);
      box.textContent = answer.text;
    }
    // `#1388` landing 1: the strike stands from this draw (a plain draw has
    // no citation links to map, so nothing is struck there).
    restrike();
  }

  /**
   * `#1787`. While a replacement is read, the standing terms are the previous
   * summary's: dim them (the stylesheet's `data-stale` rule) and say so in
   * their heading, so they never present as the new answer's. The next
   * `drawTerms` — the new answer's, or the restored one's — clears the mark.
   */
  function markTermsStale(): void {
    const box = maybe("discuss-terms");
    if (box === null || box.hidden) return;
    box.setAttribute("data-stale", "true");
    const summary = maybe("discuss-terms-summary");
    if (summary !== null && !summary.textContent.includes("previous summary")) {
      summary.textContent = `${summary.textContent} · from the previous summary`;
    }
  }

  /** The terms the answer did not understand, collapsed (`#1351`). */
  function drawTerms(answer: CatchupAnswer): void {
    const box = deps.byId("discuss-terms");
    box.removeAttribute("data-stale");
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
            else {
              dropToPartialForLink();
              deps.onJump(linkRef);
            }
          }
        }
        return;
      }
      if (el === (deps.byId("discuss-answer") as unknown)) break;
      node = (el as { parentNode?: unknown }).parentNode;
    }
    // `#1501 discuss-read-aloud-parity`. No citation took the tap: a tap on
    // a drawn block reads from that block only while this summary's read is
    // already active (`#1543`); an inactive tap selects or does nothing. A
    // fold's summary (its tap folds) and the collapse bar keep their own acts.
    if (!summaryReadActive || !deps.readDocument || lastScope === null || lastAnswer === null) return;
    let box: HTMLElement;
    try {
      box = deps.byId("discuss-answer");
    } catch {
      return;
    }
    let nodeUp: unknown = event.target;
    while (nodeUp !== null && nodeUp !== undefined && nodeUp !== (box as unknown)) {
      const elUp = nodeUp as HTMLElement;
      const tagUp = typeof elUp.tagName === "string" ? elUp.tagName.toLowerCase() : "";
      if (tagUp === "summary" || elUp.className === "discuss-collapse-bar") return;
      nodeUp = (elUp as { parentNode?: unknown }).parentNode;
    }
    const blocks = discussBlocks(box);
    let tapped: unknown = event.target;
    while (tapped !== null && tapped !== undefined && tapped !== (box as unknown)) {
      const at = blocks.indexOf(tapped as HTMLElement);
      if (at >= 0) {
        readAloud(at);
        return;
      }
      tapped = (tapped as { parentNode?: unknown }).parentNode;
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
        summary.textContent = `Earlier summary · ${entry.created_ms > 0 ? summaryTimeLabel(entry.created_ms, nowMs()) : "—"} · ${count} unread`;
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

  /**
   * One incremental Update (`#1388` landing 2): the banner's button. One
   * delta call over only the new messages; the merged summary is drawn with
   * its new parts green (landing 1's restrike). A host without the Update
   * route falls back to a full Summarize, as before landing 2.
   */
  function updateFor(scope: DiscussScope): void {
    if (pending) return;
    if (!deps.catchupUpdate) {
      fetchFor(scope, null);
      return;
    }
    generation += 1;
    const asked = generation;
    lastScope = scope;
    deps.byId("discuss-scope").textContent = scope.label;
    setTitle(scope);
    deps.byId("discuss-status").textContent = "Updating…";
    deps.byId("discuss-retry").hidden = true;
    // `#1787`: the standing summary's terms are the old one's while it updates.
    markTermsStale();
    setPending(true);
    void deps.catchupUpdate(scope.channelId, scope).then(
      (value) => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        const answer = asCatchupAnswer(value);
        lastAnswer = answer;
        scopeAnswers.set(scopeKeyOf(scope), { scope, answer, readCreatedMs });
        deps.byId("discuss-status").textContent = "Updated.";
        drawAnswer(answer, null);
        drawTerms(answer);
        drawFooter(answer, scope);
        const stampLine = formatStampLine(answer.stamp);
        deps.byId("discuss-stamp").textContent = stampLine;
        deps.byId("discuss-stamp").hidden = stampLine === "";
        recordFor(scope, true);
      },
      (error: unknown) => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        // `#1787`: the old summary stands again, its terms current once more.
        if (lastAnswer !== null) drawTerms(lastAnswer);
        if (isDiscussUnavailable(error)) {
          deps.byId("discuss-status").textContent = DISCUSS_UNAVAILABLE_HINT;
          deps.showError?.(DISCUSS_UNAVAILABLE_HINT);
          return;
        }
        const message = error instanceof Error ? error.message : String(error);
        deps.byId("discuss-status").textContent = message;
        deps.showError?.(message);
      },
    );
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
          readCreatedMs = shape.created_ms;
          scopeAnswers.set(scopeKeyOf(scope), { scope, answer: shape.summary, readCreatedMs });
          // `#1842`: a saved record seeds the scope's covered end where this
          // device kept none, so the default still starts at the last summary.
          if (coverageOf(scope).ranges.length === 0 && shape.created_ms > 0) {
            recordCoverage(scope, null, shape.created_ms);
          }
          drawAnswer(shape.summary, recordKeyFor(scope, shape.created_ms));
          drawTerms(shape.summary);
          drawFooter(shape.summary, scope);
          const stampLine = formatStampLine(shape.summary.stamp);
          deps.byId("discuss-stamp").textContent = stampLine;
          deps.byId("discuss-stamp").hidden = stampLine === "";
          if (!quiet) {
            deps.byId("discuss-status").textContent =
              `Saved summary · ${shape.created_ms > 0 ? summaryTimeLabel(shape.created_ms, nowMs()) : "—"}`;
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
    // `#1540 discuss-footer-inline-links`: the uncited remainder reads as
    // one dense comma-separated line that wraps like text, each time
    // still its own jump button — not one full-width button per time.
    const visible = answer.footer_links.filter((link) => !cited.has(link.id));
    visible.forEach((link, index) => {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "discuss-link";
      button.textContent = link.label.split(" · ")[0] ?? link.label;
      listening.listen(button, "click", () => {
        dropToPartialForLink();
        deps.onJump({
          service: deps.serviceName(scope.channelId),
          channel: scope.channelId,
          message: link.id,
          thread: link.thread_id,
        });
      });
      item.appendChild(button);
      if (index < visible.length - 1) item.appendChild(document.createTextNode(", "));
      list.appendChild(item);
    });
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
  let previewWindowCount = 0;
  let boundCount = 0;
  // `#1524`: the bound's explicit start when set by the hours end; null means
  // the count end owns it and the start is that count's oldest message.
  let boundStartMs: number | null = null;
  // `#1817`: which end owns the bound ("count" | "hours" | "since"), the
  // hours exactly as typed when the hours end owns it, and the bound input
  // being edited right now. While an input is being edited its text is the
  // owner's and is never rewritten; only the dependent end follows.
  let boundOwner: "count" | "hours" | "since" | null = null;
  let boundHours: number | null = null;
  let editingBoundInput: HTMLInputElement | null = null;
  let drawingBound = false;
  let lastSinceMs: number | null = null;
  let pending = false;

  function nowMs(): number {
    return deps.now ? deps.now() : Date.now();
  }

  /** The bound as drawn: count, start and hours, from the one state. */
  function currentBound(): { count: number; sinceMs: number | null; hours: number } {
    if (boundStartMs !== null) {
      let count = 0;
      for (const ref of previewUnread) if (ref.at_ms === null || ref.at_ms >= boundStartMs) count += 1;
      // `#1817`: an hours window covering no unread is a valid bound (count
      // 0); it is never snapped to the newest unread. Hours as typed win;
      // a start set by the clock end restates floored to one decimal.
      return {
        count,
        sinceMs: boundStartMs,
        hours: boundOwner === "hours" && boundHours !== null ? boundHours : hoursFloor1(boundStartMs, nowMs()),
      };
    }
    const bound = boundForCount(previewUnread, boundCount || previewUnread.length);
    return { count: bound.count, sinceMs: bound.sinceMs, hours: hoursFloor1(bound.sinceMs, nowMs()) };
  }

  /** Say whether a request is in flight, here and on the page's entry points. */
  function setPending(on: boolean): void {
    pending = on;
    deps.onBusy?.(on);
    drawBound();
  }

  /** Draw the Summarize bound: its button, and the three linked ends of it. */
  function drawBound(): void {
    const button = deps.byId("discuss-summarize") as HTMLButtonElement;
    const count = deps.byId("discuss-count") as HTMLInputElement;
    const hours = deps.byId("discuss-hours") as HTMLInputElement;
    const since = deps.byId("discuss-since") as HTMLInputElement;
    const none = previewUnread.length === 0;
    // `#1817`: the count/hours ends stay editable with nothing unread, so
    // any hour value can be typed before (or without) messages in range.
    count.disabled = pending;
    hours.disabled = pending;
    since.disabled = pending || none;
    if (none) {
      button.disabled = true;
      button.textContent = "Summarize";
      drawingBound = true;
      try {
        if (editingBoundInput !== count) count.value = boundOwner === "count" ? String(boundCount) : "0";
        if (editingBoundInput !== hours) {
          hours.value = boundOwner === "hours" && boundHours !== null ? String(boundHours) : "";
        }
        if (editingBoundInput !== since) since.value = "";
      } finally {
        drawingBound = false;
      }
      return;
    }
    const bound = currentBound();
    if (boundStartMs !== null) boundStartMs = bound.sinceMs;
    button.disabled = pending || bound.count === 0;
    drawingBound = true;
    try {
      // `#1817`: never rewrite the field being typed in (that rewrite was
      // the 1->2 jump); only the dependent ends follow. The owning end
      // shows its value exactly as committed, the other end follows.
      if (editingBoundInput !== count) {
        count.value = boundOwner === "count" ? String(boundCount) : String(bound.count);
      }
      if (editingBoundInput !== hours) {
        hours.value = boundOwner === "hours" && boundHours !== null
          ? String(boundHours)
          : formatHoursFloor1(bound.sinceMs, nowMs());
      }
      if (editingBoundInput !== since) {
        since.value = bound.sinceMs === null ? "" : hhmm(bound.sinceMs);
      }
    } finally {
      drawingBound = false;
    }
    // `#1382`: the button says only "Summarize"; the line beside it carries the bound.
    button.textContent = "Summarize";
  }

  /** Read the scope's preview and offer its bound; no model is asked. */
  function previewFor(scope: DiscussScope, again = false): void {
    generation += 1;
    const asked = generation;
    lastScope = scope;
    deps.byId("discuss-scope").textContent = scope.label;
    setTitle(scope);
    if (!again) deps.byId("discuss-status").textContent = "Checking what to summarize…";
    void deps.catchupPreview(scope.channelId, scope).then(
      (value) => {
        if (asked !== generation || !isOpen()) return;
        const preview = asCatchupPreview(value);
        previewUnread = preview.unread;
        previewWindowCount = preview.window?.count ?? preview.unread.length;
        boundCount = preview.unread.length;
        boundStartMs = null;
        boundOwner = null;
        boundHours = null;
        // `#1842`: with a prior summary in this scope, the default bound
        // starts where it ended, so repeated Summaries are contiguous.
        const coveredEnd = lastCoveredEnd(scope);
        if (coveredEnd !== null && preview.unread.length > 0) {
          boundStartMs = coveredEnd;
          boundOwner = "since";
        }
        // `#1761`: a thread whose older pages are still being fetched polls
        // again — each preview fetches one more page — so the held count on
        // screen grows toward the whole thread before Summarize is offered.
        if (scope.threadId !== null && preview.window?.truncated === true && preview.window.fetch_blocked !== true) {
          deps.byId("discuss-status").textContent = `Fetching older messages… ${previewWindowCount} held so far.`;
          drawBound();
          previewFor(scope, true);
          return;
        }
        if (previewUnread.length === 0) {
          deps.byId("discuss-status").textContent = "Nothing unread here.";
        } else if (preview.window?.fetch_blocked === true) {
          deps.byId("discuss-status").textContent = `${previewUnread.length} unread ready. Older messages could not be fetched, so a summary covers only what is held.`;
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
    // `#1501`: a new Summarize stops the summary it replaces, by its key.
    stopOwnReading();
    // `#1787`: the summary being replaced — kept whole so a failed ask puts
    // it back exactly as it stood, terms included.
    const previous = lastAnswer;
    const previousReadMs = readCreatedMs;
    const staleWasHidden = deps.byId("discuss-stale").hidden;
    readCreatedMs = 0;
    lastSinceMs = sinceMs;
    generation += 1;
    const asked = generation;
    lastScope = scope;
    lastAnswer = null;
    deps.byId("discuss-scope").textContent = scope.label;
    setTitle(scope);
    deps.byId("discuss-status").textContent = "Reading…";
    deps.byId("discuss-answer").textContent = "";
    deps.byId("discuss-footer").hidden = true;
    deps.byId("discuss-stamp").textContent = "";
    deps.byId("discuss-stamp").hidden = true;
    deps.byId("discuss-retry").hidden = true;
    // `#1787`: nothing of the old answer presents as current while its
    // replacement is read — the banner waits, the terms stand marked.
    deps.byId("discuss-stale").hidden = true;
    markTermsStale();
    setPending(true);
    const body: CatchupRequestBody = { thread_id: scope.threadId, view: scope.view, since_ms: sinceMs };
    void deps.catchupPost(scope.channelId, body).then(
      (value) => {
        setPending(false);
        if (asked !== generation || !isOpen()) return;
        const answer = asCatchupAnswer(value);
        lastAnswer = answer;
        scopeAnswers.set(scopeKeyOf(scope), { scope, answer, readCreatedMs });
        const coveredTimes = previewUnread.filter((ref) => answer.unread_ids.includes(ref.id) && ref.at_ms !== null).map((ref) => ref.at_ms as number);
        recordCoverage(scope, sinceMs ?? (coveredTimes.length ? Math.min(...coveredTimes) : null), coveredTimes.length ? Math.max(...coveredTimes) : nowMs());
        const unread = answer.window?.unread_count ?? answer.unread_ids.length;
        const count = answer.window?.count ?? answer.unread_ids.length;
        // `#1761`: a window cut short of the thread's start says so, visibly —
        // never a partial window presented as the whole.
        deps.byId("discuss-status").textContent = answer.window?.truncated === true
          ? `Read ${unread} unread of ${count}. Older messages are not covered: fetching them did not finish.`
          : `Read ${unread} unread of ${count}`;
        drawAnswer(answer, null);
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
        // `#1787`: a failed ask changes nothing — the summary it would have
        // replaced stands again whole, its terms unmarked.
        if (previous !== null) {
          lastAnswer = previous;
          readCreatedMs = previousReadMs;
          drawAnswer(previous, foldRecord);
          drawTerms(previous);
          drawFooter(previous, scope);
          const stampLine = formatStampLine(previous.stamp);
          deps.byId("discuss-stamp").textContent = stampLine;
          deps.byId("discuss-stamp").hidden = stampLine === "";
          deps.byId("discuss-stale").hidden = staleWasHidden;
        }
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
      // `#1388` landing 1: the Discuss mark buttons move the strike at once.
      restrike();
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
          stopOwnReading();
          if (pending) setPending(false);
          setOpen(false);
        });
        // `#1531 discuss-top-bar-slim-fixed`. No Read in the top bar: the
        // bottom toolbar's Read (`#read-aloud`) reads the summary on
        // screen whole when Discuss stands open with one; its own
        // Pause/Stop and pace stay the shared owner's (`#1501`).
        const toolbarRead = maybe("read-aloud");
        if (toolbarRead) {
          listening.listen(toolbarRead, "click", () => {
            readSummary();
          });
        }
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
          updateFor(lastScope);
        });
        listening.listen(deps.byId("discuss-retry"), "click", () => {
          if (lastScope !== null) fetchFor(lastScope, lastSinceMs);
        });
        listening.listen(deps.byId("discuss-summarize"), "click", () => {
          if (pending || lastScope === null || previewUnread.length === 0) return;
          if (currentBound().count === 0) return;
          fetchFor(lastScope, currentBound().sinceMs);
        });
        // `#1817`: typing commits on input for the dependent end only; the
        // field being typed in keeps the owner's raw text and caret. Blur
        // ends the edit, and only then may the field be redrawn (an empty
        // or invalid text falls back to the bound). Arrow keys step the
        // field natively and arrive here as ordinary input events.
        const countInput = deps.byId("discuss-count") as HTMLInputElement;
        const hoursInput = deps.byId("discuss-hours") as HTMLInputElement;
        const sinceInput = deps.byId("discuss-since") as HTMLInputElement;
        for (const input of [countInput, hoursInput, sinceInput]) {
          listening.listen(input, "focus", () => { editingBoundInput = input; });
          listening.listen(input, "blur", () => {
            if (editingBoundInput === input) editingBoundInput = null;
            drawBound();
          });
        }
        listening.listen(countInput, "input", () => {
          if (drawingBound) return;
          editingBoundInput = countInput;
          const parsed = Number(countInput.value);
          if (countInput.value.trim() === "" || !Number.isFinite(parsed) || parsed < 0) return;
          boundOwner = "count";
          boundHours = null;
          boundCount = Math.trunc(parsed);
          boundStartMs = boundForCount(previewUnread, boundCount).sinceMs;
          drawBound();
        });
        listening.listen(hoursInput, "input", () => {
          if (drawingBound) return;
          editingBoundInput = hoursInput;
          const parsed = Number(hoursInput.value);
          if (hoursInput.value.trim() === "" || !Number.isFinite(parsed) || parsed <= 0) return;
          const bound = boundForHours(previewUnread, parsed, nowMs());
          boundOwner = "hours";
          boundHours = parsed;
          boundCount = bound.count;
          boundStartMs = bound.sinceMs;
          drawBound();
        });
        listening.listen(sinceInput, "input", () => {
          if (drawingBound) return;
          editingBoundInput = sinceInput;
          boundOwner = "since";
          boundHours = null;
          boundCount = countSince(previewUnread, sinceInput.value);
          boundStartMs = boundForCount(previewUnread, boundCount).sinceMs;
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
          stopOwnReading();
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
        // `#1552 discuss-docked-height-drag`: the "Discuss agent" title bar
        // drags the docked (half-height) pane up and down. Pointer events
        // cover mouse, touch and pen; a press on a bar button never starts
        // a drag, and movement under 6px is a tap, not a drag. Capture is
        // taken only once the drag is real, so a tap still clicks its button.
        const head = maybe("discuss-head");
        if (head) {
          const isButton = (target: unknown): boolean => {
            const el = target as { tagName?: string; closest?: (s: string) => unknown } | null;
            if (!el) return false;
            if (String(el.tagName || "").toUpperCase() === "BUTTON") return true;
            try {
              return typeof el.closest === "function" && el.closest("button") !== null;
            } catch {
              return false;
            }
          };
          listening.listen(head, "pointerdown", (event: PointerEvent) => {
            if (sheetFull || isButton(event.target)) return;
            heightDrag = { startY: event.clientY, startPct: dockedHeight, moved: false };
          });
          listening.listen(head, "pointermove", (event: PointerEvent) => {
            if (!heightDrag) return;
            const dy = heightDrag.startY - event.clientY;
            if (!heightDrag.moved && Math.abs(dy) < 6) return;
            if (!heightDrag.moved) {
              heightDrag.moved = true;
              const holder = head as HTMLElement & { setPointerCapture?: (id: number) => void };
              if (typeof holder.setPointerCapture === "function" && event.pointerId !== undefined) {
                try {
                  holder.setPointerCapture(event.pointerId);
                } catch {
                  // No pointer to hold: move/up on the bar still follow.
                }
              }
            }
            if (typeof event.preventDefault === "function") event.preventDefault();
            const vh = viewportHeight();
            applyDockedHeight(heightDrag.startPct + (dy / vh) * 100, false);
          });
          const endHeightDrag = (): void => {
            if (!heightDrag) return;
            const moved = heightDrag.moved;
            heightDrag = null;
            if (moved) applyDockedHeight(snapDockedHeight(dockedHeight), true);
          };
          listening.listen(head, "pointerup", endHeightDrag);
          listening.listen(head, "pointercancel", endHeightDrag);
          listening.listen(head, "keydown", (event: KeyboardEvent) => {
            const step = event.key === "ArrowUp" ? 2 : event.key === "ArrowDown" ? -2
              : event.key === "PageUp" ? 10 : event.key === "PageDown" ? -10 : null;
            if (step !== null) {
              if (typeof event.preventDefault === "function") event.preventDefault();
              applyDockedHeight(dockedHeight + step, true);
            } else if (event.key === "Home") {
              if (typeof event.preventDefault === "function") event.preventDefault();
              applyDockedHeight(FALLBACK_MIN_DOCKED_HEIGHT, true);
            } else if (event.key === "End") {
              if (typeof event.preventDefault === "function") event.preventDefault();
              applyDockedHeight(FALLBACK_MAX_DOCKED_HEIGHT, true);
            }
          });
          listening.listen(head, "dblclick", (event: Event) => {
            if (isButton((event as PointerEvent).target)) return;
            applyDockedHeight(defaultDockedHeight(), true);
          });
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
      // `#1841`: opening for another scope switches windows, as a rescope
      // does — the old scope's body never stands under the new title.
      if (lastScope === null || scopeKeyOf(lastScope) !== scopeKeyOf(scope)) {
        stashScopeAnswer();
        showStashedScope(scope);
      }
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
      stashScopeAnswer();
      generation += 1;
      stopOwnReading();
      if (pending) setPending(false);
      setOpen(false);
    },
    isOpen,
    setScopeLabel(label: string) {
      deps.byId("discuss-scope").textContent = label;
    },
    setScope(scope: DiscussScope) {
      deps.byId("discuss-scope").textContent = scope.label;
      setTitle(scope);
    },
    rescope(scope: DiscussScope) {
      // `#1841`: switching scope is switching Discuss windows. Stash the
      // outgoing scope's state, then show the incoming scope's own — its
      // stashed summary, or a blank body the preview and saved record may
      // fill for this scope only. `previewFor` bumps the generation, so a
      // late answer for the outgoing scope is dropped, never drawn here.
      if (lastScope !== null && scopeKeyOf(lastScope) === scopeKeyOf(scope)) {
        previewFor(scope);
        recordFor(scope);
        return;
      }
      stashScopeAnswer();
      stopOwnReading();
      if (pending) setPending(false);
      showStashedScope(scope);
      previewFor(scope);
      recordFor(scope);
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
      // `#1497`: the restored summary answers to the last named record only
      // where that record was for this scope (read before the scope moves).
      const restoredRecord = lastScope !== null && lastScope.channelId === scope.channelId && lastScope.threadId === scope.threadId
        ? lastFoldRecord
        : null;
      stashScopeAnswer();
      lastScope = scope;
      lastAnswer = answer;
      scopeAnswers.set(scopeKeyOf(scope), { scope, answer, readCreatedMs });
      try {
        applyStoredLayout();
      } catch {
        // No panel in this host yet: the open below says so, as before.
      }
      setOpen(true);
      deps.byId("discuss-scope").textContent = scope.label;
      setTitle(scope);
      const unread = answer.window?.unread_count ?? answer.unread_ids.length;
      const count = answer.window?.count ?? answer.unread_ids.length;
      deps.byId("discuss-status").textContent = `Read ${unread} unread of ${count}`;
      drawAnswer(answer, restoredRecord);
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
    dockedHeight: () => dockedHeight,
    setDockedHeight(pct: unknown) {
      applyDockedHeight(pct, true);
    },
    sheetFull: () => sheetFull,
    setSheetFull(full: boolean) {
      applySheet(full, true);
    },
    mode: () => layoutMode,
    setMode(mode: DiscussLayoutMode) {
      applyMode(mode, true);
    },
    dropToPartialForLink,
    refreshStrikes() {
      restrike();
    },
    readSummary,
  };
}

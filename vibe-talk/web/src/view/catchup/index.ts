// view/catchup (`catchup-side-agent`, slice 1): Discuss — a bar button and an overlay
// panel that ask the server for a one-shot catch-up over the channel or thread on
// screen. Its public interface; the page constructs it beside the Documents view,
// tracks `mountControls()` with the boot, and calls `sync()` from `showView`.
// Construction is inert: no element is looked up, no listener is added and nothing is
// fetched until the controls are mounted and the panel opened.

export { DISCUSS_DOCUMENTS_HINT, DISCUSS_UNAVAILABLE_HINT, HOUR_MS, asCatchupAnswer, asCatchupPreview, asCatchupRecord, boundForCount, boundForHours, countForHours, countSince, createDiscussPanel, discussReadKey, discussTitle, formatHoursFloor1, formatStampLine, hhmm, hoursFloor1, hoursSinceStart, isDiscussUnavailable, summaryTimeLabel, withoutTimestampCitations } from "./panel.ts";
export { FRESH_CLASS, STRUCK_BULLET_CLASS, STRUCK_SECTION_CLASS, applyStrikes, citationIdsOf, hasWord } from "./strikes.ts";
export type { StrikeElement } from "./strikes.ts";
export type {
  CatchupAnswer,
  CatchupFooterLink,
  CatchupPreview,
  CatchupRecordShape,
  CatchupRequestBody,
  CatchupStamp,
  CatchupUnreadRef,
  CatchupWindow,
  DiscussCoverage,
  DiscussCoverageRange,
  DiscussFoldState,
  DiscussLayoutMode,
  DiscussPanel,
  DiscussPanelDependencies,
  DiscussScope,
} from "./panel.ts";

import { createListening } from "../dom/index.ts";
import type { MessageLinkRef } from "../../foundation/index.ts";
import { DISCUSS_DOCUMENTS_HINT, DISCUSS_UNAVAILABLE_HINT, createDiscussPanel, isDiscussUnavailable } from "./panel.ts";
import type { CatchupAnswer, CatchupRequestBody, DiscussCoverage, DiscussFoldState, DiscussScope } from "./panel.ts";
import {
  discussScopeKey,
  isSameDiscussScope,
  linkDecision,
  normalizeDiscussScope,
  viewChangeDecision,
} from "./binding.ts";
import type { NormalizedDiscussScope } from "./binding.ts";

export {
  DISCUSS_BINDING_RULES,
  WIDEN_OR_SIDEWAYS_ACTION,
  discussScopeKey,
  isSameDiscussScope,
  linkDecision,
  normalizeDiscussScope,
  viewChangeDecision,
} from "./binding.ts";
export type { DiscussLinkAction, LinkDecision, NormalizedDiscussScope, ViewChangeDecision } from "./binding.ts";

/**
 * `#1395 discuss-scope-binding`. What a history entry pushed on a binding
 * exit carries: Discuss open, its bound scope, the `#1345` record key
 * (channel + thread, the scope key), and the in-memory answer as the
 * restore fallback until the record lands. Back restores from this with
 * no model call.
 */
export interface DiscussHistoryState {
  discussOpen: true;
  scope: DiscussScope;
  recordKey: string;
  snapshot: { scope: DiscussScope; answer: CatchupAnswer } | null;
}

/** `#1821 vibe-talk-discuss-state-persists`. Where Discuss stands, as kept on the device. */
export interface DiscussPanelState {
  open: boolean;
  channelId: string;
  threadId: string | null;
  view: "flat" | "main";
  // `#1841`: the bound scope's own name (a thread's slug or first line), so a
  // restored thread scope is never titled with the channel's name.
  label?: string;
}

/** `#1841`: a thread's own name for its Discuss title: its filed slug or its
 * first line, truncated; "" when the thread is not named yet — never the
 * channel's name. */
function threadScopeLabel(thread: { title?: unknown; display_name?: unknown } | null | undefined): string {
  const named = thread?.display_name ?? thread?.title;
  const text = typeof named === "string" ? named.trim() : "";
  return text.length > 48 ? `${text.slice(0, 47)}…` : text;
}

/** The selection the panel scopes to, as far as Discuss reads it. */
export interface DiscussSelection {
  view: string;
  threadId: string | null;
  thread?: { title?: unknown; display_name?: unknown } | null;
}

/** What the Discuss view needs from the page. Each is asked at the moment it is needed. */
export interface DiscussViewDependencies {
  byId(id: string): HTMLElement;
  /** The page's API request, used where no `catchupPost` is injected. */
  api?(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  /** POST one catch-up for `channelId`, where the page (or a test) injects its own. */
  catchupPost?(channelId: string, body: CatchupRequestBody): Promise<unknown>;
  /** GET one scope's unread preview, where the page (or a test) injects its own. */
  catchupPreview?(channelId: string, scope: DiscussScope): Promise<unknown>;
  /** GET one scope's saved record, where the page (or a test) injects its own. */
  catchupRecord?(channelId: string, scope: DiscussScope): Promise<unknown>;
  /** POST one follow-up, where the page (or a test) injects its own. */
  catchupFollowup?(channelId: string, scope: DiscussScope, text: string): Promise<unknown>;
  /** POST one incremental Update, where the page (or a test) injects its own (`#1388` landing 2). */
  catchupUpdate?(channelId: string, scope: DiscussScope): Promise<unknown>;
  /** `#1388` landing 1. Whether a message is read now, from the page's read set. */
  isReadNow?(id: string): boolean;
  /** Whether a bare Enter sends, handed to the panel's composer keys. */
  enterSends?(): boolean;
  /** The channel on screen: the parent channel even in a thread. */
  selectedChannelId(): string;
  /** Which list the left column shows, and its thread where it shows one. */
  selection(): DiscussSelection;
  /**
   * Whether this deployment has a direct LLM summarizer Discuss can use, read on
   * every draw: absent means available. The page wires client-config's
   * `discuss_available`, held with the other server facts.
   */
  discussAvailable?(): boolean;
  /**
   * `#1812 vibe-talk-discuss-off-in-documents`. Whether the Documents major
   * view is active: Discuss has no document scope there, so it is disabled
   * (tooltip on the control), an open panel closes, and no entry point opens
   * it until the reader switches back. Absent means never in Documents.
   */
  inDocumentsView?(): boolean;
  /** The chat service `channelId` is read through, for jump refs; "discord" unwired. */
  serviceName?(channelId: string): string;
  /** The reader's name for `channelId`, for the scope line; the id unwired. */
  channelLabel?(channelId: string): string;
  /** Jump to the message a footer link names. */
  onJump(ref: MessageLinkRef): unknown;
  /**
   * `#1395 discuss-scope-binding`. Show the bound global view, then jump to
   * `ref` there (rule `out-of-thread-link-returns-to-bound`); the panel
   * stays open. Absent, a return-to-bound tap jumps as `onJump` does.
   */
  navigateToBound?(scope: DiscussScope, ref: MessageLinkRef): unknown;
  /**
   * `#1395 discuss-scope-binding`. Push the history entry a binding exit
   * (widen/sideways close, fallback jump) leaves for Back to restore from.
   * Absent in hosts without history; Back-restore then keeps the snapshot
   * in memory only.
   */
  pushDiscussState?(state: DiscussHistoryState): unknown;
  /** The Done acts, which own the ≤1000-id batching of dismiss and restore. */
  dismissMessages(body: { messages: readonly string[] }): Promise<unknown>;
  restoreMessages(ids: readonly string[]): Promise<unknown>;
  showError?(text: string): unknown;
  setStatus?(text: string): unknown;
  /** `#1344 discuss-panel-resize`. The panel layout kept on this device, forwarded to the panel. */
  storedWidth?(): number;
  keepWidth?(px: number): void;
  clampWidth?(value: unknown): number;
  defaultWidth?: number;
  storedDockedHeight?(): number;
  keepDockedHeight?(pct: number): void;
  clampDockedHeight?(value: unknown): number;
  defaultDockedHeight?: number;
  storedSheetFull?(): boolean;
  keepSheetFull?(full: boolean): void;
  /** `#1354 discuss-layout-modes`. The panel's layout mode, forwarded to the panel. */
  storedMode?(): "overlay" | "docked" | "maximized";
  keepMode?(mode: "overlay" | "docked" | "maximized"): void;
  /** `#1497 discuss-fold-state-persist`. The kept Discuss folds, forwarded to the panel. */
  storedFolds?(): DiscussFoldState | null;
  keepFolds?(state: DiscussFoldState): void;
  /** `#1842`. The per-scope covered ranges kept on this device, forwarded to the panel. */
  storedCoverage?(key: string): DiscussCoverage | null;
  keepCoverage?(key: string, coverage: DiscussCoverage): void;
  /** `#1821 vibe-talk-discuss-state-persists`. Where Discuss stands, kept on this device. */
  storedPanelState?(): DiscussPanelState | null;
  keepPanelState?(state: DiscussPanelState): void;
  /** `#1524 discuss-bound-hours-control`. Now, forwarded to the panel; injected in tests. */
  now?(): number;
  /** `#1501 discuss-read-aloud-parity`. The shared read-aloud owner, forwarded to the panel. */
  readDocument?(doc: {
    key: string;
    title?: string;
    texts: readonly string[];
    kinds?: readonly string[];
    onReadingBlock?: (block: number | null, element?: number | null) => unknown;
  }, from?: { part: number; block: number | null }): unknown;
  /** Stop the reading of summary `key`, where that summary's is the one playing; no other. */
  stopSpeaking?(key: string): unknown;
}

/** The Discuss view, as the page calls it. */
export interface DiscussView {
  /** Wire the bar button and the panel's controls; the boot tracks the answer. */
  mountControls(): { unmount(): void };
  /**
   * Apply the binding to the selection now (`#1395`): same scope keeps
   * the panel open under its bound label; entering a thread re-scopes
   * the open panel to that thread (`#1811`); widening, sideways, and a
   * channel change close it behind a history entry.
   */
  sync(): void;
  /** Open the panel for the selection now and ask at once. */
  open(): void;
  /** `#1395`. Open the panel bound to `scope` (record restore, tests). */
  openWith(scope: DiscussScope): void;
  close(): void;
  isOpen(): boolean;
  /** `#1395`. The scope Discuss is bound to (kept while closed), or null. */
  boundScope(): NormalizedDiscussScope | null;
  /** `#1395`. Whether the selection now is the bound scope (All = Main). */
  isSameScopeAsCurrent(): boolean;
  /**
   * `#1395`. Restore Discuss from a history entry pushed on a binding
   * exit: the same summary, no model call. True when it restored.
   */
  restoreFromHistory(state: unknown): boolean;
  /** `#1386 discuss-mod8-toggle`. Open when closed, close when open, as the bar button does. */
  toggle(): void;
  /** `#1344`. The overlay width in force, in CSS pixels. */
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
  mode(): "overlay" | "docked" | "maximized";
  /** `#1354`. Stand the panel in `mode` and keep the choice on the device. */
  setMode(mode: "overlay" | "docked" | "maximized"): void;
  /** `#1586`. Leave full screen so a linked message can land in the list. */
  dropToPartialForLink(): void;
  /** `#1388` landing 1. Restate the summary's strike from the read set now. */
  refreshStrikes(): void;
  /** `#1531 discuss-top-bar-slim-fixed`. Read the summary on screen aloud whole; false with none. */
  readSummary(): boolean;
}

/** Make the Discuss view from the page's thunks. Nothing is looked up yet. */
export function createDiscussView(deps: DiscussViewDependencies): DiscussView {
  const listening = createListening();
  let mounted = false;
  let panelTeardown: { unmount(): void } | null = null;
  // The server refused for want of a summarizer after the boot said otherwise:
  // the button is drawn unavailable from then on, as if the boot had said so.
  let refused = false;

  /** `#1812`. Whether the Documents major view is active, asked at the moment it is needed. */
  function documentsActive(): boolean {
    try {
      return deps.inDocumentsView?.() === true;
    } catch {
      return false;
    }
  }

  /** Whether Discuss can work here now: the boot's word, no refusal since, and not in Documents. */
  function available(): boolean {
    return !refused && !documentsActive() && (deps.discussAvailable?.() ?? true);
  }

  /**
   * Draw the button's availability. `aria-disabled`, not the `disabled`
   * attribute, for the bar's standing reason (18-control-bar.css): the button
   * stays a hover and focus target, so its tooltip can say why; the refusal
   * itself is enforced in the click and in `open`, never by appearance alone.
   */
  function drawAvailability(): void {
    // Busy draws like unavailable (`discuss-summarize-button`): while one request is
    // in flight no entry point starts another, and the reason shows on hover.
    const off = !available() || busy;
    const why = documentsActive()
      ? DISCUSS_DOCUMENTS_HINT
      : busy && available() ? "Summarizing…" : DISCUSS_UNAVAILABLE_HINT;
    const button = deps.byId("discuss-open");
    if (!off) {
      button.removeAttribute("aria-disabled");
      button.setAttribute("title", "Catch up on this channel or thread");
      button.removeAttribute("aria-label");
    } else {
      button.setAttribute("aria-disabled", "true");
      button.setAttribute("title", why);
      if (why === DISCUSS_UNAVAILABLE_HINT || why === DISCUSS_DOCUMENTS_HINT) button.setAttribute("aria-label", why);
      button.setAttribute("aria-expanded", "false");
    }
    // The menu's Discuss (the phone's way in) says the same, in its own dress.
    try {
      const item = deps.byId("discuss-menu-open");
      if (!off) {
        item.removeAttribute("aria-disabled");
        item.setAttribute("title", "Catch up on this channel or thread");
        item.removeAttribute("aria-label");
      } else {
        item.setAttribute("aria-disabled", "true");
        item.setAttribute("title", why);
        if (why === DISCUSS_UNAVAILABLE_HINT || why === DISCUSS_DOCUMENTS_HINT) item.setAttribute("aria-label", why);
      }
    } catch {
      // A host without the menu item: the bar button already says it.
    }
  }

  function catchupPost(channelId: string, body: CatchupRequestBody): Promise<unknown> {
    const asked = deps.catchupPost
      ? deps.catchupPost(channelId, body)
      : deps.api
        ? deps.api(`/api/v1/channels/${encodeURIComponent(channelId)}/catchup`, { method: "POST", body })
        : Promise.reject(new Error("Discuss has no way to reach the server."));
    return asked.catch((error: unknown) => {
      if (isDiscussUnavailable(error)) {
        refused = true;
        try {
          drawAvailability();
        } catch {
          // No button in this host; the panel still says the hint.
        }
      }
      throw error;
    });
  }

  let busy = false;

  function catchupPreview(channelId: string, scope: DiscussScope): Promise<unknown> {
    if (deps.catchupPreview) return deps.catchupPreview(channelId, scope);
    if (!deps.api) return Promise.reject(new Error("Discuss has no way to reach the server."));
    const query = new URLSearchParams();
    if (scope.threadId !== null) query.set("thread_id", scope.threadId);
    query.set("view", scope.view);
    return deps.api(`/api/v1/channels/${encodeURIComponent(channelId)}/catchup/preview?${query.toString()}`, { method: "GET" });
  }

  function catchupRecord(channelId: string, scope: DiscussScope): Promise<unknown> {
    if (deps.catchupRecord) return deps.catchupRecord(channelId, scope);
    if (!deps.api) return Promise.reject(new Error("Discuss has no way to reach the server."));
    const query = new URLSearchParams();
    if (scope.threadId !== null) query.set("thread_id", scope.threadId);
    query.set("view", scope.view);
    return deps.api(`/api/v1/channels/${encodeURIComponent(channelId)}/catchup/record?${query.toString()}`, { method: "GET" });
  }

  // `#1395 discuss-scope-binding`. The scope Discuss is bound to, set on
  // open and kept while closed (a same-scope reopen restores, Back reads
  // it); `boundRaw` is the same scope as opened, for its label and view.
  let boundRaw: DiscussScope | null = null;
  let boundScope: NormalizedDiscussScope | null = null;

  /** Leave the history entry Back restores this binding from, if wired. */
  function pushRestoreState(): void {
    if (boundRaw === null || boundScope === null) return;
    try {
      deps.pushDiscussState?.({
        discussOpen: true,
        scope: boundRaw,
        recordKey: discussScopeKey(boundScope),
        snapshot: panel.snapshot(),
      });
    } catch {
      // History refused: the in-memory snapshot still restores this session.
    }
  }

  // `#1821 vibe-talk-discuss-state-persists`. Keep openness with its bound scope
  // on the device, from the panel's one openness change; restore it on load.
  let restoreDone = false;
  function persistPanelState(open: boolean): void {
    if (boundRaw === null) return;
    try {
      deps.keepPanelState?.({ open, channelId: boundRaw.channelId, threadId: boundRaw.threadId, view: boundRaw.view, label: boundRaw.label });
    } catch {
      // Storage refused: openness holds until a reload, as the layout does.
    }
  }

  /** Reopen for the kept scope once its channel is on screen; true when it just did. */
  function tryRestorePanel(): boolean {
    if (restoreDone) return false;
    // A shell stood open early by the page (`#1821`) has no bound scope yet:
    // it still needs its kept scope filled, so only a bound panel is done.
    if (panel.isOpen() && boundScope !== null) {
      restoreDone = true;
      return false;
    }
    let stored: DiscussPanelState | null = null;
    try {
      stored = deps.storedPanelState?.() ?? null;
    } catch {
      stored = null;
    }
    if (stored === null || !stored.open) {
      restoreDone = true;
      return false;
    }
    const selected = String(deps.selectedChannelId() ?? "");
    if (selected === "") return false;
    if (stored.channelId !== selected) {
      // Not this channel's panel: an early-opened shell comes down, and a
      // later sync on the kept channel may still restore it.
      if (panel.isOpen() && boundScope === null) panel.close();
      return false;
    }
    // Busy, and a summarizer not yet known at boot, both defer: a later
    // sync (client-config's, a view change's) retries the restore.
    if (busy || !available()) return false;
    restoreDone = true;
    // `#1841`: a thread scope restores under the thread's own name — the kept
    // label, or the open thread's where it is this thread — never the channel's.
    let label = stored.threadId === null
      ? (deps.channelLabel?.(stored.channelId) ?? stored.channelId)
      : (typeof stored.label === "string" ? stored.label : "");
    try {
      const selection = deps.selection();
      if (stored.threadId !== null && selection.threadId === stored.threadId) {
        const named = threadScopeLabel(selection.thread);
        if (named !== "") label = named;
      }
    } catch {
      // The kept label stands where the selection is not ready.
    }
    openWith({ channelId: stored.channelId, threadId: stored.threadId, view: stored.view, label });
    return true;
  }

  /** Route one citation tap by the binding; the panel stays unless it closes. */
  function routeLink(ref: MessageLinkRef, outOfScope: boolean): void {
    let current: NormalizedDiscussScope;
    try {
      current = normalizeDiscussScope(scopeNow());
    } catch {
      panel.dropToPartialForLink();
      deps.onJump(ref);
      return;
    }
    const decision = linkDecision({
      bound: boundScope,
      current,
      linkChannel: ref.channel,
      linkThread: ref.thread,
      outOfScope,
    });
    if (decision.action === "jump-in-place") {
      // `#1586`: out of full screen first, so the jump lands visibly.
      panel.dropToPartialForLink();
      deps.onJump(ref);
    } else if (decision.action === "return-to-bound" && boundRaw !== null) {
      panel.dropToPartialForLink();
      if (deps.navigateToBound) deps.navigateToBound(boundRaw, ref);
      else deps.onJump(ref);
    } else {
      pushRestoreState();
      panel.close();
      deps.onJump(ref);
    }
  }

  const panel = createDiscussPanel({
    byId: deps.byId,
    catchupPost,
    catchupPreview,
    catchupRecord,
    catchupFollowup(channelId: string, scope: DiscussScope, text: string): Promise<unknown> {
      if (deps.catchupFollowup) return deps.catchupFollowup(channelId, scope, text);
      if (!deps.api) return Promise.reject(new Error("Discuss has no way to reach the server."));
      return deps.api(`/api/v1/channels/${encodeURIComponent(channelId)}/catchup/messages`, {
        method: "POST",
        body: JSON.stringify({ thread_id: scope.threadId, text }),
      });
    },
    catchupUpdate(channelId: string, scope: DiscussScope): Promise<unknown> {
      if (deps.catchupUpdate) return deps.catchupUpdate(channelId, scope);
      if (!deps.api) return Promise.reject(new Error("Discuss has no way to reach the server."));
      return deps.api(`/api/v1/channels/${encodeURIComponent(channelId)}/catchup/update`, {
        method: "POST",
        body: JSON.stringify({ thread_id: scope.threadId, view: scope.view }),
      });
    },
    isReadNow: (id: string) => deps.isReadNow?.(id) ?? false,
    enterSends: () => deps.enterSends?.() ?? true,
    onBusy(on) {
      busy = on;
      drawAvailability();
    },
    serviceName: (channelId) => deps.serviceName?.(channelId) ?? "discord",
    onJump: deps.onJump,
    onLinkTap: (ref, outOfScope) => routeLink(ref, outOfScope),
    dismissMessages: deps.dismissMessages,
    restoreMessages: deps.restoreMessages,
    showError: deps.showError,
    setStatus: deps.setStatus,
    storedWidth: deps.storedWidth,
    keepWidth: deps.keepWidth,
    clampWidth: deps.clampWidth,
    defaultWidth: deps.defaultWidth,
    storedDockedHeight: deps.storedDockedHeight,
    keepDockedHeight: deps.keepDockedHeight,
    clampDockedHeight: deps.clampDockedHeight,
    defaultDockedHeight: deps.defaultDockedHeight,
    storedSheetFull: deps.storedSheetFull,
    keepSheetFull: deps.keepSheetFull,
    storedMode: deps.storedMode,
    keepMode: deps.keepMode,
    storedFolds: deps.storedFolds,
    keepFolds: deps.keepFolds,
    storedCoverage: deps.storedCoverage,
    keepCoverage: deps.keepCoverage,
    onOpenChange: (open: boolean) => persistPanelState(open),
    readDocument: deps.readDocument,
    stopSpeaking: deps.stopSpeaking,
    now: deps.now,
  });

  /** Where Discuss is aimed now: the open thread, else the channel in its current view. */
  function scopeNow(): DiscussScope {
    const channelId = String(deps.selectedChannelId() ?? "");
    const selection = deps.selection();
    const threadId = selection.view === "thread" && selection.threadId ? String(selection.threadId) : null;
    let label = deps.channelLabel?.(channelId) ?? channelId;
    if (threadId !== null) {
      // `#1841`: the thread's own name, or no name yet — a thread scope must
      // never borrow the channel's label for its title.
      label = threadScopeLabel(selection.thread);
    }
    return {
      channelId,
      threadId,
      view: threadId === null && selection.view === "main" ? "main" : "flat",
      label,
    };
  }

  function open(): void {
    if (!available() || busy) return;
    const scope = scopeNow();
    // A same-scope reopen restores the answer on screen (Mod+8, Back):
    // no preview read, no model call. Any other scope starts fresh.
    const held = panel.snapshot();
    if (boundScope !== null && held !== null && isSameDiscussScope(boundScope, scope)) {
      panel.restoreAnswer(held.scope, held.answer);
      return;
    }
    boundRaw = scope;
    boundScope = normalizeDiscussScope(scope);
    panel.openWith(scope);
  }

  function openWith(scope: DiscussScope): void {
    if (!available() || busy) return;
    boundRaw = scope;
    boundScope = normalizeDiscussScope(scope);
    panel.openWith(scope);
  }

  return {
    mountControls() {
      if (!mounted) {
        mounted = true;
        panelTeardown = panel.mount();
        listening.listen(deps.byId("discuss-open"), "click", () => {
          if (busy) return;
          if (panel.isOpen()) panel.close();
          else if (available()) open();
          // Unavailable: the click does nothing — no panel, no POST — and the
          // button's title and aria label already say why.
        });
        // The action menu's Discuss: choosing it closes the menu (the list
        // controls' standing act for its items) and opens the same panel.
        try {
          listening.listen(deps.byId("discuss-menu-open"), "click", () => {
            // `#1812`: in Documents the item stands disabled; a tap says why
            // in place, as its tooltip does on hover.
            if (documentsActive()) deps.setStatus?.(DISCUSS_DOCUMENTS_HINT);
            else if (!busy && available()) open();
          });
        } catch {
          // A host without the menu item keeps the bar button's way in.
        }
        drawAvailability();
        try {
          panel.setScope(scopeNow());
        } catch {
          // A host whose selection is not ready yet labels it at the first sync.
        }
        // `#1821`: restore at once where the channel already stands; else sync retries.
        tryRestorePanel();
      }
      return {
        unmount() {
          mounted = false;
          listening.unmount();
          panelTeardown?.unmount();
          panelTeardown = null;
        },
      };
    },
    sync() {
      // `#1821`: a just-restored panel is not judged by the sync that restored it.
      if (tryRestorePanel()) return;
      try {
        // `#1812`: Documents disables Discuss. Close an open panel (its
        // snapshot is kept, so switching back and reopening restores it with
        // no model call) and draw the control disabled with its reason.
        if (documentsActive()) {
          if (panel.isOpen()) panel.close();
          drawAvailability();
          return;
        }
        const currentRaw = scopeNow();
        if (panel.isOpen() && boundScope !== null && boundRaw !== null) {
          const decision = viewChangeDecision(boundScope, normalizeDiscussScope(currentRaw));
          if (decision.action === "stay") {
            // Same scope: the answer stands. The label may name itself only
            // after entry (the thread's summary landing), so a name the
            // current scope now carries replaces a nameless bound label
            // (#1841) — never the other way round.
            if (currentRaw.label !== "" && currentRaw.label !== boundRaw.label) {
              boundRaw = currentRaw;
            }
            panel.setScope(boundRaw);
          } else if (decision.action === "rescope") {
            // `#1811`: entering a thread re-aims Discuss at that thread —
            // its unread count, summarize bound and context become the
            // thread's alone; the panel stays open, no model call yet.
            boundRaw = currentRaw;
            boundScope = normalizeDiscussScope(currentRaw);
            panel.rescope(currentRaw);
          } else {
            pushRestoreState();
            panel.close();
          }
        } else {
          panel.setScope(currentRaw);
        }
        drawAvailability();
        // `#1388` landing 1: a view change may carry new read state with it.
        panel.refreshStrikes();
      } catch {
        // No panel in this host yet: sync is a label only, and says nothing.
      }
    },
    open,
    openWith,
    close: () => panel.close(),
    isOpen: () => panel.isOpen(),
    boundScope: () => boundScope,
    isSameScopeAsCurrent() {
      if (boundScope === null) return false;
      try {
        return isSameDiscussScope(boundScope, scopeNow());
      } catch {
        return false;
      }
    },
    restoreFromHistory(state: unknown) {
      if (typeof state !== "object" || state === null) return false;
      const record = state as Partial<DiscussHistoryState>;
      if (record.discussOpen !== true || typeof record.scope !== "object" || record.scope === null) return false;
      // `#1812`: no entry point opens Discuss in Documents, Back included.
      if (!available() || busy) return false;
      const scope = record.scope as DiscussScope;
      boundRaw = scope;
      boundScope = normalizeDiscussScope(scope);
        // The in-memory answer first (no fetch at all); without it, the
      // scope reopen reads the `#1345` record, still with no model call.
      const snapshot = record.snapshot;
      if (snapshot && typeof snapshot === "object" && snapshot.answer) {
        panel.restoreAnswer(snapshot.scope ?? scope, snapshot.answer);
      } else {
        panel.openWith(scope);
      }
      return true;
    },
    toggle() {
      if (busy) return;
      if (panel.isOpen()) panel.close();
      else if (available()) open();
      // `#1812`: Mod+8 in Documents does not open it, and says why in place.
      else if (documentsActive()) deps.setStatus?.(DISCUSS_DOCUMENTS_HINT);
    },
    width: () => panel.width(),
    setWidth: (px: unknown) => panel.setWidth(px),
    dockedHeight: () => panel.dockedHeight(),
    setDockedHeight: (pct: unknown) => panel.setDockedHeight(pct),
    sheetFull: () => panel.sheetFull(),
    setSheetFull: (full: boolean) => panel.setSheetFull(full),
    mode: () => panel.mode(),
    setMode: (mode: "overlay" | "docked" | "maximized") => panel.setMode(mode),
    dropToPartialForLink: () => panel.dropToPartialForLink(),
    refreshStrikes: () => panel.refreshStrikes(),
    readSummary: () => panel.readSummary(),
  };
}

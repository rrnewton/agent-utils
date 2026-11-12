// view/catchup (`catchup-side-agent`, slice 1): Discuss — a bar button and an overlay
// panel that ask the server for a one-shot catch-up over the channel or thread on
// screen. Its public interface; the page constructs it beside the Documents view,
// tracks `mountControls()` with the boot, and calls `sync()` from `showView`.
// Construction is inert: no element is looked up, no listener is added and nothing is
// fetched until the controls are mounted and the panel opened.

export { DISCUSS_UNAVAILABLE_HINT, asCatchupAnswer, createDiscussPanel, formatStampLine, isDiscussUnavailable } from "./panel.ts";
export type {
  CatchupAnswer,
  CatchupFooterLink,
  CatchupRequestBody,
  CatchupStamp,
  CatchupWindow,
  DiscussPanel,
  DiscussPanelDependencies,
  DiscussScope,
} from "./panel.ts";

import { createListening } from "../dom/index.ts";
import type { MessageLinkRef } from "../../foundation/index.ts";
import { DISCUSS_UNAVAILABLE_HINT, createDiscussPanel, isDiscussUnavailable } from "./panel.ts";
import type { CatchupRequestBody, DiscussScope } from "./panel.ts";

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
  /** The chat service `channelId` is read through, for jump refs; "discord" unwired. */
  serviceName?(channelId: string): string;
  /** The reader's name for `channelId`, for the scope line; the id unwired. */
  channelLabel?(channelId: string): string;
  /** Jump to the message a footer link names. */
  onJump(ref: MessageLinkRef): unknown;
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
  storedSheetFull?(): boolean;
  keepSheetFull?(full: boolean): void;
}

/** The Discuss view, as the page calls it. */
export interface DiscussView {
  /** Wire the bar button and the panel's controls; the boot tracks the answer. */
  mountControls(): { unmount(): void };
  /** Restate the scope line for the selection now, forgetting an answer aimed elsewhere. */
  sync(): void;
  /** Open the panel for the selection now and ask at once. */
  open(): void;
  close(): void;
  isOpen(): boolean;
  /** `#1344`. The overlay width in force, in CSS pixels. */
  width(): number;
  /** `#1344`. Put a width on the panel and keep it on the device. */
  setWidth(px: unknown): void;
  /** `#1344`. Whether the phone sheet stands at full height. */
  sheetFull(): boolean;
  /** `#1344`. Stand the phone sheet at full or half height and keep the choice. */
  setSheetFull(full: boolean): void;
}

/** Make the Discuss view from the page's thunks. Nothing is looked up yet. */
export function createDiscussView(deps: DiscussViewDependencies): DiscussView {
  const listening = createListening();
  let mounted = false;
  let panelTeardown: { unmount(): void } | null = null;
  let syncedScopeKey: string | null = null;
  // The server refused for want of a summarizer after the boot said otherwise:
  // the button is drawn unavailable from then on, as if the boot had said so.
  let refused = false;

  /** Whether Discuss can work here now: the boot's word, and no refusal since. */
  function available(): boolean {
    return !refused && (deps.discussAvailable?.() ?? true);
  }

  /**
   * Draw the button's availability. `aria-disabled`, not the `disabled`
   * attribute, for the bar's standing reason (18-control-bar.css): the button
   * stays a hover and focus target, so its tooltip can say why; the refusal
   * itself is enforced in the click and in `open`, never by appearance alone.
   */
  function drawAvailability(): void {
    const button = deps.byId("discuss-open");
    if (available()) {
      button.removeAttribute("aria-disabled");
      button.setAttribute("title", "Catch up on this channel or thread");
      button.removeAttribute("aria-label");
    } else {
      button.setAttribute("aria-disabled", "true");
      button.setAttribute("title", DISCUSS_UNAVAILABLE_HINT);
      button.setAttribute("aria-label", DISCUSS_UNAVAILABLE_HINT);
      button.setAttribute("aria-expanded", "false");
    }
    // The menu's Discuss (the phone's way in) says the same, in its own dress.
    try {
      const item = deps.byId("discuss-menu-open");
      if (available()) {
        item.removeAttribute("aria-disabled");
        item.setAttribute("title", "Catch up on this channel or thread");
        item.removeAttribute("aria-label");
      } else {
        item.setAttribute("aria-disabled", "true");
        item.setAttribute("title", DISCUSS_UNAVAILABLE_HINT);
        item.setAttribute("aria-label", DISCUSS_UNAVAILABLE_HINT);
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

  const panel = createDiscussPanel({
    byId: deps.byId,
    catchupPost,
    serviceName: (channelId) => deps.serviceName?.(channelId) ?? "discord",
    onJump: deps.onJump,
    dismissMessages: deps.dismissMessages,
    restoreMessages: deps.restoreMessages,
    showError: deps.showError,
    setStatus: deps.setStatus,
    storedWidth: deps.storedWidth,
    keepWidth: deps.keepWidth,
    clampWidth: deps.clampWidth,
    defaultWidth: deps.defaultWidth,
    storedSheetFull: deps.storedSheetFull,
    keepSheetFull: deps.keepSheetFull,
  });

  /** Where Discuss is aimed now: the open thread, else the channel in its current view. */
  function scopeNow(): DiscussScope {
    const channelId = String(deps.selectedChannelId() ?? "");
    const selection = deps.selection();
    const threadId = selection.view === "thread" && selection.threadId ? String(selection.threadId) : null;
    let label = deps.channelLabel?.(channelId) ?? channelId;
    if (threadId !== null) {
      const named = selection.thread?.display_name ?? selection.thread?.title;
      label = typeof named === "string" && named !== "" ? named : label;
    }
    return {
      channelId,
      threadId,
      view: threadId === null && selection.view === "main" ? "main" : "flat",
      label,
    };
  }

  function scopeKey(scope: DiscussScope): string {
    return `${scope.channelId}\n${scope.threadId ?? ""}\n${scope.view}`;
  }

  function open(): void {
    if (!available()) return;
    const scope = scopeNow();
    syncedScopeKey = scopeKey(scope);
    panel.openWith(scope);
  }

  return {
    mountControls() {
      if (!mounted) {
        mounted = true;
        panelTeardown = panel.mount();
        listening.listen(deps.byId("discuss-open"), "click", () => {
          if (panel.isOpen()) panel.close();
          else if (available()) open();
          // Unavailable: the click does nothing — no panel, no POST — and the
          // button's title and aria label already say why.
        });
        // The action menu's Discuss: choosing it closes the menu (the list
        // controls' standing act for its items) and opens the same panel.
        try {
          listening.listen(deps.byId("discuss-menu-open"), "click", () => {
            if (available()) open();
          });
        } catch {
          // A host without the menu item keeps the bar button's way in.
        }
        drawAvailability();
        try {
          panel.setScopeLabel(scopeNow().label);
        } catch {
          // A host whose selection is not ready yet labels it at the first sync.
        }
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
      try {
        const scope = scopeNow();
        const key = scopeKey(scope);
        if (syncedScopeKey !== null && key !== syncedScopeKey) panel.invalidate();
        syncedScopeKey = key;
        panel.setScopeLabel(scope.label);
        drawAvailability();
      } catch {
        // No panel in this host yet: sync is a label only, and says nothing.
      }
    },
    open,
    close: () => panel.close(),
    isOpen: () => panel.isOpen(),
    width: () => panel.width(),
    setWidth: (px: unknown) => panel.setWidth(px),
    sheetFull: () => panel.sheetFull(),
    setSheetFull: (full: boolean) => panel.setSheetFull(full),
  };
}

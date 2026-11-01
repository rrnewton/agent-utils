// How much of the channel's history the server keeps (`#602 history-settings-and-limits`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 16, view/settings).
//
// The owner, 2026-10-27: "I assume it is a per channel setting? I think we should expose it under the
// channel pane in settings ... It would be nice to show progress and ETA in that same channel settings
// pane." So the channel box says how much of the channel the server's message cache holds, whether its
// walk into older messages is under way and how fast, when it should reach the beginning, and why it is
// paused when it is; and it offers the channel's own mode (the server's default, recent days only, or
// the whole history) and "Read older messages next", which puts the channel at the front of the
// server's queue. Like Clear unread, the choice is the server's: read and written through /history,
// nothing kept on this device. The box reads it again every CHANNEL_HISTORY_RECHECK_MS while Settings
// shows it, more often while the channel is queued (web/src/present/settings).

import type { ChannelHistoryState, Contract, HistoryModeChoice } from "../../contract/index.ts";
import { channelHistorySentence } from "../../present/notices/index.ts";
import {
  historyChoiceOf,
  historyDefaultWords,
  historyPrioritizeOffered,
  historyPrioritizedWords,
  historyReadFailure,
  historyRecheckWait,
  historyWaitingWords,
} from "../../present/settings/index.ts";

/** A request's failure as the page's server reports one: the server's own words, or the error's. */
type RequestFailure = { readonly detail?: string; readonly message?: string };

/** What the box asks of the page. */
export interface ChannelHistoryDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** A request to the page's server, its answer checked against the contract (`apiDecoded`). */
  decoded<K extends keyof Contract>(type: K, path: string, options?: { method: string; body?: unknown }): Promise<Contract[K]>;
  /** Whether the picker is on All channels (`allChannelsChosen`). */
  allChannelsChosen(): boolean;
  /** The configured channel with this id, or nothing (`knownChannel`). */
  knownChannel(id: string): unknown;
  /** Whether the server's message cache is open (`serverCache.available`). */
  cacheOpen(): boolean;
  /** Whether this token may change the server's settings (`tokenScope === "write"`). */
  writable(): boolean;
  /** The page's timers. */
  setTimeout(run: () => void, ms: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
}

/** The box's named commands. */
export interface ChannelHistoryBox {
  /** Redraw the history part of the box for the channel its picker is on. */
  render(): void;
  /** Read the box's channel again, as when Settings opens. */
  refresh(): void;
  /** Enable Save, and offer "Read older messages next", to match what is chosen and saved. */
  renderChoice(): void;
  /** Save the chosen mode for the box's channel. */
  save(): Promise<void>;
  /** "Read older messages next": the server reads this channel's older history before any other's. */
  prioritize(): Promise<void>;
}

/** Nothing read, no read waiting. Made once, where the page wires its Settings. */
export function mountChannelHistory(deps: ChannelHistoryDependencies): ChannelHistoryBox {
  /**
   * What the server last said about the box's channel: its id, its answer or null, and the failure to
   * show instead when there is no answer.
   */
  let channelHistory: { channel: string; view: ChannelHistoryState | null; failure: string } | null = null;
  /** Counts reads, so an answer for a channel the box has since left is dropped rather than drawn. */
  let channelHistoryReads = 0;
  /** The next read of the box's channel, while Settings shows it. */
  let channelHistoryTimer: ReturnType<typeof setTimeout> | null = null;

  const pickedChannel = (): string => String((deps.lookup("settings-channel") as HTMLSelectElement).value);
  const modeSelect = (): HTMLSelectElement => deps.lookup("channel-history-mode") as HTMLSelectElement;
  const failureOf = (error: unknown): string | undefined => (error as RequestFailure).detail || (error as RequestFailure).message;
  const historyPath = (channel: string, tail = ""): string => `/api/v1/channels/${encodeURIComponent(channel)}/history${tail}`;

  /** Whether the box shows the channel's history: one channel chosen, on a server whose cache is open. */
  const offered = (id: string): boolean => !deps.allChannelsChosen() && Boolean(deps.knownChannel(id)) && deps.cacheOpen();

  /** Whether Save would change anything: a choice other than the server's. */
  function changed(): boolean {
    const view = channelHistory ? channelHistory.view : null;
    return Boolean(view) && modeSelect().value !== historyChoiceOf(view as ChannelHistoryState);
  }

  function renderChoice(): void {
    const view = channelHistory ? channelHistory.view : null;
    const writable = deps.writable() && Boolean(view && view.settable);
    modeSelect().disabled = !writable;
    deps.lookup("channel-history-save").hidden = !writable;
    (deps.lookup("channel-history-save") as HTMLButtonElement).disabled = !writable || !changed();
    const next = deps.lookup("channel-history-prioritize");
    next.hidden = !historyPrioritizeOffered(view, deps.writable());
  }

  /** Stop reading the box's channel again. */
  function stopRecheck(): void {
    if (channelHistoryTimer !== null) deps.clearTimeout(channelHistoryTimer);
    channelHistoryTimer = null;
  }

  /** Read the box's channel again in a while, while Settings shows it. */
  function scheduleRecheck(): void {
    stopRecheck();
    const view = channelHistory ? channelHistory.view : null;
    if (!channelHistory || deps.lookup("screen-settings").hidden) return;
    const wait = historyRecheckWait(view);
    const channel = channelHistory.channel;
    channelHistoryTimer = deps.setTimeout(() => {
      channelHistoryTimer = null;
      if (deps.lookup("screen-settings").hidden || !channelHistory || channelHistory.channel !== channel) return;
      // What a Save or a press said is about then; the progress line says what holds now.
      deps.lookup("channel-history-state").textContent = "";
      load(channel);
    }, wait);
  }

  /** Draw what the server last said, or why there is nothing to draw. */
  function draw(): void {
    if (!channelHistory) return;
    const view = channelHistory.view;
    if (!view) {
      deps.lookup("channel-history-progress").textContent = historyWaitingWords(channelHistory.failure);
      renderChoice();
      return;
    }
    deps.lookup("channel-history-default").textContent = historyDefaultWords(view);
    deps.lookup("channel-history-progress").textContent = channelHistorySentence(view);
    renderChoice();
  }

  /**
   * Take the server's answer for `channel` as the truth, unless the box has moved on. A choice the
   * owner has made and not saved stays under his hand, as in Clear unread: the select follows the
   * server only while it still shows what was saved before.
   */
  function adopt(channel: string, view: ChannelHistoryState): void {
    if (!channelHistory || channelHistory.channel !== channel) return;
    const before = channelHistory.view ? historyChoiceOf(channelHistory.view) : null;
    const select = modeSelect();
    if (before === null || select.value === before) select.value = historyChoiceOf(view);
    channelHistory = { channel, view, failure: "" };
    draw();
    scheduleRecheck();
  }

  /** Ask the server how `channel`'s history stands. */
  async function load(channel: string): Promise<void> {
    const read = ++channelHistoryReads;
    try {
      const view = await deps.decoded("ChannelHistoryState", historyPath(channel));
      if (read === channelHistoryReads) adopt(channel, view);
    } catch (error) {
      if (read !== channelHistoryReads || !channelHistory || channelHistory.channel !== channel) return;
      channelHistory = { channel, view: null, failure: historyReadFailure(String(failureOf(error))) };
      draw();
    }
  }

  function render(): void {
    const id = pickedChannel();
    const shown = offered(id);
    deps.lookup("channel-history").hidden = !shown;
    if (!shown) {
      channelHistory = null;
      stopRecheck();
      return;
    }
    if (!channelHistory || channelHistory.channel !== id) {
      channelHistory = { channel: id, view: null, failure: "" };
      stopRecheck();
      modeSelect().value = "default";
      deps.lookup("channel-history-state").textContent = "";
      draw();
      // Asked only while Settings is showing, as Clear unread is.
      if (!deps.lookup("screen-settings").hidden) load(id);
      return;
    }
    draw();
  }

  return {
    render,
    refresh() {
      const id = pickedChannel();
      if (!offered(id)) return;
      if (channelHistory && channelHistory.channel === id) load(id);
      else render();
    },
    renderChoice,
    async save() {
      const channel = pickedChannel();
      if (!offered(channel) || !changed()) return;
      const mode = modeSelect().value as HistoryModeChoice;
      (deps.lookup("channel-history-save") as HTMLButtonElement).disabled = true;
      deps.lookup("channel-history-state").textContent = "Saving…";
      let view: ChannelHistoryState;
      try {
        view = await deps.decoded("ChannelHistoryState", historyPath(channel), {
          method: "PUT",
          body: { mode },
        });
      } catch (error) {
        deps.lookup("channel-history-state").textContent = `Not saved: ${failureOf(error)}`;
        renderChoice();
        return;
      }
      channelHistoryReads += 1;
      if (channelHistory && channelHistory.channel === channel) {
        // What was saved is now what the select shows: adopt the answer as the saved choice.
        channelHistory = { channel, view: null, failure: "" };
        adopt(channel, view);
        deps.lookup("channel-history-state").textContent = "Saved.";
      }
    },
    async prioritize() {
      const channel = pickedChannel();
      if (!offered(channel)) return;
      (deps.lookup("channel-history-prioritize") as HTMLButtonElement).disabled = true;
      deps.lookup("channel-history-state").textContent = "Asking the server…";
      let view: ChannelHistoryState | null = null;
      try {
        view = await deps.decoded(
          "ChannelHistoryState",
          historyPath(channel, "/prioritize"),
          { method: "POST" }
        );
      } catch (error) {
        deps.lookup("channel-history-state").textContent = `Not asked: ${failureOf(error)}`;
      }
      (deps.lookup("channel-history-prioritize") as HTMLButtonElement).disabled = false;
      if (!view) {
        renderChoice();
        return;
      }
      channelHistoryReads += 1;
      adopt(channel, view);
      deps.lookup("channel-history-state").textContent = historyPrioritizedWords(view);
    },
  };
}

// What one frame of the stream of every channel means, and following it for a call
// (`#624 voice-page-modularity`, moved from the legacy script). Words and order are as
// they were in the script. The stream itself is core/sync's (`createWatchAll`), made in
// the script and handed in; it takes each frame's cursor down before handing it here.

import { is } from "../../contract/index.ts";
import type { LiveMessageEvent } from "../../contract/index.ts";

/** One frame of the stream, as this owner reads it (core/sync `EventFrame`). */
export interface WatchFrame {
  event: string;
  data: string;
}

/** The stream of every channel, as this owner uses it (core/sync `WatchAll`). */
export interface WatchAllLike {
  start(): void;
  stop(): void;
  resumed(): boolean;
}

/** The call a frame may be relayed to, as this owner uses it. */
export interface WatchCall {
  canSendText(): boolean;
  reportUnrelayed(ids: unknown[], why: string): void;
  relayToAgent(message: unknown, selfPosted: unknown, replayed: unknown, options: { fromTail: boolean; resumed: boolean }): void;
}

/** What reading the stream of every channel asks of the page. */
export interface WatchFrameDependencies {
  watchAll(): WatchAllLike;
  refreshServerSettings(): Promise<unknown>;
  voiceCall(): WatchCall;
  relayMode(): string;
  onSettingsFrame(frame: WatchFrame): void;
  noteSelfAuthor(id: unknown): void;
  settleFromServer(key: string): void;
}

/** Following the stream of every channel, frame by frame. */
export interface WatchFrameOwner {
  startWatchAll(): void;
  stopWatchAll(): void;
  onWatchAllFrame(frame: WatchFrame): "reset" | undefined;
}

/** The watch-all frame reading over `deps`. */
export function createWatchFrame(deps: WatchFrameDependencies): WatchFrameOwner {
  /** Follow every watched channel for the call that has just connected. */
  function startWatchAll(): void {
    deps.watchAll().start();
  }

  /** Stop following every channel. Idempotent. */
  function stopWatchAll(): void {
    deps.watchAll().stop();
  }

  /**
   * One frame of the stream of every channel, its cursor (the cursor over every channel) already taken
   * down by `watchAll`, and dropped there on a reset. A message is relayed as one from the channel's own
   * stream is, under the same guards and the same ledger.
   *
   */
  function onWatchAllFrame(frame: WatchFrame): "reset" | undefined {
    if (frame.event === "reset") {
      deps.refreshServerSettings().catch(() => {});
      if (deps.voiceCall().canSendText() && deps.relayMode() !== "off") deps.voiceCall().reportUnrelayed([], "gap");
      return "reset";
    }
    if (frame.event === "settings") {
      deps.onSettingsFrame(frame);
      return undefined;
    }
    if (frame.event !== "message" || !frame.data) return undefined;
    let payload: LiveMessageEvent | null = null;
    try {
      payload = JSON.parse(frame.data) as LiveMessageEvent;
    } catch (_error) {
      return undefined;
    }
    if (!is("LiveMessageEvent", payload)) return undefined;
    if (payload.self_posted) deps.noteSelfAuthor(payload.message.author_id);
    if (payload.self_posted && typeof payload.client_key === "string") deps.settleFromServer(payload.client_key);
    deps.voiceCall().relayToAgent(payload.message, payload.self_posted, payload.replayed, {
      fromTail: payload.from_tail === true,
      resumed: deps.watchAll().resumed(),
    });
    return undefined;
  }

  return { startWatchAll, stopWatchAll, onWatchAllFrame };
}

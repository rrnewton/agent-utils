// Clear unread in the chat service's own app, for one channel or for every channel at once (`#232
// upstream-mark-read`, `#253 upstream-clear-polish`, `#349 channel-settings-all`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 16, view/settings).
//
// vibe-talk reads only some of the owner's channels, mostly coding agents posting updates, and in the
// chat service's own app those updates bury the messages from people. With this on for a channel, the
// SERVER marks every message it receives there read in that app too, so the app's unread list keeps the
// people and vibe-talk keeps the agents. The choice is the server's, not this device's: the server does
// it with no page open, so the box reads and writes /upstream-clear and keeps nothing locally. It is
// offered only where the channel's own service can do it and only to a token that may change it (the
// page's `upstreamClearOffered`), and the warning beside it is always on screen, because what it does to
// the other app is the whole decision.
//
// ALL CHANNELS (`#349`). The owner, 2026-10-18: the channel drop-down gains "All channels", "Clear
// unread can be set for all at once", and it shows "Mixed" when the channels differ. With All channels
// chosen the box reads every channel whose chat service can clear unread (the page's
// `upstreamClearAllChannels`, so a channel whose service cannot is left out and said to be), shows the
// mode they share or Mixed, and Save puts the chosen mode on each, one PUT per channel, through the same
// /upstream-clear the one-channel box writes. "Mark read now" is a push for one channel and is not
// offered. The one-channel box's state is set aside while this is up, so nothing it is still waiting on
// can draw over it.

import type { Contract, UpstreamClearMode, UpstreamClearNowResponse, UpstreamClearState } from "../../contract/index.ts";
import { upstreamClearNowSentence, upstreamClearSentence } from "../../present/notices/index.ts";
import {
  UPSTREAM_CLEAR_MODE_WORDS,
  UPSTREAM_CLEAR_RECHECK_LIMIT_MS,
  UPSTREAM_CLEAR_RECHECK_MS,
  upstreamClearAllCommon,
  upstreamClearAllSavedWords,
  upstreamClearAllSentence,
  upstreamClearAttemptKey,
  upstreamClearPressSettled,
  upstreamClearPushingWords,
  upstreamClearReadFailure,
  upstreamClearUnansweredWords,
} from "../../present/settings/index.ts";

/** A request's failure as the page's server reports one: the server's own words, or the error's. */
type RequestFailure = { readonly detail?: string; readonly message?: string };

/** What the box asks of the page. */
export interface UpstreamClearDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** A request to the page's server, its answer checked against the contract (`apiDecoded`). */
  decoded<K extends keyof Contract>(type: K, path: string, options?: { method: string; body?: unknown }): Promise<Contract[K]>;
  /** Whether the box offers the control for the channel `id` (`upstreamClearOffered`). */
  offered(id: string): boolean;
  /** The channels All channels reaches (`upstreamClearAllChannels`). */
  allChannels(): string[];
  /** Whether the picker is on All channels (`allChannelsChosen`). */
  allChannelsChosen(): boolean;
  /** What the channel's chat service is called (`chatServiceOf`). */
  serviceOf(id: string): string;
  /** How many channels Settings' picker offers (`settingsChannels`). */
  channelCount(): number;
  /** What the page calls the channel with this id (`channelName`). */
  nameOf(id: string): string;
  /** The page's timers. */
  setTimeout(run: () => void, ms: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
}

/** The box's named commands. */
export interface UpstreamClearBox {
  /** Redraw the box for the channel its picker is on, or for All channels, reading the server when it changed. */
  render(): void;
  /** Read the box's channel, or every channel, again, as when Settings opens: the server may have pushed since. */
  refresh(): void;
  /** Enable Save, and offer "Mark read now" or "Try it now", to match what is chosen and saved. */
  renderChoice(): void;
  /** Save the chosen mode, on the box's channel or on every channel All channels reaches. */
  save(): Promise<void>;
  /** "Mark read now", or "Try it now" in a dry run: the server pushes the channel's newest message at once. */
  push(): Promise<void>;
}

/** Nothing read, nothing pressed. Made once, where the page wires its Settings. */
export function mountUpstreamClear(deps: UpstreamClearDependencies): UpstreamClearBox {
  /**
   * What the server last said about the channel the box is on: its id, its answer or null, and the
   * failure to show instead when there is no answer.
   */
  let upstreamClear: { channel: string; view: UpstreamClearState | null; failure: string } | null = null;
  /** Counts reads, so an answer for a channel the box has since left is dropped rather than drawn. */
  let upstreamClearReads = 0;
  /**
   * The channel a "Mark read now" press is waiting on the server for, or null. One press is one
   * push: the button stays disabled until the answer, so a second tap cannot send a second request.
   * `#253 upstream-clear-polish`.
   */
  let upstreamClearPushing: string | null = null;
  /**
   * A press the server answered `waiting`, which the box is still asking about, or null. `mode` is
   * the mode it was pressed in, `messageAtMs` the instant of the message it asked to push, and
   * `before` the attempt the server had last recorded when it answered, so a newer one is
   * recognised. `#253 upstream-clear-polish`.
   */
  let upstreamClearFollowing: {
    channel: string; service: string; mode: UpstreamClearMode;
    messageAtMs: number | null; before: string; asked: number; timer: ReturnType<typeof setTimeout> | 0;
  } | null = null;
  /**
   * `#349 channel-settings-all`. What the server last said about each channel All channels reaches: the
   * channels (`ids`), each answer, each failure to read one, whether the owner has moved the choice
   * since it was drawn (`chose`), and whether a Save is under way.
   */
  let upstreamClearAll: {
    ids: string[]; views: Map<string, UpstreamClearState>; failures: Map<string, string>; chose: boolean; saving: boolean;
  } | null = null;
  /** Counts the box's reads of every channel, so an answer to an older one is dropped. */
  let upstreamClearAllReads = 0;

  const picked = (): string => (deps.lookup("settings-channel") as HTMLSelectElement).value;
  const modeSelect = (): HTMLSelectElement => deps.lookup("upstream-clear-mode") as HTMLSelectElement;
  const failureOf = (error: unknown): string | undefined => (error as RequestFailure).detail || (error as RequestFailure).message;
  const clearPath = (channel: string, tail = ""): string => `/api/v1/channels/${encodeURIComponent(channel)}/upstream-clear${tail}`;
  const state = (text: string): void => {
    deps.lookup("upstream-clear-state").textContent = text;
  };

  /** Stop asking about a press the server answered `waiting`. */
  function stopFollowing(): void {
    if (upstreamClearFollowing) deps.clearTimeout(upstreamClearFollowing.timer as ReturnType<typeof setTimeout>);
    upstreamClearFollowing = null;
  }

  /**
   * Ask again about a press the server answered `waiting`, every UPSTREAM_CLEAR_RECHECK_MS, until
   * something answers it or UPSTREAM_CLEAR_RECHECK_LIMIT_MS have passed, and say what it came to.
   * Only while Settings shows the press's channel. `#253 upstream-clear-polish`.
   */
  function follow(channel: string, answer: UpstreamClearNowResponse, service: string): void {
    stopFollowing();
    const press: NonNullable<typeof upstreamClearFollowing> = {
      channel,
      service,
      mode: answer.state.mode,
      messageAtMs: typeof answer.message_at_ms === "number" ? answer.message_at_ms : null,
      before: upstreamClearAttemptKey(answer.state.last),
      asked: 0,
      timer: 0,
    };
    upstreamClearFollowing = press;
    const here = (): boolean => upstreamClearFollowing === press && Boolean(upstreamClear) &&
      (upstreamClear as NonNullable<typeof upstreamClear>).channel === channel && !deps.lookup("screen-settings").hidden;
    const ask = async (): Promise<void> => {
      if (!here()) {
        if (upstreamClearFollowing === press) upstreamClearFollowing = null;
        return;
      }
      press.asked += 1;
      const read = ++upstreamClearReads;
      let view: UpstreamClearState | null = null;
      try {
        view = await deps.decoded("UpstreamClearState", clearPath(channel));
      } catch (_error) {
        // Unread this time; asked again at the next tick, as long as the bound allows.
        view = null;
      }
      if (!here()) return;
      if (view && read === upstreamClearReads) adopt(channel, view);
      const settled = view ? upstreamClearPressSettled(press, view) : null;
      if (settled) {
        upstreamClearFollowing = null;
        state(upstreamClearNowSentence(settled, service));
        return;
      }
      if ((press.asked + 1) * UPSTREAM_CLEAR_RECHECK_MS > UPSTREAM_CLEAR_RECHECK_LIMIT_MS) {
        upstreamClearFollowing = null;
        state(upstreamClearUnansweredWords(service));
        return;
      }
      state(upstreamClearNowSentence({ outcome: "waiting", state: view || answer.state }, service));
      press.timer = deps.setTimeout(ask, UPSTREAM_CLEAR_RECHECK_MS);
    };
    press.timer = deps.setTimeout(ask, UPSTREAM_CLEAR_RECHECK_MS);
  }

  /** Whether Save would change anything: a mode other than the server's. */
  function changed(): boolean {
    const stored = upstreamClear && upstreamClear.view ? upstreamClear.view.mode : null;
    return stored !== null && modeSelect().value !== stored;
  }

  function renderChoice(): void {
    // `#349 channel-settings-all`. With All channels chosen the choice is about every channel at once.
    if (deps.allChannelsChosen()) {
      if (upstreamClearAll) upstreamClearAll.chose = true;
      renderAllChoice();
      return;
    }
    const view = upstreamClear ? upstreamClear.view : null;
    const stored = view ? view.mode : "off";
    (deps.lookup("upstream-clear-save") as HTMLButtonElement).disabled = !changed();
    // The button acts on the SAVED mode, so it is offered only while the choice still matches it:
    // "Mark read now" beside an unsaved Off would mark messages read in a channel being turned off.
    const push = deps.lookup("upstream-clear-push") as HTMLButtonElement;
    push.hidden = !view || stored === "off" || modeSelect().value !== stored || Boolean(view.unavailable);
    push.textContent = stored === "dry_run" ? "Try it now" : "Mark read now";
    push.disabled = upstreamClearPushing !== null;
  }

  function render(): void {
    if (deps.allChannelsChosen()) {
      renderAll();
      return;
    }
    upstreamClearAll = null;
    const id = picked();
    const offered = deps.offered(id);
    deps.lookup("upstream-clear").hidden = !offered;
    if (!offered) return;
    // What All channels changed in the box, put back for one channel.
    deps.lookup("upstream-clear-scope").textContent = "this channel";
    deps.lookup("upstream-clear-mixed").hidden = true;
    modeSelect().disabled = false;
    const service = deps.serviceOf(id);
    deps.lookup("upstream-clear-provider").textContent = service;
    deps.lookup("upstream-clear-warning-provider").textContent = service;
    deps.lookup("upstream-clear-threads-provider").textContent = service;
    if (!upstreamClear || upstreamClear.channel !== String(id)) {
      upstreamClear = { channel: String(id), view: null, failure: "" };
      modeSelect().value = "off";
      state("Checking with the server…");
      renderChoice();
      // Asked only while Settings is showing: a page that never opens it never spends the request,
      // and opening it asks (`refresh`).
      if (!deps.lookup("screen-settings").hidden) load(String(id));
      return;
    }
    draw();
  }

  /**
   * Draw what the server last said, or why there is nothing to draw.
   *
   * A mode the owner has chosen and not saved is not the server's to take back. `#253
   * upstream-clear-polish`: after a "Mark read now" the server answered `waiting`, the box reads the
   * channel every few seconds for up to thirty, and each read put the saved mode back in the select,
   * so a choice made meanwhile went back under the owner's hand and Save went grey. So the select
   * follows the server only while it still shows the mode saved before this answer, `savedBefore`; a
   * choice moved off it stays, with Save offered for it, until it is saved or put back.
   *
   * @param savedBefore The saved mode the box showed before this answer, or null when it showed none:
   *   the box's first answer for a channel sets the select.
   */
  function draw(savedBefore: UpstreamClearMode | null = upstreamClear && upstreamClear.view ? upstreamClear.view.mode : null): void {
    if (!upstreamClear) return;
    const view = upstreamClear.view;
    if (!view) {
      if (upstreamClear.failure) state(upstreamClear.failure);
      renderChoice();
      return;
    }
    const select = modeSelect();
    if (savedBefore === null || select.value === savedBefore) select.value = view.mode;
    state(upstreamClearSentence(view, view.provider_name || deps.serviceOf(upstreamClear.channel)));
    renderChoice();
  }

  /** Take the server's answer for `channel` as the truth, unless the box has moved on. */
  function adopt(channel: string, view: UpstreamClearState): void {
    if (!upstreamClear || upstreamClear.channel !== channel) return;
    const savedBefore = upstreamClear.view ? upstreamClear.view.mode : null;
    upstreamClear = { channel, view, failure: "" };
    draw(savedBefore);
  }

  /** Ask the server what it does for `channel`. */
  async function load(channel: string): Promise<void> {
    const read = ++upstreamClearReads;
    try {
      const view = await deps.decoded(
        "UpstreamClearState",
        clearPath(channel)
      );
      if (read === upstreamClearReads) adopt(channel, view);
    } catch (error) {
      if (read !== upstreamClearReads || !upstreamClear || upstreamClear.channel !== channel) return;
      upstreamClear = {
        channel,
        view: null,
        failure: upstreamClearReadFailure(failureOf(error)),
      };
      draw();
    }
  }

  /** Enable Save only for a mode that would change something, once every channel has been read. */
  function renderAllChoice(): void {
    const held = upstreamClearAll;
    const common = upstreamClearAllCommon(held);
    const chosen = modeSelect().value;
    modeSelect().disabled = common === undefined;
    (deps.lookup("upstream-clear-save") as HTMLButtonElement).disabled = !held || held.saving || common === undefined || chosen === "mixed" ||
      chosen === common;
    deps.lookup("upstream-clear-push").hidden = true;
  }

  /** The status line for All channels. */
  const allSentence = (): string => upstreamClearAllSentence(upstreamClearAll, deps.channelCount(), deps.nameOf);

  /** Draw what the server said about every channel: their mode, or Mixed, and the status line. */
  function drawAll(): void {
    const held = upstreamClearAll;
    if (!held) return;
    const common = upstreamClearAllCommon(held);
    deps.lookup("upstream-clear-mixed").hidden = common !== null;
    if (!held.chose) modeSelect().value = common === undefined ? "off" : common === null ? "mixed" : common;
    state(allSentence());
    renderAllChoice();
  }

  /** Redraw the box for All channels, reading every channel's state when the set of them changed. */
  function renderAll(): void {
    // The one-channel box's own state is set aside: nothing it waits on draws over this.
    stopFollowing();
    upstreamClear = null;
    const ids = deps.allChannels();
    deps.lookup("upstream-clear").hidden = ids.length === 0;
    if (ids.length === 0) {
      upstreamClearAll = null;
      return;
    }
    const services = [...new Set(ids.map((id) => deps.serviceOf(id)))];
    const service = services.length === 1 ? services[0] as string : "their chat services";
    deps.lookup("upstream-clear-provider").textContent = service;
    deps.lookup("upstream-clear-warning-provider").textContent = service;
    deps.lookup("upstream-clear-threads-provider").textContent = service;
    deps.lookup("upstream-clear-scope").textContent = "each of these channels";
    if (!upstreamClearAll || upstreamClearAll.ids.join(" ") !== ids.join(" ")) {
      upstreamClearAll = { ids, views: new Map(), failures: new Map(), chose: false, saving: false };
      // Asked only while Settings is showing, as the one-channel box asks.
      if (!deps.lookup("screen-settings").hidden) loadAll();
    }
    drawAll();
  }

  /** Ask the server what it does for every channel All channels reaches, one channel after another. */
  async function loadAll(): Promise<void> {
    const held = upstreamClearAll;
    if (!held) return;
    const read = ++upstreamClearAllReads;
    held.failures.clear();
    for (const id of held.ids) {
      try {
        const view = await deps.decoded("UpstreamClearState", clearPath(id));
        if (read !== upstreamClearAllReads || upstreamClearAll !== held) return;
        held.views.set(id, view);
      } catch (error) {
        if (read !== upstreamClearAllReads || upstreamClearAll !== held) return;
        held.failures.set(id, failureOf(error) as string);
      }
    }
    if (deps.allChannelsChosen()) drawAll();
  }

  /**
   * Save the chosen mode on every channel All channels reaches, and say how many it reached. A channel
   * that refuses keeps what it had, and is named.
   */
  async function saveAll(): Promise<void> {
    const held = upstreamClearAll;
    const mode = modeSelect().value;
    if (!held || held.saving || !Object.prototype.hasOwnProperty.call(UPSTREAM_CLEAR_MODE_WORDS, mode) ||
        upstreamClearAllCommon(upstreamClearAll) === undefined) return;
    held.saving = true;
    renderAllChoice();
    state("Saving…");
    const refused: string[] = [];
    for (const id of held.ids) {
      try {
        const view = await deps.decoded("UpstreamClearState", clearPath(id),
          { method: "PUT", body: { mode } });
        held.views.set(id, view);
      } catch (error) {
        refused.push(`${deps.nameOf(id)} (${failureOf(error)})`);
      }
    }
    held.saving = false;
    if (upstreamClearAll !== held || !deps.allChannelsChosen()) return;
    held.chose = false;
    drawAll();
    state(upstreamClearAllSavedWords(refused, held.ids.length, allSentence));
  }

  return {
    render,
    refresh() {
      if (deps.allChannelsChosen()) {
        if (upstreamClearAll) loadAll();
        else render();
        return;
      }
      const id = picked();
      if (!deps.offered(id)) return;
      if (upstreamClear && upstreamClear.channel === String(id)) {
        load(String(id));
      } else {
        render();
      }
    },
    renderChoice,
    /**
     * Turning it on pushes nothing by itself: the next message that arrives is marked, and everything
     * before it, as the warning says; "Mark read now" does it at once.
     */
    async save() {
      if (deps.allChannelsChosen()) {
        await saveAll();
        return;
      }
      const channel = picked();
      if (!deps.offered(channel) || !changed()) return;
      const mode = modeSelect().value;
      (deps.lookup("upstream-clear-save") as HTMLButtonElement).disabled = true;
      state("Saving…");
      // A save is the owner's newest word on the channel: a press still being asked about is not.
      stopFollowing();
      let view: UpstreamClearState;
      try {
        view = await deps.decoded(
          "UpstreamClearState",
          clearPath(channel),
          { method: "PUT", body: { mode } }
        );
      } catch (error) {
        state(`Not saved: ${failureOf(error)}`);
        renderChoice();
        return;
      }
      upstreamClearReads += 1;
      adopt(String(channel), view);
    },
    /**
     * `#253 upstream-clear-polish`. Through the server's own loop, which answers what that came to, and
     * that goes in the status line. One press, one request: the button is disabled from the press until
     * the answer. It never changes the mode; the server refuses it for a channel another device has
     * turned off.
     */
    async push() {
      const channel = String(picked());
      const view = upstreamClear && upstreamClear.channel === channel ? upstreamClear.view : null;
      if (!view || view.mode === "off" || upstreamClearPushing !== null || !deps.offered(channel)) return;
      const service = view.provider_name || deps.serviceOf(channel);
      stopFollowing();
      upstreamClearPushing = channel;
      renderChoice();
      state(upstreamClearPushingWords(view.mode, service));
      let answer: UpstreamClearNowResponse | null = null;
      let failure = "";
      try {
        answer = await deps.decoded(
          "UpstreamClearNowResponse",
          clearPath(channel, "/now"),
          { method: "POST" }
        );
      } catch (error) {
        failure = `Not done: ${failureOf(error)}`;
      }
      upstreamClearPushing = null;
      const here = Boolean(upstreamClear && upstreamClear.channel === channel);
      if (!answer) {
        if (here) state(failure);
        renderChoice();
        return;
      }
      upstreamClearReads += 1;
      adopt(channel, answer.state);
      if (!here) {
        renderChoice();
        return;
      }
      state(upstreamClearNowSentence(answer, service));
      // The server stopped waiting before the push landed; ask again until something answers it.
      if (answer.outcome === "waiting") follow(channel, answer, service);
    },
  };
}

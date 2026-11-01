// Saved messages on this server (`#208 persistent-cache`; `#624 voice-page-modularity`, moved from the
// legacy script; plan step 16, view/settings).
//
// The server can keep its own copy of the channel messages it reads, in a file of its own beside its
// store. That is NOT this browser's saved rows (the page's `readMessageCache`, the reader's offline
// copy): it is the SERVER's file, switched by the owner and measured by the server. So, like the voice
// switch, it is drawn from client-config and redrawn from what each PUT or DELETE answered, never from
// the click.
//
//   * SHOWN ONLY WHERE THE SERVER REPORTS IT. A server whose configuration has no [message_cache]
//     table sends no status, and no server sends one to a read token, which cannot switch it. The
//     page hides it from anything but a write token as well, so a status that reached one anyway
//     shows nothing. The help entry is hidden with the group.
//   * BOTH ERASING ACTS ASK FIRST. Unticking turns saving off and erases the file; Erase empties it
//     and carries on. Each puts its question in `#message-cache-confirm`, the box goes on showing
//     what the server holds until the answer, and only the question's own button sends. Keep sends
//     nothing at all.
//   * TURNING IT ON SENDS AT ONCE: it erases nothing. The server refuses it where its own
//     configuration turns the cache off — the operator's word wins — and the line says why.

import type { Contract, MessageCacheStatus } from "../../contract/index.ts";
import { serverCacheErases, serverCacheLine } from "../../present/notices/index.ts";
import {
  SERVER_CACHE_ASK,
  SERVER_CACHE_SENDING,
  serverCacheKeepsWords,
  serverCacheUnconfirmed,
  serverHoldsWholeHistory,
} from "../../present/settings/index.ts";
import type { ServerCacheAct } from "../../present/settings/index.ts";

/** A request's failure as the page's server reports one: the server's own words, or the error's. */
type RequestFailure = { readonly detail?: string; readonly message?: string };

/** What the group asks of the page. */
export interface ServerCacheDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** A request to the page's server, its answer checked against the contract (`apiDecoded`). */
  decoded<K extends keyof Contract>(type: K, path: string, options?: { method: string; body?: unknown }): Promise<Contract[K]>;
  /** Whether this token may change the server's settings (`tokenScope === "write"`). */
  writable(): boolean;
  /** Redraw Settings' Expand all, which counts only the folds on the page (`settingsFolds.renderAll`). */
  renderFolds(): void;
  /** Redraw the channel box's history, which follows whether a file is open (`channelHistoryBox.render`). */
  renderHistory(): void;
}

/** The group's read surface and named commands. */
export interface ServerCache {
  /** Whether the server reports a cache whose file is open. */
  open(): boolean;
  /** Whether the server says it holds every message of every channel it caches. `#595 cache-full-history`. */
  holdsWholeHistory(): boolean;
  /** Take down what client-config said: the status, or null when it carries none. */
  apply(status: MessageCacheStatus | null | undefined): void;
  /** The switch, as the owner's finger left it: on sends at once, off asks first. */
  switched(): Promise<void>;
  /** Ask before an act that erases the file, and wait for the answer at the safe button. */
  ask(act: "off" | "erase"): void;
  /** The answer to the question: its own button sends; Keep sends nothing and goes back where it asked. */
  answer(yes: boolean): Promise<void>;
}

/** No status reported, nothing asked or sent. Made where the page keeps the server's settings. */
export function mountServerCache(deps: ServerCacheDependencies): ServerCache {
  /** The status as the server last reported it, or null when it reports none. */
  let serverCache: MessageCacheStatus | null = null;
  /** What is on its way to the server — "on", "off" or "erase" — or null, so a second press cannot race it. */
  let serverCacheSending: ServerCacheAct | null = null;
  /** The erasing act the owner is being asked about — "off" or "erase" — or null. */
  let serverCacheAsking: "off" | "erase" | null = null;

  /**
   * Draw the group, the limits in its help entry, and any question, from `serverCache`. `note` leads
   * the line: what a request just did, or why it did not.
   */
  function render(note = ""): void {
    const shown = serverCache !== null && deps.writable();
    deps.lookup("message-cache-group").hidden = !shown;
    deps.lookup("help-message-cache").hidden = !shown;
    // `#351 settings-polish`. As with the agent's console link: the group's fold counts for Expand all only shown.
    deps.renderFolds();
    if (!shown) {
      serverCacheAsking = null;
      deps.lookup("message-cache-confirm").hidden = true;
      return;
    }
    const status = serverCache as MessageCacheStatus;
    deps.lookup("message-cache-help-days").textContent = serverCacheKeepsWords(status);
    deps.lookup("message-cache-help-megabytes").textContent = `${status.max_megabytes} MB`;
    const busy = serverCacheSending !== null;
    const box = deps.lookup("message-cache-enabled") as HTMLInputElement;
    // Ticked while the server says it is on, the question about turning it off included: nothing has
    // changed until the owner answers.
    box.checked = status.enabled;
    box.disabled = busy || status.reason === "config_disabled";
    (deps.lookup("message-cache-erase") as HTMLButtonElement).disabled = busy || !status.available;
    const line = busy ? SERVER_CACHE_SENDING[serverCacheSending as ServerCacheAct] : serverCacheLine(status);
    deps.lookup("message-cache-state").textContent = note ? `${note} ${line}` : line;
    const asking = busy ? null : serverCacheAsking;
    deps.lookup("message-cache-confirm").hidden = asking === null;
    // While a question is up its own two buttons are the way on; a third that erases would blur it.
    deps.lookup("message-cache-erase").hidden = asking !== null;
    if (asking !== null) {
      const words = SERVER_CACHE_ASK[asking];
      deps.lookup("message-cache-confirm-text").textContent = words.ask(serverCacheErases(status));
      deps.lookup("message-cache-confirm-yes").textContent = words.yes;
      deps.lookup("message-cache-confirm-no").textContent = words.no;
    }
  }

  function ask(act: "off" | "erase"): void {
    if (serverCache === null || serverCacheSending !== null) return;
    if (act === "erase" && !serverCache.available) return;
    serverCacheAsking = act;
    render();
    deps.lookup("message-cache-confirm-no").focus();
  }

  /** Send one switch, then draw what the server says it LEFT; on failure, what it last said, and why. */
  async function send(act: ServerCacheAct): Promise<void> {
    if (serverCacheSending !== null) return;
    serverCacheSending = act;
    render();
    let status: MessageCacheStatus | null = null;
    try {
      status = await deps.decoded(
        "MessageCacheStatus",
        "/api/v1/message-cache",
        act === "erase" ? { method: "DELETE" } : { method: "PUT", body: { enabled: act === "on" } }
      );
    } catch (error) {
      serverCacheSending = null;
      render(serverCacheUnconfirmed(String((error as RequestFailure).detail || (error as RequestFailure).message)));
      return;
    }
    serverCacheSending = null;
    serverCache = status;
    render(act === "erase" ? "Erased." : "");
  }

  return {
    open: () => Boolean(serverCache && serverCache.available),
    holdsWholeHistory: () => serverHoldsWholeHistory(serverCache),
    apply(status) {
      serverCache = status && typeof status.enabled === "boolean" ? status : null;
      serverCacheAsking = null;
      render();
      // `#602 history-settings-and-limits`. The channel box's history follows whether a file is open.
      deps.renderHistory();
    },
    async switched() {
      if (serverCache === null) return;
      if ((deps.lookup("message-cache-enabled") as HTMLInputElement).checked) {
        serverCacheAsking = null;
        await send("on");
        return;
      }
      ask("off");
    },
    ask,
    async answer(yes) {
      const act = serverCacheAsking;
      serverCacheAsking = null;
      if (!yes || act === null) {
        render();
        deps.lookup(act === "erase" ? "message-cache-erase" : "message-cache-enabled").focus();
        return;
      }
      await send(act);
    },
  };
}

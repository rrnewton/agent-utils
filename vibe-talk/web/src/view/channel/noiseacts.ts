// Messages read automatically: how many loaded ones the server calls noise, and
// rescuing ones a rule caught by mistake (`#624 voice-page-modularity`, moved from the
// legacy script). Words and order are as they were in the script. The verdict itself is
// the server's, carried by present/rows' `isNoise`; nothing here matches text.

import { isNoise } from "../../present/rows/index.ts";
import type { ChannelStore } from "../../core/channels/index.ts";
import type { Message } from "../../contract/index.ts";

/** A message whose noise verdict this owner may clear. */
interface NoiseMessage { id: unknown; noise?: boolean }

/** What tallying and rescuing noise asks of the page. */
export interface NoiseActsDependencies {
  providerFacts: { threading(): boolean };
  readModes(): { hiding(): boolean };
  todoView(): { state(): { noise?: number; window?: number } };
  timelineRows(): { messages: Message[] };
  el(id: string): { children: Iterable<unknown> };
  rowMessages(row: unknown): Message[];
  selectedChannelId(): string;
  api(path: string, options?: unknown): Promise<unknown>;
  channelStore: ChannelStore;
  eachColumn(act: () => void): void;
  guardQuietly(fn: () => Promise<unknown>): () => unknown;
  refreshAfterInboxChange(): Promise<unknown>;
  setStatus(text: string): void;
}

/** Tallying noise and rescuing messages from it. */
export interface NoiseActs {
  noiseTally(): { matched: number; total: number };
  markNotNoise(ids: readonly unknown[]): Promise<void>;
}

/** The noise acts over `deps`. */
export function createNoiseActs(deps: NoiseActsDependencies): NoiseActs {
  /**
   * How many of the LOADED messages the server calls noise, and out of how many.
   *
   * The rows on screen, or the threaded view's messages — except in the plain to-do list, where the
   * server already left the placeholders out and said how many it left, so that number is the one.
   */
  function noiseTally(): { matched: number; total: number } {
    if (!deps.providerFacts.threading() && deps.readModes().hiding()) {
      return { matched: deps.todoView().state().noise || 0, total: deps.todoView().state().window || 0 };
    }
    const loaded = deps.providerFacts.threading()
      ? deps.timelineRows().messages
      : [...deps.el("discord-log").children].flatMap(deps.rowMessages);
    return { matched: loaded.filter(isNoise).length, total: loaded.length };
  }

  /**
   * Rescue messages a rule caught by mistake: the server records them as not noise, and every copy
   * of them this page holds is told the same, so the row is ordinary at once rather than after the
   * next read.
   *
   * Not optimistic, unlike Done. Done is undoable and its failure puts back a row the reader can
   * see; a rescue that failed silently would leave him believing a message is safe from a rule that
   * is still hiding it.
   */
  async function markNotNoise(ids: readonly unknown[]): Promise<void> {
    const channel = deps.selectedChannelId();
    const rescued = new Set(ids.map(String));
    if (!channel || rescued.size === 0) return;
    await deps.api(`/api/v1/channels/${encodeURIComponent(channel)}/not-noise`, {
      method: "POST",
      body: { messages: [...rescued] },
    });
    const copies: NoiseMessage[] = [...deps.channelStore.current().messages];
    // Every column's copies: the message's verdict is not one column's. `#229 desktop-two-column`.
    deps.eachColumn(() => copies.push(...deps.timelineRows().messages, ...[...deps.el("discord-log").children].flatMap(deps.rowMessages)));
    for (const message of copies) {
      if (rescued.has(String(message.id))) message.noise = false;
    }
    if (deps.providerFacts.threading()) deps.eachColumn(() => deps.guardQuietly(deps.refreshAfterInboxChange)());
    else await deps.refreshAfterInboxChange();
    const count = rescued.size;
    deps.setStatus(
      `${count} message${count === 1 ? "" : "s"} no longer read automatically — the rule still applies to the others.`
    );
  }

  return { noiseTally, markNotNoise };
}

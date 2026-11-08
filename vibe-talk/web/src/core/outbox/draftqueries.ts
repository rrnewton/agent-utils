// Which drafts are held, and whether one is live (`#624 voice-page-modularity`, moved
// from the legacy script into core/outbox; mu-readcore): the draft ledger's readings for
// the chip — held, live (its row still on its way), and here. Writing the ledger stays
// with each writer's existing calls; this owner only reads, so reply-draft persistence
// timing is untouched.

/** The ledger as queries read it. */
import type { DraftEntry } from "./draftledger.ts";
import { draftIdentity } from "./draftledger.ts";

/** The ledger as queries read it. */
export interface DraftLedgerLike {
  inChannel(channel: string, live: (draft: DraftEntry) => boolean): DraftEntry[];
}

/** What draft queries ask of the page. */
export interface DraftQueriesDependencies {
  ledger: DraftLedgerLike;
  composerDraft(key: string): string | undefined;
  composerDraftKeys(): Iterable<string>;
  replyDraft(key: string): string | undefined;
  replyDraftIds(): Iterable<string>;
  quickDraft(key: string): string | undefined;
  quickDraftKeys(): Iterable<string>;
  quickDraftMessage(key: string): string;
  storeNow(): { channel: string; removed: { has(id: string): boolean }; messages: readonly { id: unknown }[] };
  selectedChannelId(): string;
}

/** Draft queries' commands: the legacy names, unchanged. */
export interface DraftQueries {
  draftHeld(draft: DraftEntry): boolean;
  draftLive(draft: DraftEntry): boolean;
  draftsHere(): DraftEntry[];
}

/** Draft queries over `deps`. */
export function createDraftQueries(deps: DraftQueriesDependencies): DraftQueries {
  const draftLedger = deps.ledger;
  const channelDrafts = { get: (key: string) => deps.composerDraft(key), keys: () => deps.composerDraftKeys() };
  const replyDrafts = { get: (key: string) => deps.replyDraft(key), ids: () => deps.replyDraftIds() };
  const quickDrafts = { get: (key: string) => deps.quickDraft(key), keys: () => deps.quickDraftKeys() };
  const quoteOwner = { quickDraftMessage: (key: string) => deps.quickDraftMessage(key) };
  const channelStore = { current: () => deps.storeNow() };
  const selectedChannelId = deps.selectedChannelId;

function draftHeld(draft: DraftEntry): boolean {
  const store = draft.kind === "composer" ? channelDrafts : draft.kind === "reply" ? replyDrafts : quickDrafts;
  return Boolean((store.get(draft.key) || "").trim());
}

function draftLive(draft: DraftEntry): boolean {
  if (!draftHeld(draft)) return false;
  if (draft.kind === "composer") return true;
  const message = draft.kind === "reply" ? draft.key : quoteOwner.quickDraftMessage(draft.key);
  return !(channelStore.current().channel === draft.channel && channelStore.current().removed.has(message));
}

function draftsHere(): DraftEntry[] {
  const channel = String(selectedChannelId() || "");
  if (!channel) return [];
  const known = draftLedger.inChannel(channel, draftLive);
  const seen = new Set(known.map(draftIdentity));
  /** @type {import("./src/core/outbox/index.ts").DraftEntry[]} */
  const more: DraftEntry[] = [];
  const add = (kind: string, key: string) => {
    const draft: DraftEntry = { kind: kind as DraftEntry["kind"], key, channel, at: 0 };
    if (!seen.has(draftIdentity(draft)) && draftLive(draft)) more.push(draft);
  };
  for (const key of channelDrafts.keys()) {
    let parts = null;
    try {
      parts = JSON.parse(key);
    } catch (_error) {
      parts = null;
    }
    if (Array.isArray(parts) && String(parts[0]) === channel) add("composer", key);
  }
  for (const key of quickDrafts.keys()) if (key.startsWith(`${channel}\n`)) add("quick", key);
  if (channelStore.current().channel === channel) {
    const held = new Set(channelStore.current().messages.map((message) => String(message.id)));
    for (const id of replyDrafts.ids()) if (held.has(id)) add("reply", id);
  }
  return [...known, ...more];
}

  return { draftHeld, draftLive, draftsHere };
}

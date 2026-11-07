// The device's saved copy of the channels' rows, and the shell that lets a reload draw before
// `/client-config` answers (`#18 offline-message-cache`; `#624 voice-page-modularity`, moved from
// the legacy script). One envelope under one key, scoped to the token that read the rows: anything
// that disagrees with that token — another token, no token, a refusal — deletes the whole envelope
// rather than filtering it, because messages that credential can no longer read must not stay on
// the device. What a load does with an entry once it has it (restoring the store, the bookmark,
// thread colours) and what a save gathers to put in one (the store's shape is `saved.ts`'s) stay
// with the page; this is only the envelope's side of it.

import { is } from "../../contract/index.ts";
import type { ChannelInfo, ClientConfigResponse, Message, ProviderDescription, ThreadSummary } from "../../contract/index.ts";
import type { StoragePort } from "../../ports/index.ts";
import { inViewOf, threadOf, timeOf, viewOfKey } from "./canon.ts";
import type { SavedTimeline } from "./saved.ts";

export const MESSAGE_CACHE_KEY = "vibe-talk.voice.message-cache";
export const MESSAGE_CACHE_VERSION = 1;
/** Rows kept per channel, newest first: its messages, and separately its thread cards. */
export const MESSAGE_CACHE_ROWS = 120;
/** Channels kept at once; the least recently opened goes first. */
export const MESSAGE_CACHE_SCOPES = 12;
/** The whole envelope, in UTF-16 code units — about 1.2 MB, leaving the quota to the outbox. */
export const MESSAGE_CACHE_CHARS = 600000;

/** A channel's rows as saved before timeline mode: one page, drawn as saved. */
export interface SavedPage {
  mode: "page";
  savedAt: number;
  usedAt: number;
  messages: Message[];
  threads: ThreadSummary[];
  more: boolean;
  cursor: string | null;
  /** `#578 search-status-clarity`: where the rows saved stop being one stretch, if they do. */
  gapFrom: number | null;
  dismissed: string[];
  bookmark?: unknown;
}

/** One channel's saved rows, in either mode the page has ever written. */
export type CacheEntry = SavedTimeline | SavedPage;

/**
 * What the page needs to draw before `/client-config` answers: the channels, the read mode, and
 * who "me" is, so a saved row is coloured as it was. `owner_author_ids` is read by the page's
 * hydrate but never written here (a pre-existing gap in the legacy shape, kept, not fixed).
 */
export interface CacheShell {
  channels: ChannelInfo[];
  threading_supported: boolean;
  self_author_id: string | null;
  owner_author_id: string | null;
  owner_author_ids?: string[];
  coding_agent_ids: string[];
  coding_agents_named_only: boolean;
  providers: ProviderDescription[];
}

/** The envelope for one token, as stored under `MESSAGE_CACHE_KEY`. */
export interface MessageCacheEnvelope {
  v: number;
  identity: string;
  shell: CacheShell | null;
  scopes: Record<string, CacheEntry>;
}

/** What the cache reads of the page besides its storage. */
export interface MessageCacheDependencies {
  /** The device's storage (`localStorage` on web): every call may throw, a write may be dropped. */
  readonly storage: StoragePort;
  /** The saved token now, or "" when there is none. Read on every use: it can be replaced live. */
  token(): string;
  /** The channels as the directory last had them, for `saveCacheChannels`; asked only then. */
  channels(): ChannelInfo[];
  /**
   * Whether `message` is still to do over the Done set `done` (the page's `stillToDo` with its
   * set given): a cut read reply is one more of its thread's the store lacks for being old; an
   * unread one is not (`#222 unread-replies`).
   */
  stillToDo(message: Message, done: ReadonlySet<string>): boolean;
}

/** The device message cache's commands and questions. */
export interface MessageCache {
  /**
   * The envelope for the token saved right now, or null when there is no token to scope it to.
   * Anything unreadable — an interrupted write, another version, another identity — is removed
   * and replaced by an empty envelope. A damaged entry is dropped on its own; the rest survive
   * it. The answer is the parsed object itself, never a copy: callers mark `usedAt` on it and
   * hand it back to `writeMessageCache`.
   */
  readMessageCache(): MessageCacheEnvelope | null;
  /**
   * Store the envelope inside its bounds, giving up space in a fixed order: other channels,
   * least recently opened first; then half of `keep` at a time; then the whole cache. It is a
   * convenience, so it never holds on to space the token, the drafts or an unsent message could
   * need. False when even the whole cache had to go.
   */
  writeMessageCache(cache: MessageCacheEnvelope, keep?: string | null): boolean;
  /** Forget the whole envelope: sign-out, another token, or a refusal. Never throws. */
  dropMessageCache(): void;
  /** Forget the saved rows of every channel `gone` answers true for. */
  forgetChannelScopes(gone: (key: string) => boolean): void;
  /**
   * Keep what the page needs to draw the channel before `/client-config` answers, from `config`.
   * Channels the server no longer lists go at the same moment — the credential can no longer
   * read them. The arrays `config` carries are held as they are, never copied.
   */
  saveCacheShell(config: ClientConfigResponse): void;
  /**
   * Carry an add, rename or removal made here into the saved channel list, so the next cold
   * start draws the picker as it now is, not as `/client-config` last described it.
   */
  saveCacheChannels(): void;
  /**
   * Keep the newest `limit` rows of an entry. A timeline entry is one channel's store, so
   * cutting it raises the floor of every view that lost a row — the view then says older history
   * exists rather than showing a gap. A timeline cursor is never kept: it is opaque and it
   * expires. A legacy page cursor is just the oldest id, so it is kept.
   */
  trimCacheEntry(entry: CacheEntry, limit: number): void;
  /** Write and read back: private browsing and a full quota both refuse silently or by throwing. */
  storedExactly(key: string, encoded: string): boolean;
}

/** A stable 64-bit label for a token. Not a secret and not a check: only "is it the same one". */
export function tokenFingerprint(value: string): string {
  if (!value) return "";
  let a = 0x811c9dc5;
  let b = 0x9747b28c;
  for (let i = 0; i < value.length; i += 1) {
    const c = value.charCodeAt(i);
    a = Math.imul(a ^ c, 0x01000193);
    b = Math.imul(b ^ c, 0x5bd1e995);
    b ^= b >>> 13;
  }
  const hex = (n: number): string => (n >>> 0).toString(16).padStart(8, "0");
  return hex(a) + hex(b);
}

const asRecord = (value: unknown): Record<string, unknown> | null =>
  value !== null && typeof value === "object" ? (value as Record<string, unknown>) : null;

const cachedRowsValid = (rows: unknown): boolean => Array.isArray(rows) && rows.every((row) =>
  row !== null && typeof row === "object" && (typeof (row as { id?: unknown }).id === "string" || typeof (row as { id?: unknown }).id === "number"));

/**
 * Saved messages and thread cards are server rows held across a reload, and storage is a boundary
 * like the network: another version of this page, an interrupted write, or anything else with
 * access to the origin may have left them. Each row must still match the wire contract, or its
 * entry is dropped exactly as a damaged one always was — the channel then reads fresh.
 */
const cachedWireRowsValid = (rows: unknown, type: "Message" | "ThreadSummary"): boolean =>
  Array.isArray(rows) && rows.every((row) => is(type, row));

const coverageValid = (views: unknown): boolean => views !== null && typeof views === "object" && !Array.isArray(views) &&
  Object.values(views).every((cover) => cover !== null && typeof cover === "object" &&
    ((cover as { floor?: unknown }).floor === null || Number.isFinite((cover as { floor?: unknown }).floor)) &&
    typeof (cover as { more?: unknown }).more === "boolean" &&
    Number.isFinite((cover as { at?: unknown }).at) && typeof (cover as { notice?: unknown }).notice === "string" &&
    // Absent from a cover saved before `#578 search-status-clarity`, whose next read then joins nothing.
    [(cover as { top?: unknown }).top, (cover as { dense?: unknown }).dense]
      .every((at) => at === undefined || at === null || Number.isFinite(at)));

function validCacheEntry(entry: unknown): entry is CacheEntry {
  const held = asRecord(entry);
  if (held === null || !Number.isFinite(held.usedAt) ||
      !cachedWireRowsValid(held.messages, "Message") ||
      !cachedWireRowsValid(held.threads, "ThreadSummary") ||
      !Array.isArray(held.dismissed)) return false;
  if (held.mode === "timeline") {
    return coverageValid(held.views) && typeof held.hasThreads === "boolean" &&
      (held.scope === undefined || held.scope === null || typeof held.scope === "string") &&
      // Absent from an entry saved before `#185 reply-new-thread`, which then has nothing unplaced.
      (held.unplaced === undefined ||
        (Array.isArray(held.unplaced) && held.unplaced.every((id) => typeof id === "string"))) &&
      // Absent from an entry saved before `#315 removed-reply-target`, which then holds nothing removed.
      (held.removed === undefined ||
        (Array.isArray(held.removed) && held.removed.every((id) => typeof id === "string"))) &&
      // Absent from an entry saved before `#222 unread-replies`, which then knows no gap yet.
      (held.replyGaps === undefined || (held.replyGaps !== null && typeof held.replyGaps === "object" &&
        !Array.isArray(held.replyGaps) &&
        Object.values(held.replyGaps).every((gap) => Number.isInteger(gap) && (gap as number) >= 0))) &&
      // Absent from an entry saved before All's page passing over a thread was kept.
      (held.passedOver === undefined ||
        (Array.isArray(held.passedOver) && held.passedOver.every((id) => typeof id === "string"))) &&
      // Absent from an entry saved before what Done settled was kept (`loadCanon` then drops its gaps).
      (held.settledByDone === undefined || (held.settledByDone !== null &&
        typeof held.settledByDone === "object" && !Array.isArray(held.settledByDone) &&
        Object.values(held.settledByDone).every((gap) => Number.isInteger(gap) && (gap as number) >= 0)));
  }
  return held.mode === "page" && Number.isFinite(held.savedAt) && typeof held.more === "boolean";
}

function validCacheShell(shell: unknown): shell is CacheShell {
  return shell !== null && typeof shell === "object" && cachedRowsValid((shell as { channels?: unknown }).channels);
}

/** The device's message cache over `deps.storage`, scoped to `deps.token()` as it stands per use. */
export function createMessageCache(deps: MessageCacheDependencies): MessageCache {
  const storedExactly = (key: string, encoded: string): boolean => {
    try {
      deps.storage.setItem(key, encoded);
      return deps.storage.getItem(key) === encoded;
    } catch (_error) {
      return false;
    }
  };

  const dropMessageCache = (): void => {
    try {
      deps.storage.removeItem(MESSAGE_CACHE_KEY);
    } catch (_error) {
      // Nothing was readable either, so nothing is left to disclose.
    }
  };

  /** Cut a timeline entry's `field` to its newest `limit` and say which views lost rows by it. */
  function trimCanonField(entry: SavedTimeline, field: "messages" | "threads", limit: number): void {
    if (entry[field].length <= limit) return;
    const timeField = field === "threads" ? "updated_at" : "timestamp";
    const removed = entry[field].slice(0, entry[field].length - limit);
    entry[field] = entry[field].slice(-limit) as never;
    const cut = timeOf(entry[field][0], timeField);
    // `#222 unread-replies`. A read reply cut is one more of its thread's the store lacks for being
    // old, so the shortfall against the thread's count is not read as replies posted since. AN UNREAD
    // ONE IS NOT: counted as old it would vanish from its root's count, and the root from Hide read,
    // after a reload. Left out of the gap, it counts as new ("1 new") until a read of the thread or
    // of All brings it back. What Done settled moves with the gap, so putting the root back still
    // finds it (`unsettleReplies`).
    if (field === "messages" && entry.replyGaps) {
      const done = new Set(entry.dismissed.map(String));
      for (const message of removed as Message[]) {
        const id = threadOf(message);
        if (!id || (message.thread as { is_root?: unknown } | null | undefined)?.is_root === true ||
            !Number.isInteger(entry.replyGaps[id]) || deps.stillToDo(message, done)) continue;
        entry.replyGaps[id] = (entry.replyGaps[id] as number) + 1;
        if (entry.settledByDone && Number.isInteger(entry.settledByDone[id])) {
          entry.settledByDone[id] = (entry.settledByDone[id] as number) + 1;
        }
      }
    }
    for (const [key, cover] of Object.entries(entry.views)) {
      const { view, thread } = viewOfKey(key);
      if ((field === "threads") !== (view === "threads")) continue;
      if (field === "messages" &&
          !(removed as Message[]).some((message) => inViewOf(message, view, thread, entry.scope || null))) continue;
      cover.more = true;
      if (Number.isFinite(cut)) cover.floor = cover.floor === null ? cut : Math.max(cover.floor, cut);
    }
  }

  function trimCacheEntry(entry: CacheEntry, limit: number): void {
    if (entry.mode === "timeline") {
      trimCanonField(entry, "messages", limit);
      trimCanonField(entry, "threads", limit);
    } else if (entry.messages.length > limit) {
      entry.messages = entry.messages.slice(-limit);
      entry.more = true;
    }
    const kept = new Set(entry.messages.map((message) => String(message.id)));
    entry.dismissed = entry.dismissed.map(String).filter((id) => kept.has(id));
    if ("unplaced" in entry && entry.unplaced) entry.unplaced = entry.unplaced.filter((id) => kept.has(id));
    if ("removed" in entry && entry.removed) entry.removed = entry.removed.filter((id) => kept.has(id));
    if (entry.mode === "page") {
      entry.cursor = entry.more && entry.messages.length > 0 ? String(entry.messages[0]?.id) : null;
    }
  }

  function readMessageCache(): MessageCacheEnvelope | null {
    const identity = tokenFingerprint(deps.token());
    let raw: string | null = null;
    try {
      raw = deps.storage.getItem(MESSAGE_CACHE_KEY);
    } catch (_error) {
      return null;
    }
    const empty: MessageCacheEnvelope | null = identity ? { v: MESSAGE_CACHE_VERSION, identity, shell: null, scopes: {} } : null;
    if (raw === null) return empty;
    let parsed: unknown = null;
    try {
      parsed = JSON.parse(raw);
    } catch (_error) {
      parsed = null;
    }
    const cache = asRecord(parsed);
    if (!identity || cache === null || cache.v !== MESSAGE_CACHE_VERSION ||
        cache.identity !== identity || cache.scopes === null || typeof cache.scopes !== "object" ||
        Array.isArray(cache.scopes)) {
      dropMessageCache();
      return empty;
    }
    const envelope = parsed as MessageCacheEnvelope;
    for (const [key, entry] of Object.entries(envelope.scopes)) {
      if (!validCacheEntry(entry)) delete envelope.scopes[key];
    }
    if (!validCacheShell(envelope.shell)) envelope.shell = null;
    return envelope;
  }

  function writeMessageCache(cache: MessageCacheEnvelope, keep: string | null = null): boolean {
    const leastRecent = (): string[] => Object.keys(cache.scopes)
      .filter((key) => key !== keep)
      .sort((a, b) => (cache.scopes[a] as CacheEntry).usedAt - (cache.scopes[b] as CacheEntry).usedAt);
    while (Object.keys(cache.scopes).length > MESSAGE_CACHE_SCOPES) {
      delete cache.scopes[leastRecent()[0] as string];
    }
    for (;;) {
      const encoded = JSON.stringify(cache);
      if (encoded.length <= MESSAGE_CACHE_CHARS && storedExactly(MESSAGE_CACHE_KEY, encoded)) return true;
      const oldest = leastRecent()[0];
      const held = keep === null ? undefined : cache.scopes[keep];
      const size = held ? Math.max(held.messages.length, held.threads.length) : 0;
      if (oldest !== undefined) {
        delete cache.scopes[oldest];
      } else if (size > 1 && held) {
        trimCacheEntry(held, Math.floor(size / 2));
      } else {
        dropMessageCache();
        return false;
      }
    }
  }

  return {
    readMessageCache,
    writeMessageCache,
    dropMessageCache,
    storedExactly,
    trimCacheEntry,
    forgetChannelScopes(gone) {
      const cache = readMessageCache();
      if (!cache) return;
      for (const key of Object.keys(cache.scopes)) {
        if (gone(key)) delete cache.scopes[key];
      }
      writeMessageCache(cache);
    },
    saveCacheShell(config) {
      const cache = readMessageCache();
      if (!cache) return;
      const channels = Array.isArray(config.channels) ? config.channels : [];
      cache.shell = {
        channels,
        threading_supported: config.threading_supported === true,
        self_author_id: config.self_author_id || null,
        owner_author_id: config.owner_author_id || null,
        coding_agent_ids: Array.isArray(config.coding_agent_ids) ? config.coding_agent_ids : [],
        coding_agents_named_only: config.coding_agents_named_only === true,
        providers: Array.isArray(config.providers) ? config.providers : [],
      };
      const listed = new Set(channels.map((channel) => String(channel.id)));
      for (const key of Object.keys(cache.scopes)) {
        if (!listed.has(key)) delete cache.scopes[key];
      }
      writeMessageCache(cache);
    },
    saveCacheChannels() {
      const cache = readMessageCache();
      if (!cache || !cache.shell) return;
      cache.shell.channels = deps.channels();
      writeMessageCache(cache);
    },
  };
}

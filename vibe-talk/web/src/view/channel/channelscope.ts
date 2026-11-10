// The channel's saved scope: recording what the channel holds, drawing the saved rows back,
// taking everything a credential read off the screen, and saying a read failed (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they were in
// the script. The store, the cache and the directory are core/channels', made in the script
// and handed in; how old the rows look (the freshness pill) stays in the script for now.

import { MESSAGE_CACHE_ROWS, savedTimelineOf, tokenFingerprint } from "../../core/channels/index.ts";
import type { CacheEntry, ChannelDirectory, ChannelStore, MessageCache, MessageCacheEnvelope, ProviderFacts } from "../../core/channels/index.ts";
import type { Message, ProviderDescription } from "../../contract/index.ts";

/** A channel read failure, as the page annotates it on its way to the reader. */
export interface MutableFailure {
  message?: string; refused?: unknown; status?: unknown; code?: unknown; network?: unknown; shownInPill?: boolean;
}

/** The rows state of the working column, as this owner uses it. */
interface ScopeRows { messages: Message[]; threads: unknown[] }

/** What keeping and clearing a channel's scope asks of the page. */
export interface ChannelScopeDependencies {
  providerFacts: ProviderFacts;
  channelStore: ChannelStore;
  channelDirectory: ChannelDirectory;
  messageCache: MessageCache;
  selectedChannelId(): unknown;
  selection(): { view: string; threadId: unknown; thread: unknown };
  columnWork: { side(): boolean };
  columns(): { main: unknown; side: { mode: string } };
  timelineRows(): ScopeRows;
  doneState(): { archived: Set<string> };
  loadState(): { generation: number; freshAt: number };
  listState(): { newestId: string | null };
  olderPages(): { moreAbove: unknown; cursor: unknown };
  readModes(): { hiding(): boolean };
  bookmarkToSave(channel: string): unknown;
  listedMessages(): Message[];
  plainReach(): { gapFrom(): number | null; restore(gap: number | null): void };
  loadCanon(channel: unknown): void;
  inheritAllCover(view: string, threadId: unknown, thread: unknown): void;
  drawProjection(): boolean;
  knownChannel(id: unknown): unknown;
  channelName(channel: unknown): string;
  restoreSavedBookmark(key: string, bookmark: unknown): void;
  el(id: string): HTMLElement;
  applyNewestPage(payload: unknown, saved: boolean): number;
  olderPager(): { loadedIsWhole(more: unknown): boolean };
  channelSummary(count: number, whole: boolean, name: string): string;
  renderChannelSeam(summary: string): void;
  setChannelFreshness(state: string): void;
  renderOutgoingMessages(): void;
  learnProviders(providers: unknown): void;
  providerOfChannel(id: unknown): ProviderDescription | null;
  fillChannelSelect(id: string): void;
  whoIsWho(): { setDeploymentCodingAgents(shell: unknown): void; setOwnerAuthorId(id: string | null): void; learnOwnerIds(ids: string[] | undefined): void };
  noteSelfAuthor(id: string): void;
  channelSettings(): { fillServiceSelect(): void; renderChannelBox(): void };
  openChannelView(): void;
  restoreChannelComposer(): void;
  inColumn(column: unknown, act: () => void): void;
  readAloudOwner(): { stopReading(): void; readingMode: boolean };
  channelContexts: { clear(): void };
  channelReads(): { payCursor(): void };
  forgetPins(): void;
  readingBookmark(): { forgetDrawn(): void };
  renderOlderControl(): void;
  screenIdentity: { claim(fingerprint: string): void; belongsTo(fingerprint: string): boolean };
  token(): string;
  sideThreads: { clear(): void };
  resetSideColumn(): void;
  forgetBookmarks(): void;
  putDockedReplyAway(): void;
  inboxLedger(): { clear(): void };
  uiState(): { forget(): void };
  readableChannelFailure(error: MutableFailure): string;
  isSlowFailure(error: MutableFailure | null | undefined): boolean;
  channelContextKey(): string;
  threadPresence: { forget(): void };
  voiceCall(): { renderControls(): void };
  renderColumns(): void;
  saveUiState(): void;
}

/** Saving, drawing back and clearing a channel's scope. */
export interface ChannelScope {
  syncCanonDismissed(): void;
  saveChannelScope(): void;
  hydrateChannelScope(): boolean;
  hydrateFromCache(): boolean;
  clearChannelScreen(): void;
  screenBelongsToToken(): boolean;
  forgetMessages(): void;
  noteChannelReadFailure(error: MutableFailure | null | undefined): void;
  reconcileChannelSnapshot(before: { key: string; threading: boolean }): void;
}

/** The channel scope's keeping and clearing over `deps`. */
export function createChannelScope(deps: ChannelScopeDependencies): ChannelScope {
  /** The read state of the rows on screen is the store's read state for those rows. */
  function syncCanonDismissed(): void {
    if (!deps.providerFacts.threading() || deps.channelStore.current().channel !== String(deps.selectedChannelId())) return;
    // `#407 unpack-row-parity`. Over the cards the right column has no rows on screen to speak for.
    if (deps.columnWork.side() && deps.columns().side.mode !== "thread") return;
    for (const message of deps.timelineRows().messages) {
      const id = String(message.id);
      if (deps.doneState().archived.has(id)) deps.channelStore.current().dismissed.add(id);
      else deps.channelStore.current().dismissed.delete(id);
    }
  }






  /**
   * Record what the channel holds. Called after every read that succeeded and after every local
   * change to the rows — a live arrival, an acknowledged send, an archive — so a reload shows what
   * the reader last saw.
   *
   * NOT in the legacy to-do filter: that list is `/todo`, a filtered answer, and saving it as the
   * channel would show a reload a channel with its dealt-with messages missing.
   */
  function saveChannelScope(): void {
    if (!deps.selectedChannelId() || (deps.readModes().hiding() && !deps.providerFacts.threading()) || !screenBelongsToToken()) return;
    const cache = deps.messageCache.readMessageCache();
    if (!cache) return;
    const key = String(deps.selectedChannelId());
    const now = Date.now();
    let entry: CacheEntry | null = null;
    if (deps.providerFacts.threading()) {
      if (deps.channelStore.current().channel !== key) return;
      syncCanonDismissed();
      // The store as the device keeps it (web/src/core/channels `savedTimelineOf`).
      entry = savedTimelineOf(deps.channelStore.current(), now, deps.bookmarkToSave(key));
    } else {
      // In time order, gathered replies or not (`#215 reply-coalesce`): a reload draws these as they
      // are saved, and nothing it draws is gathered.
      const messages = deps.listedMessages();
      const shown = new Set(messages.map((message) => String(message.id)));
      entry = {
        mode: "page",
        savedAt: deps.loadState().freshAt || now,
        usedAt: now,
        messages,
        threads: [],
        more: deps.olderPages().moreAbove !== false,
        cursor: null,
        // `#578 search-status-clarity`: where the rows saved stop being one stretch, if they do.
        gapFrom: deps.plainReach().gapFrom(),
        dismissed: [...deps.doneState().archived].filter((id) => shown.has(id)),
        bookmark: deps.bookmarkToSave(key),
      };
    }
    deps.messageCache.trimCacheEntry(entry, MESSAGE_CACHE_ROWS);
    cache.scopes[key] = entry;
    deps.messageCache.writeMessageCache(cache, key);
  }

  /**
   * Draw the selected channel's saved rows, if this device has them: the store's projection of the
   * current view in timeline mode, the saved page in legacy mode.
   */
  function hydrateChannelScope(): boolean {
    const channel = deps.selectedChannelId();
    if (!channel || (deps.readModes().hiding() && !deps.providerFacts.threading())) return false;
    screenBelongsToToken();
    if (deps.providerFacts.threading()) {
      if (deps.channelStore.current().channel !== String(channel)) deps.loadCanon(channel);
      deps.inheritAllCover(deps.selection().view, deps.selection().threadId, deps.selection().thread);
      return deps.drawProjection();
    }
    const cache = deps.messageCache.readMessageCache();
    const key = String(channel);
    const entry = cache ? cache.scopes[key] : undefined;
    if (!entry || entry.mode !== "page") return false;
    entry.usedAt = Date.now();
    deps.messageCache.writeMessageCache(cache as MessageCacheEnvelope, key);
    deps.restoreSavedBookmark(key, entry.bookmark);
    const payload = {
      channel: deps.knownChannel(channel),
      messages: entry.messages,
      has_more: entry.more,
      next_before: entry.cursor || null,
      dismissed: entry.dismissed,
    };
    try {
      deps.el("discord-log").replaceChildren();
      const loaded = deps.applyNewestPage(payload, true);
      deps.plainReach().restore(entry.gapFrom);
      deps.renderChannelSeam(deps.channelSummary(loaded, deps.olderPager().loadedIsWhole(deps.olderPages().moreAbove), deps.channelName(payload.channel)));
    } catch (_error) {
      clearChannelScreen();
      deps.messageCache.forgetChannelScopes((id) => id === key);
      return false;
    }
    const rows = entry.messages;
    deps.listState().newestId = rows.length > 0 ? String(rows[rows.length - 1]!.id) : null;
    deps.loadState().freshAt = entry.savedAt;
    deps.setChannelFreshness("saved");
    deps.renderOutgoingMessages();
    return true;
  }

  /**
   * Before `/client-config` answers: put the channel picker and the saved rows up from the shell, so
   * a reload opens on the application rather than on the sign-in form and an empty channel.
   */
  function hydrateFromCache(): boolean {
    const cache = deps.messageCache.readMessageCache();
    if (!cache || !cache.shell) return false;
    const shell = cache.shell;
    deps.providerFacts.setThreading(shell.threading_supported === true);
    deps.channelDirectory.setCapability("threading", deps.providerFacts.threading());
    deps.providerFacts.setBroadcast(shell.broadcast_supported === true);
    deps.channelDirectory.setCapability("broadcast", deps.providerFacts.broadcastSupported());
    deps.channelDirectory.setChannels(shell.channels);
    deps.learnProviders(shell.providers);
    deps.whoIsWho().setDeploymentCodingAgents(shell);
    if (shell.self_author_id) deps.noteSelfAuthor(shell.self_author_id);
    deps.whoIsWho().setOwnerAuthorId(shell.owner_author_id);
    deps.whoIsWho().learnOwnerIds(shell.owner_author_ids);
    deps.fillChannelSelect("discord-channel");
    // `#355 provider-grouped-channels`. Settings by service, from the saved shell as from client-config.
    deps.channelSettings().fillServiceSelect();
    deps.fillChannelSelect("settings-channel");
    const provider = deps.providerOfChannel(deps.selectedChannelId());
    if (provider) { deps.providerFacts.setThreading(provider.threading_supported === true); deps.providerFacts.setBroadcast(provider.broadcast_supported === true); }
    deps.channelSettings().renderChannelBox();
    // Before the composer and the rows: both are the view's. `#189 restore-ui-state`.
    deps.openChannelView();
    deps.restoreChannelComposer();
    hydrateChannelScope();
    return true;
  }

  /** Take every channel row off the screen and out of memory. */
  function clearChannelScreen(): void {
    // From the left column, whichever column's read found the credential refused. `#229`.
    if (deps.columnWork.side()) return deps.inColumn(deps.columns().main, clearChannelScreen);
    ++deps.loadState().generation;
    deps.readAloudOwner().stopReading();
    deps.readAloudOwner().readingMode = false;
    deps.channelContexts.clear();
    deps.channelStore.open();
    deps.timelineRows().messages = [];
    deps.timelineRows().threads = [];
    deps.doneState().archived = new Set();
    deps.olderPages().moreAbove = false;
    deps.olderPages().cursor = null;
    deps.channelReads().payCursor();
    deps.listState().newestId = null;
    deps.el("discord-log").replaceChildren();
    deps.el("thread-list").replaceChildren();
    deps.el("channel-summary").replaceChildren();
    deps.el("timeline-notice").hidden = true;
    // The pins this credential read go with the rest of what it read. The bookmarks are kept: they
    // are the server's, read again with the channel, and a write of one still on its way is kept track
    // of for the read that follows (`noteBookmark`). Only another credential forgets them
    // (`forgetBookmarks`). The rows the line was drawn on are gone.
    deps.forgetPins();
    deps.readingBookmark().forgetDrawn();
    deps.el("pinned-log").replaceChildren();
    deps.renderOlderControl();
    deps.loadState().freshAt = 0;
    deps.setChannelFreshness("fresh");
    deps.screenIdentity.claim(tokenFingerprint(deps.token()));
    // `#229 desktop-two-column`. What the right column read goes with the rest, and the threads selected
    // there, which this credential read.
    deps.sideThreads.clear();
    deps.resetSideColumn();
  }

  /**
   * Whether the rows on screen were read for the token saved now. When another tab or the main app
   * has replaced it, they are cleared here — false — so they can be neither saved under the new
   * identity nor merged into its first read.
   */
  function screenBelongsToToken(): boolean {
    if (deps.screenIdentity.belongsTo(tokenFingerprint(deps.token()))) return true;
    clearChannelScreen();
    deps.forgetBookmarks();
    // Reading the envelope deletes it when it, too, belongs to the previous token.
    deps.messageCache.readMessageCache();
    return false;
  }

  /** Sign-out, another token, or a refusal: nothing this credential read may stay behind. */
  function forgetMessages(): void {
    if (deps.columnWork.side()) return deps.inColumn(deps.columns().main, forgetMessages);
    // `#277 reply-in-column`. A reply open in a column answers a message that is going; its words are
    // kept as a draft, as every reply put down is.
    deps.putDockedReplyAway();
    deps.messageCache.dropMessageCache();
    clearChannelScreen();
    deps.forgetBookmarks();
    // `#892 voice-page-optimistic-done`. Nor the Done writes it kept to send again.
    deps.inboxLedger().clear();
    // ...nor where the reader was in it: the record names a thread this credential read, and the
    // next one opens the channel as a first visit does. `#189 restore-ui-state`.
    deps.uiState().forget();
    deps.openChannelView();
    deps.restoreChannelComposer();
  }

  /** A channel read failed. Say so over the rows it leaves standing — or take them away. */
  function noteChannelReadFailure(error: MutableFailure | null | undefined): void {
    // `#195 send-resilience`. Whatever reports this — the banner, for a read the reader asked for —
    // says what it means rather than the server's diagnosis of it.
    if (error) error.message = deps.readableChannelFailure(error);
    if (error && error.refused) {
      forgetMessages();
      return;
    }
    if (error && error.status === 404 && error.code === "unknown_channel") {
      const channel = String(deps.selectedChannelId());
      deps.messageCache.forgetChannelScopes((id) => id === channel);
      clearChannelScreen();
      return;
    }
    // With nothing on screen the error itself is the whole report.
    if (deps.loadState().freshAt) {
      deps.setChannelFreshness(error && error.network ? "offline" : deps.isSlowFailure(error) ? "slow" : "failed");
      // `#195 send-resilience`. Said, over the rows it is about: a refresh nobody asked for needs no
      // banner on top of it. See `refreshQuietly`.
      if (error) error.shownInPill = true;
    }
  }

  /**
   * The page drew a snapshot for one channel and mode; `/client-config` may describe another.
   *
   * `#189 restore-ui-state`. The view is decided again rather than reset to Main, so a channel the
   * saved shell called threadless whose provider has threads after all opens in All — here, once —
   * unless the reader chose a view. Nothing is read here: with the channel on screen, what asked for
   * sign-in reads the view now up next — a reopen's owed read, or the poll of a page that opened
   * offline — and a read of its own would be a second one.
   */
  function reconcileChannelSnapshot(before: { key: string; threading: boolean }): void {
    if (before.key === deps.channelContextKey() && before.threading === deps.providerFacts.threading()) return;
    clearChannelScreen();
    deps.openChannelView();
    deps.threadPresence.forget();
    hydrateChannelScope();
    deps.restoreChannelComposer();
    deps.voiceCall().renderControls();
    // Threads or none, as sign-in says: one column or two decided again. `#229 desktop-two-column`.
    deps.renderColumns();
    deps.saveUiState();
  }

  return {
    syncCanonDismissed, saveChannelScope, hydrateChannelScope, hydrateFromCache,
    clearChannelScreen, screenBelongsToToken, forgetMessages, noteChannelReadFailure,
    reconcileChannelSnapshot,
  };
}

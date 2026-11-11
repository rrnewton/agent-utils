// Which view a channel opens in, and where the reader was, across a reload: the kept view per
// channel, the saved place from the record, and putting the reader back on their message
// (`#624 voice-page-modularity`, moved from the legacy script). The record itself is
// core/navigation's (`createUiState`), made in the script and handed in. Words, order and the
// place a reopen lands on are as they were in the script.

import { threadOf } from "../../core/channels/index.ts";
import type { ChannelCanon } from "../../core/channels/index.ts";
import { is } from "../../contract/index.ts";
import type { ThreadSummary } from "../../contract/index.ts";

/** The saved record of what was on screen, as this owner uses it (core/navigation `UiState`). */
export interface OpeningRecord {
  chosenView(channel: string): { channelView: string; thread: { id: unknown; origin: string; summary: ThreadSummary | null } | null } | undefined;
  forgetChosen(keep: (id: string) => boolean): void;
  chosenViewsHeld(): { channel: string }[];
  setChosen(channels: { channel: string }[]): void;
  remember(shown: unknown): void;
  heldPlace(): { key: string; place: { view: string; atNewest: boolean; anchor: string | null; offset: number } } | null;
  holdPlace(held: { key: string; place: { view: string; atNewest: boolean; anchor: string | null; offset: number } }): void;
  settled(): boolean;
  settle(): void;
  saved(): { channel: string; view: string; place: { view: string; atNewest: boolean; anchor: string | null; offset: number } | null } | null;
}

/** What opening a channel's view asks of the page. */
export interface OpeningViewDependencies {
  uiState(): OpeningRecord;
  providerFacts: { threading(): boolean };
  reopenedViews: readonly string[];
  uiStateKey: string;
  uiStateVersion: number;
  uiStateChars: number;
  channelStore(): { current(): ChannelCanon };
  threadPresence(): { forget(): void };
  captureScroll(): { pinned: boolean; anchor: Element | null; top: number };
  channelContextKey(): string;
  deepLinkFromAddress(): { view: string | null; channel?: string; thread?: string; message?: { channel?: string; thread?: string } } | null;
  deepLinkReaderMoved(): boolean;
  el(id: string): HTMLElement;
  idsOf(row: unknown): string[];
  inColumn(column: unknown, act: () => void): void;
  listPlaceTop(area: HTMLElement): number;
  loadCanon(channel: string): void;
  renderColumns(): void;
  restoreChannelComposer(): void;
  restoreScroll(mark: { pinned: boolean; anchor: Element; top: number }): void;
  scheduleBackSync(): void;
  selectedChannelId(): unknown;
  selection(): { view: string; thread: ThreadSummary | null; threadId: unknown; origin: string };
  token(): string;
  tokenFingerprint(token: string): string;
  upsertThreadSummary(canon: ChannelCanon, summary: ThreadSummary): void;
  viewKey(view?: string, threadId?: unknown): string;
  visibleList(): { children: Iterable<Element> };
  windowStorage(): { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void };
  deepLinks(): { applied(): boolean };
  sideColumnUp(): boolean;
  columns(): { main: unknown };
  readModes(): { hiding(): boolean; mode(): string };
  screens(): { screen(): string; view(): string };
  documentsQueryValue(name: string): string | null;
  documentsDeepLinkKey(): string;
  showView(name: string): void;
  showDocuments(key: string): void;
  reopenReadOwed(): { owe(after: unknown): void; take(): { after: unknown } | null; keep(owed: { after: unknown }): void };
  replayStarted(): unknown;
  readEnteredChannel(fresh: boolean): void;
  freshAt(): number;
  whenLiveAttached(pay: () => void): void;
  columnWork(): { side(): boolean };
  messageCache(): { storedExactly(key: string, value: string): void };
}

/** Which view a channel opens in, and where the reader was. */
export interface OpeningView {
  windowItem(key: string): string | null;
  keepWindowItem(key: string, value: string | null): void;
  defaultChannelView(): string;
  threadStillKnown(id: unknown): boolean;
  openingChannelView(): { view: string; thread: string | null; summary: ThreadSummary | null; origin: string };
  openChannelView(): void;
  holdChannelPlace(): void;
  savedThreadSummary(): ThreadSummary | null;
  viewOnScreen(): { channel: string; channelView: string; thread: { id: string; origin: string; summary: ThreadSummary | null } | null } | null;
  saveUiState(): void;
  returnToSavedPlace(): void;
  reopenSavedView(): void;
  payReopenRead(): void;
}

/** Which view a channel opens in, over `deps`. */
export function createOpeningView(deps: OpeningViewDependencies): OpeningView {
  // --- which view a channel opens in, and where the reader was, across a reload ----------------------
  //
  // `#189 restore-ui-state`. The owner, 2026-10-04: "Let's default to All view when opening the app
  // channel view. But let's save prior UI state if possible. When I kill and reopen my pwa I want to
  // see mostly the same thing." A phone reclaims an installed app whenever it likes, so a reopen is an
  // ordinary event, and it used to open on the call view, in Main, at the newest message, with Hide
  // read off, whatever the reader had been doing. The page now keeps one small record of what was on
  // screen and reopens on it: the call or the channel; which part of the channel — Main, All or a
  // thread, with the thread's summary so its heading has a name before anything is read; whether
  // read messages were hidden; and the message the reader's eye was on.
  //
  // EACH CHANNEL KEEPS ITS OWN VIEW. `#205 channel-view-memory`. The owner, 2026-10-04 16:18: "All
  // should be the default but if changed the selection should be saved for that channel. Ideally it
  // should survive restarts." A choice used to last only until the channel changed: Main picked in
  // one channel, a look at another, and the first was in All again. The record now keeps the view the
  // reader chose in each channel — Main, All, or a thread with its summary — for the channels most
  // recently on screen, and changing to one of them, or reopening the app on it, opens it there. A
  // channel nobody chose a view in opens in its default. Picking All is kept as a choice too, though
  // it is the default today: what the reader picked is what the channel opens in, whatever the
  // default later becomes.
  //
  // NOT THE SCREEN. A reopen lands on the main screen whatever was up: Settings and Help are a tap
  // away, and a reply keeps its draft without its screen
  // being reopened (`DRAFTS_KEY`, web/src/core/outbox). The selected channel already has its own key (`ACTIVE_CHANNEL_KEY`);
  // the record names it only so a place measured in one channel is never applied to another.
  //
  // A PLACE IS A MESSAGE, not a pixel offset — `captureScroll`'s anchor, put back by `restoreScroll`.
  // The rows above it are redrawn from the snapshot and then re-read, and an offset would land on
  // whatever happens to be there by then. A reader who was on the newest line stays on it, and one
  // whose message is gone lands on the newest, as a first visit does.
  //
  // CHECKED ON THE WAY BACK, because storage is a boundary like the network: a channel no longer
  // listed, a thread the channel's store no longer knows, or a provider that no longer has threads
  // falls back to the default, a damaged choice is dropped on its own, and a record that does not
  // parse is deleted. It is scoped to the token as the snapshot is, and forgotten with it: a thread's
  // summary is something that credential read. A channel the server stops listing loses its choice
  // when it loses its saved rows.
  //
  // EACH WINDOW REOPENS ON ITS OWN. `#265 window-channel-memory`. The owner, 2026-10-13, with two
  // windows of the installed app on a desk, one left on each of two channels: "refresh will not
  // respect those selections." The channel (`ACTIVE_CHANNEL_KEY`) and this record were kept only in
  // `localStorage`, which every window of the origin shares, so a reload of either window came back
  // on whatever the window that wrote last had shown. Both are now kept in the window's own
  // `sessionStorage` as well, which outlives a reload of that window and no other, and a reload reads
  // the window's own first: its channel, the call or the channel, the view or thread it had open,
  // and the message it was on. The shared copy is still written, and is what a brand-new window opens
  // on — the channel chosen last, in any window. The view chosen in a channel this window has not
  // chosen one in is taken from the shared record, behind the window's own choices. The read mode
  // is a preference of the device rather than a place, so it comes from the shared record whichever
  // window wrote that. What the right column shows in two columns is kept for the session only
  // (`sideThreads`), in memory, which is already the window's own. `#281 view-keys`: and the layout —
  // one column, the thread list on its own, or two columns — is the window's own as well (`COLUMNS_KEY`,
  // web/src/core/navigation/layout.ts), so each of the four views Ctrl+1 to Ctrl+4 choose is what a reload
  // of that window comes back to.
  //
  // The window's storage, not its address: a channel in the URL would outlive a reload too, but it
  // would ride along in every link copied out of the address bar, and every route, the installed
  // app's start_url and the offline shell would have to keep accepting it. A window's own storage
  // is invisible to all of them.

  /**
   * This window's own copy of `key`, or null when it has none — or cannot keep one, where storage is
   * refused, and every window then reads the device's, as every window did before.
   */
  function windowItem(key: string) {
    try {
      return deps.windowStorage().getItem(key);
    } catch (_error) {
      return null;
    }
  }

  /** Keep `value` as this window's own `key`, or forget it for null. Nothing, where storage is refused. */
  function keepWindowItem(key: string, value: string | null) {
    try {
      if (value === null) deps.windowStorage().removeItem(key);
      else deps.windowStorage().setItem(key, value);
    } catch (_error) {
      // The device's copy is still written, and a reload of this window falls back on it.
    }
  }

  /**
   * The view a channel opens in until the reader picks one: All where its provider has threads — every
   * message, each tagged with its thread — and Main where it has none, because there Main IS the
   * whole channel and All is not offered. The owner, again at 09:31: "My channel view seems to still
   * default to Main. I want All."
   */
  function defaultChannelView() {
    return deps.providerFacts.threading() ? "flat" : "main";
  }


  /**
   * Whether the selected channel's store still knows thread `id`: its summary, a message in it, or a
   * page read for it. A thread the store has never heard of is not one to reopen into.
   */
  function threadStillKnown(id: unknown) {
    const channel = String(deps.selectedChannelId());
    if (deps.channelStore().current().channel !== channel) deps.loadCanon(channel);
    if (id === deps.channelStore().current().scope) return false;
    return deps.channelStore().current().views.has(deps.viewKey("thread", id)) ||
      deps.channelStore().current().threads.some((summary) => String(summary.id) === id) ||
      deps.channelStore().current().messages.some((message) => threadOf(message) === id);
  }

  /**
   * The view to open the selected channel in: the one the reader chose there while it holds, and
   * otherwise the default. A choice holds on a provider that still has threads — Main included, since
   * preferring Main to All is the owner's call to make — and a thread only while the channel's store
   * still knows it. A thread the store has lost is forgotten as it is passed over, so the channel
   * opens in its default from then on, not back in the thread on some later visit whose read happens
   * to mention it. A provider without threads only passes a choice over: the saved shell may say so
   * of a provider `/client-config` then says has them.
   */
  function openingChannelView() {
    const channel = String(deps.selectedChannelId());
    // `#462 webapp-deep-links`. A link's one-column view is decided HERE, with every other
    // opening decision, not put on later over the reopened one: an async apply racing the
    // reopen flow yanks a reader who has already settled. Layout views and the message
    // landing still apply after (`applyDeepLink`), and only until the link is applied.
    if (!deps.deepLinks().applied() && !deps.deepLinkReaderMoved()) {
      const wanted = deps.deepLinkFromAddress();
      const wantedChannel = wanted && ((wanted.message && wanted.message.channel) || wanted.channel);
      if (wanted && (!wantedChannel || wantedChannel === channel)) {
        const thread = wanted.thread || (wanted.message && wanted.message.thread) || null;
        if (wanted.view === "main") return { view: "main", thread: null, summary: null, origin: "main" };
        if (wanted.view === "all") return { view: "flat", thread: null, summary: null, origin: "main" };
        if ((wanted.view === "thread" || (!wanted.view && thread)) && thread && deps.providerFacts.threading()) {
          // The very thread this channel already opens in — a reload standing on its own
          // address: the kept choice below says where its Back goes (the thread list it was
          // opened from, say), and a link naming the same thread must not rewrite that.
          const kept = deps.uiState().chosenView(channel);
          if (!(kept && kept.channelView === "thread" && kept.thread && String(kept.thread.id) === thread)) {
            const summary = deps.channelStore().current().threads.find((held) => String(held.id) === thread) || null;
            return { view: "thread", thread, summary: summary || null, origin: "main" };
          }
        }
      }
    }
    const chosen = deps.providerFacts.threading() ? deps.uiState().chosenView(channel) : undefined;
    const fallback = defaultChannelView();
    if (chosen && chosen.channelView === "thread" && chosen.thread && !threadStillKnown(chosen.thread.id)) {
      deps.uiState().forgetChosen((id) => id === channel);
    } else if (chosen) {
      const thread = chosen.thread;
      return {
        view: chosen.channelView,
        thread: thread ? String(thread.id) : null, summary: thread ? thread.summary : null,
        origin: thread ? thread.origin : fallback,
      };
    }
    return { view: fallback, thread: null, summary: null, origin: fallback };
  }

  /** Put the selected channel on the view it opens in. Draws nothing; every caller draws next. */
  function openChannelView() {
    // `#229 desktop-two-column`. In two columns the left column is Main, whatever the channel opens in.
    // `#281 view-keys`: and behind the thread list on its own.
    const opening = deps.sideColumnUp() ? { view: "main", thread: null, summary: null, origin: "main" } : openingChannelView();
    deps.selection().view = opening.view;
    deps.selection().threadId = opening.thread;
    deps.selection().thread = opening.summary;
    deps.selection().origin = opening.origin;
    // A reopened thread is named from the store, like every thread, so the summary kept with the
    // record goes into it when the store has none: the heading has its name before any read.
    const summary = opening.summary;
    if (summary && !deps.channelStore().current().threads.some((held) => String(held.id) === String(summary.id))) {
      deps.upsertThreadSummary(deps.channelStore().current(), summary);
    }
    // `#304 back-gesture-in-app`. A thread reopened has an entry to go back through, as one entered has.
    deps.scheduleBackSync();
  }

  /** The channel and view a measured place belongs to. */
  const placeKey = () => JSON.stringify([String(deps.selectedChannelId()), deps.viewKey()]);

  /**
   * Note which message the reader's eye is on in the channel list, while there is one on screen to
   * measure: before another view, another screen or the page going away hides it, since a hidden
   * list has no boxes left to measure.
   */
  function holdChannelPlace() {
    // A reopen comes back to one list: the left column's. `#229 desktop-two-column`.
    if (deps.columnWork().side()) return;
    if (deps.screens().screen() !== "main" || deps.screens().view() !== "discord" || !deps.reopenedViews.includes(deps.selection().view)) return;
    const mark = deps.captureScroll();
    const anchor = mark.anchor;
    deps.uiState().holdPlace({
      key: placeKey(),
      place: {
        view: deps.viewKey(),
        atNewest: mark.pinned,
        anchor: anchor ? anchor.getAttribute("data-context-id") || deps.idsOf(anchor)[0] || null : null,
        offset: anchor ? mark.top - deps.listPlaceTop(deps.el("scroll-area")) : 0,
      },
    });
  }

  /** The open thread's summary as the store holds it, for the heading a reopen draws first. */
  function savedThreadSummary() {
    const id = String(deps.selection().threadId);
    const summary = deps.channelStore().current().threads.find((held) => String(held.id) === id) || deps.selection().thread;
    return summary && is("ThreadSummary", summary) ? summary : null;
  }

  /** The view on screen as its channel's choice, or null for a view no channel is reopened in. */
  function viewOnScreen() {
    const channel = String(deps.selectedChannelId());
    if (!channel || !deps.reopenedViews.includes(deps.selection().view) || (deps.selection().view === "thread" && !deps.selection().threadId)) return null;
    const thread = deps.selection().view === "thread" ? {
      id: String(deps.selection().threadId),
      origin: ["main", "flat", "list"].includes(deps.selection().origin) ? deps.selection().origin : defaultChannelView(),
      summary: savedThreadSummary(),
    } : null;
    return { channel, channelView: deps.selection().view, thread };
  }

  /**
   * Record what is on screen now. One small key, so it is written whenever the reader changes what
   * they are looking at; the place is the one last measured, if it is a place in this view.
   */
  function saveUiState() {
    // What is on screen is recorded from the left column, whichever column asked. `#229`.
    if (deps.columnWork().side()) return deps.inColumn(deps.columns().main, saveUiState);
    const identity = deps.tokenFingerprint(deps.token());
    if (!deps.uiState().settled() || !identity) return;
    const channel = String(deps.selectedChannelId());
    const shown = viewOnScreen();
    const chosen = deps.uiState().chosenView(channel);
    // The channel's choice, on screen: it is now the last to be given up, and an open thread's
    // summary is taken again from the store, which may have named the thread since it was chosen.
    if (shown && chosen && shown.channelView === chosen.channelView &&
        (shown.thread && shown.thread.id) === (chosen.thread && chosen.thread.id)) {
      deps.uiState().remember(shown);
    }
    const held = deps.uiState().heldPlace();
    const record = {
      v: deps.uiStateVersion,
      identity,
      view: deps.screens().view() === "discord" ? "discord" : "voice",
      // Hide read, as every page has kept it, and beside it the read mode it is one of (`#221
      // read-modes`): a page from before the modes reads `todo` and ignores `read`.
      todo: deps.readModes().hiding(),
      read: deps.readModes().mode(),
      channel,
      place: held && held.key === placeKey() ? held.place : null,
      channels: deps.uiState().chosenViewsHeld(),
    };
    // Inside its bound by giving up the choices of the channels least recently on screen — never the
    // selected channel's, which is one view and at most one thread's summary.
    let encoded = JSON.stringify(record);
    for (let last = record.channels.length - 1; encoded.length > deps.uiStateChars && last >= 0; last -= 1) {
      if (record.channels[last]!.channel === channel) continue;
      record.channels = record.channels.filter((_entry, index) => index !== last);
      encoded = JSON.stringify(record);
    }
    deps.uiState().setChosen(record.channels);
    // The device's, for the next brand-new window; and this window's, for a reload of it. `#265`.
    deps.messageCache().storedExactly(deps.uiStateKey, encoded);
    keepWindowItem(deps.uiStateKey, encoded);
  }

  /**
   * Put the reader back on the message the record says they were reading, at the same height in the
   * window, when it is drawn in the view they left. Otherwise they stay on the newest message, where
   * entering the view put them.
   */
  function returnToSavedPlace() {
    const saved = deps.uiState().saved();
    const place = saved ? saved.place : null;
    if (!saved || !place || place.atNewest || !place.anchor || saved.channel !== String(deps.selectedChannelId()) ||
        place.view !== deps.viewKey()) return;
    const row = [...deps.visibleList().children].find((candidate) =>
      candidate.getAttribute("data-context-id") === place.anchor || deps.idsOf(candidate).includes(place.anchor!));
    if (!row) return;
    deps.restoreScroll({ pinned: false, anchor: row, top: deps.listPlaceTop(deps.el("scroll-area")) + place.offset });
  }

  /**
   * Reopen on the channel when that is where the reader left the page — once, as soon as there is a
   * channel to show: at load from the saved shell, or after sign-in when there was none. Its view was
   * decided already, by `openChannelView`; this enters it and puts the reader back on their message.
   * The view's one newest page is owed from here, and `payReopenRead` reads it.
   */
  function reopenSavedView(): void {
    // `?v=docs` (with `&d=KEY` for one document) opens Documents (`documents-view-client`).
    // Before the settled guard: an explicit address outranks a saved place.
    if (deps.documentsQueryValue("v") === "docs") {
      deps.uiState().settle();
      deps.showView("docs");
      deps.showDocuments(deps.documentsDeepLinkKey());
      saveUiState();
      return;
    }
    if (deps.uiState().settled()) return;
    deps.uiState().settle();
    const saved = deps.uiState().saved();
    if (saved && saved.view === "discord" && deps.selectedChannelId() && deps.screens().view() !== "discord") {
      deps.showView("discord");
      returnToSavedPlace();
      holdChannelPlace();
      deps.reopenReadOwed().owe(deps.replayStarted());
    }
    saveUiState();
  }

  /**
   * Read what the reopen put on screen, once sign-in has answered: at once on a channel without
   * threads, whose replay is not accounted for by read, and on one with threads when the live stream
   * has attached. A refusal reads nothing, and leaves it owed to the token that replaces this one.
   */
  function payReopenRead(): void {
    const owed = deps.reopenReadOwed().take();
    if (!owed) return;
    const pay = () => {
      // Signed out while it waited: owed to the next sign-in, as after a refusal.
      if (!deps.token()) {
        deps.reopenReadOwed().keep(owed);
        return;
      }
      if (deps.screens().view() !== "discord" || deps.replayStarted() !== owed.after) return;
      // Merged into the rows the snapshot drew, keeping the reader's message, as a channel change
      // does; with nothing drawn it is a first visit and settles on the newest.
      deps.readEnteredChannel(deps.freshAt() > 0);
    };
    if (deps.providerFacts.threading()) deps.whenLiveAttached(pay);
    else pay();
  }

  return {
    reopenSavedView, payReopenRead,
    windowItem, keepWindowItem, defaultChannelView, threadStillKnown, openingChannelView,
    openChannelView, holdChannelPlace, savedThreadSummary, viewOnScreen, saveUiState, returnToSavedPlace,
  };
}

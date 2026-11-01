// Browsing channels (`#19 channel-browser`, `#371 channel-search-hash`; `#624 voice-page-modularity`,
// moved from the legacy script; plan step 16, view/settings).
//
// Pasting a link works, but only for someone who already has the link. The bridge can list the
// channels its account sees, so Settings offers that list, searchable, with one tap to add. Adding goes
// through exactly the same `POST /api/v1/channels` as the add form, so a browsed channel is registered,
// probed and stored the same way a pasted one is. What it asks for and says is
// web/src/present/settings's; the channel list it adds to, and the box around it, are the page's.

import {
  DIRECTORY_SEARCH_DELAY_MS,
  DIRECTORY_TICK_MS,
  directoryAskingWords,
  directoryEntryTracked,
  directoryFailureIsFinal,
  directoryFailureSentence,
  directoryLabel,
  directoryPath,
  directorySummary,
  directoryWaitWords,
} from "../../present/settings/index.ts";
import type { DirectoryEntry, DirectoryFailure } from "../../present/settings/index.ts";

/** A page of the directory as the server answers it; anything else reads as no entries. */
type DirectoryPage = { readonly entries?: unknown; readonly next_cursor?: unknown; readonly truncated?: unknown } | null;

/** What an add answers: the channel list, and the channel added. */
type AddedChannel = { readonly channel?: { readonly id?: unknown } | null } & Record<string, unknown>;

/** What the browser asks of the page. */
export interface ChannelDirectoryDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** A request to the page's server (`api`). */
  request(path: string, options?: { method: string; body?: unknown }): Promise<unknown>;
  /** Whether the channel with this id is on the list (`knownChannel`). */
  known(id: string): boolean;
  /** Open one of the channel box's panels, or none (`showChannelPanel`). */
  showPanel(which: string | null): void;
  /** What the service Settings is showing is called (`settingsServiceName`). */
  serviceName(): string;
  /** The service Settings is showing, with several to choose from; null with one (`settingsService`). */
  provider(): string | null;
  /** A click handler that reports a failure rather than throwing it (`guardQuietly`). */
  guard(run: () => Promise<void>): () => void;
  /** Take the server's channel list as the truth (`adoptChannelList`). */
  adoptChannelList(payload: unknown): void;
  /** Point Settings at the channel with this id (`pointSettingsAt`). */
  pointSettingsAt(id: string): void;
  /** Redraw the channel box (`renderChannelBox`). */
  renderChannelBox(): void;
  /** What the page calls a channel (`channelName`). */
  channelName(channel: unknown): string;
  /** The page's clock and timers. */
  now(): number;
  setTimeout(run: () => void, ms: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
}

/** The browser's named commands. */
export interface ChannelDirectory {
  /** Open the browse panel, and read the first page unless it is already showing one. */
  open(): Promise<void>;
  /** Search as the owner types, once the typing pauses. */
  scheduleSearch(): void;
  /** Search now, for the text in the box, unless that is already the search on screen. */
  search(): Promise<void>;
  /** Read a page of the directory: the first page of the current search, or the next one. */
  load(more: boolean): Promise<void>;
  /** Retry what failed: the next page when rows are already showing, otherwise the first. */
  retry(): Promise<void>;
  /** Draw the rows. */
  render(): void;
  /** Empty the browse list, and drop any answer still on its way: it was for the service that was showing. */
  reset(): void;
  /**
   * `#371 channel-search-hash`. Give up on a read still on its way, as the list closes: its count stops,
   * and its answer is not drawn into a list nobody is looking at.
   */
  giveUp(): void;
  /** The channel with this id is no longer on the list: its entry can be added again. */
  forget(id: string): void;
}

/** Nothing read. Made once, where the page wires its Settings. */
export function mountChannelDirectory(deps: ChannelDirectoryDependencies): ChannelDirectory {
  /**
   * The browse panel's state. `#19 channel-browser`.
   *
   * `generation` is what keeps a slow answer from overwriting a newer one: every fresh search takes
   * a new number, and an answer for an older number is dropped when it arrives. Typing "rel", then
   * "release", must end showing the "release" results however the two requests race.
   */
  const channelDirectory: {
    query: string; entries: DirectoryEntry[]; nextCursor: string | null; truncated: boolean; loading: boolean;
    failed: boolean; generation: number; searchTimer: ReturnType<typeof setTimeout> | null;
    ticker: ReturnType<typeof setTimeout> | null; adding: Set<string>;
  } = {
    query: "",
    entries: [],
    nextCursor: null,
    truncated: false,
    loading: false,
    failed: false,
    generation: 0,
    searchTimer: null,
    /** `#371 channel-search-hash`. The timer that says how long the read on its way has taken. */
    ticker: null,
    adding: new Set(),
  };

  const tracked = (entry: DirectoryEntry): boolean => directoryEntryTracked(entry, deps.known);

  /** Stop saying how long the directory read has taken: it answered. */
  function stopTicker(): void {
    if (channelDirectory.ticker !== null) deps.clearTimeout(channelDirectory.ticker);
    channelDirectory.ticker = null;
  }

  /** Every string is text; nothing from the bridge is ever parsed as markup. */
  function render(): void {
    const rows = channelDirectory.entries.map((entry) => {
      const row = document.createElement("li");
      const name = document.createElement("span");
      name.className = "directory-name";
      name.textContent = entry.name;
      row.append(name);
      if (tracked(entry)) {
        const already = document.createElement("span");
        already.className = "directory-tracked";
        already.textContent = "Already added";
        row.append(already);
      } else {
        const add = document.createElement("button");
        add.type = "button";
        add.className = "secondary";
        const adding = channelDirectory.adding.has(entry.source);
        add.textContent = adding ? "Adding…" : "Add";
        add.disabled = adding;
        add.setAttribute("aria-label", `Add ${entry.name}`);
        add.addEventListener("click", deps.guard(() => addEntry(entry)));
        row.append(add);
      }
      return row;
    });
    deps.lookup("channel-directory-list").replaceChildren(...rows);
    deps.lookup("channel-directory-more").hidden = channelDirectory.nextCursor === null;
    (deps.lookup("channel-directory-more") as HTMLButtonElement).disabled = channelDirectory.loading;
    if (!channelDirectory.failed) deps.lookup("channel-directory-retry").hidden = true;
  }

  /**
   * A failure while showing more keeps the rows already on screen. They are still true, and
   * throwing away a list the owner was scrolling through because page three failed would be worse
   * than saying page three failed.
   */
  async function load(more: boolean): Promise<void> {
    const generation = more ? channelDirectory.generation : ++channelDirectory.generation;
    const cursor = more ? channelDirectory.nextCursor : null;
    if (more && !cursor) return;
    channelDirectory.loading = true;
    channelDirectory.failed = false;
    if (!more) {
      channelDirectory.entries = [];
      channelDirectory.nextCursor = null;
      channelDirectory.truncated = false;
    }
    render();
    const said = deps.lookup("channel-directory-state");
    const asking = directoryAskingWords(more, channelDirectory.query);
    said.textContent = asking;
    // `#371 channel-search-hash`. A service may take seconds to list its channels (Slack lists every
    // conversation the app is in before it can answer one search), so the line says how long this read
    // has taken, and once it is slow, why the next will not be. The server keeps the listing for two
    // minutes, so the searches after it answer at once.
    if (channelDirectory.ticker !== null) deps.clearTimeout(channelDirectory.ticker);
    const startedAt = deps.now();
    const tick = (): void => {
      channelDirectory.ticker = null;
      if (generation !== channelDirectory.generation || !channelDirectory.loading) return;
      said.textContent = directoryWaitWords(asking, deps.now() - startedAt, deps.serviceName());
      channelDirectory.ticker = deps.setTimeout(tick, DIRECTORY_TICK_MS);
    };
    channelDirectory.ticker = deps.setTimeout(tick, DIRECTORY_TICK_MS);
    const path = directoryPath(channelDirectory.query, cursor, deps.provider());
    let payload: DirectoryPage = null;
    try {
      payload = await deps.request(path) as DirectoryPage;
    } catch (error) {
      if (generation !== channelDirectory.generation) return;
      stopTicker();
      channelDirectory.loading = false;
      channelDirectory.failed = true;
      render();
      said.textContent = directoryFailureSentence(error as DirectoryFailure, deps.serviceName());
      // Retrying cannot change a refusal or a missing feature; it can change everything else.
      deps.lookup("channel-directory-retry").hidden = directoryFailureIsFinal(error as DirectoryFailure);
      return;
    }
    if (generation !== channelDirectory.generation) return;
    stopTicker();
    channelDirectory.loading = false;
    const entries = Array.isArray(payload && payload.entries) ? (payload as { entries: DirectoryEntry[] }).entries : [];
    channelDirectory.entries = more ? [...channelDirectory.entries, ...entries] : entries;
    channelDirectory.nextCursor =
      payload && typeof payload.next_cursor === "string" && payload.next_cursor
        ? payload.next_cursor
        : null;
    channelDirectory.truncated = payload !== null && payload.truncated === true;
    render();
    said.textContent = directorySummary(channelDirectory, deps.serviceName());
  }

  /** Add one browsed channel through the ordinary add route, and mark it added. */
  async function addEntry(entry: DirectoryEntry): Promise<void> {
    if (tracked(entry) || channelDirectory.adding.has(entry.source)) return;
    const said = deps.lookup("channel-directory-state");
    const label = directoryLabel(entry.name);
    channelDirectory.adding.add(entry.source);
    render();
    said.textContent = `Adding "${entry.name}" and checking that it can be read…`;
    let payload: AddedChannel | null = null;
    try {
      const body: Record<string, unknown> = { source: entry.source, label };
      // `#355 provider-grouped-channels`. Added to the service it was listed for.
      const provider = deps.provider();
      if (provider !== null) body.provider = provider;
      payload = await deps.request("/api/v1/channels", {
        method: "POST",
        body,
      }) as AddedChannel;
    } catch (error) {
      channelDirectory.adding.delete(entry.source);
      // A conflict means the source is already a channel here, which is the tracked state.
      if ((error as DirectoryFailure).status === 409) entry.tracked = true;
      render();
      said.textContent =
        (error as DirectoryFailure).status === 409 ? `"${entry.name}" is already in the channel list.` : (error as DirectoryFailure).message as string;
      return;
    }
    channelDirectory.adding.delete(entry.source);
    deps.adoptChannelList(payload);
    entry.tracked = true;
    entry.channel_id = payload.channel && payload.channel.id ? String(payload.channel.id) : null;
    if (entry.channel_id) deps.pointSettingsAt(entry.channel_id);
    // The box onto the channel just added, as the add form does: with one chat service `pointSettingsAt`
    // only sets the picker, which fires nothing, and who is who names its channel (`#402
    // who-is-who-channel-box`), so a box left drawn for the last channel says so in words.
    deps.renderChannelBox();
    render();
    // Named from the answer, for the reason the add form gives.
    const added = deps.channelName(payload.channel || { label });
    said.textContent = `Added. "${added}" is now in the channel picker.`;
  }

  async function search(): Promise<void> {
    if (channelDirectory.searchTimer !== null) {
      deps.clearTimeout(channelDirectory.searchTimer);
      channelDirectory.searchTimer = null;
    }
    const query = (deps.lookup("channel-directory-search") as HTMLInputElement).value.trim();
    const showing = !channelDirectory.failed && channelDirectory.entries.length > 0;
    if (query === channelDirectory.query && showing) return;
    channelDirectory.query = query;
    await load(false);
  }

  return {
    async open() {
      deps.showPanel("browse-channel-fields");
      deps.lookup("channel-directory-search").focus();
      if (channelDirectory.entries.length === 0 || channelDirectory.failed) {
        await load(false);
      } else {
        render();
      }
    },
    scheduleSearch() {
      if (channelDirectory.searchTimer !== null) deps.clearTimeout(channelDirectory.searchTimer);
      channelDirectory.searchTimer = deps.setTimeout(() => {
        channelDirectory.searchTimer = null;
        search();
      }, DIRECTORY_SEARCH_DELAY_MS);
    },
    search,
    load,
    retry: () => load(channelDirectory.entries.length > 0 && channelDirectory.nextCursor !== null),
    render,
    reset() {
      channelDirectory.generation += 1;
      if (channelDirectory.searchTimer !== null) {
        deps.clearTimeout(channelDirectory.searchTimer);
        channelDirectory.searchTimer = null;
      }
      channelDirectory.query = "";
      channelDirectory.entries = [];
      channelDirectory.nextCursor = null;
      channelDirectory.truncated = false;
      channelDirectory.loading = false;
      channelDirectory.failed = false;
      (deps.lookup("channel-directory-search") as HTMLInputElement).value = "";
      deps.lookup("channel-directory-state").textContent = "";
      render();
    },
    giveUp() {
      if (!channelDirectory.loading) return;
      stopTicker();
      channelDirectory.generation += 1;
      channelDirectory.loading = false;
    },
    forget(id) {
      // The directory is cached while Settings stays open. Put a removed source back into its real
      // state immediately; otherwise reopening Browse in the same page still said "Already added"
      // and offered no way to add it again.
      for (const entry of channelDirectory.entries) {
        if (String(entry.channel_id || "") === String(id)) {
          entry.tracked = false;
          entry.channel_id = null;
        }
      }
    },
  };
}

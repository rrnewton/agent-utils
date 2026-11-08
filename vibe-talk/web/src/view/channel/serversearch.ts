// The server's copy, searched (`#605`, `#607`, `#608 server-side-search`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): which search is on
// screen and its request, gathering older history on request, and searching the threads'
// replies further back over the cards. The search state itself is core/search's, instantiated
// here; drawing what came back is olderwalks'.

import type { Message, SearchScope } from "../../contract/index.ts";
import { repliesWalkWords } from "../../present/notices/index.ts";
import { rowShown } from "./rows.ts";

/** What searching the server's copy asks of the page. */
export interface ServerSearchDependencies {
  readonly factories: {
    createServerSearch(deps: unknown): unknown;
    createGathering(deps: unknown): unknown;
    createRepliesWalk(deps: unknown): unknown;
  };
  clock(): unknown;
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  selectedChannelId(): string;
  query(): string;
  oldestLoadedMs(): number | null;
  fetchAnswer(path: string): Promise<unknown>;
  renderServerSearch(): void;
  renderRepliesSearch(): void;
  allLoaded(): boolean;
  timeOfFirst(payload: unknown): number;
  canon(): unknown;
  guardQuietly<T>(fn: () => T): (...args: unknown[]) => Promise<unknown>;
  cacheOpen(): boolean;
  tokenScope(): string;
  prioritize(channel: string): Promise<unknown>;
  fetchHistoryState(channel: string): Promise<unknown>;
  scheduleServerSearch(): void;
  cardsOnScreen(): Element | null;
  inColumn<T>(column: unknown, fn: () => T): T;
  syncUnreadReplies(): void;
  saveChannelScope(): void;
  renderSideColumn(): void;
  serviceWords(): string;
  failureWords(failure: unknown): string;
  repliesFloor(): number | null;
  olderThreadsToRead(): boolean;
  searchOlderPages: number;
  searchOlderMs: number;
  searchOlderHits: number;
  searchOlderTapMs: number;
  repliesOffered(): boolean;
  endSearchWalk(column: unknown): unknown;
  endFilterWalk(): void;
  mainColumn(): unknown;
  sideColumn(): unknown;
  allCover(): { live: boolean; more: boolean; cursor: string | null } | null;
  repliesKey(): string;
  canon(): unknown;
  currentCanon(): unknown;
  heldNow(canon: unknown): unknown;
  readTimeline(path: string, options?: unknown): Promise<unknown>;
  pageLimit: number;
  newerPage(canon: unknown, view: string, number: number): boolean;
  foldLateTimelinePage(payload: unknown, read: unknown): void;
  placeFromPage(payload: unknown, canon: unknown, number: number, held: unknown): boolean;
  followPlacement(): void;
  foldTimelinePage(payload: unknown, older: boolean, view: string, thread: unknown): void;
  heldViews(): Map<string, { more: boolean; floor: number | null; cursor: string | null }>;
  flatViewKey(): string;
  sentNumberOf(payload: unknown): number;
  setStatus(text: string): void;
  searchParsed(query: string): { node: unknown };
  messageMatches(node: unknown, message: Message): boolean;
  threadOf(message: unknown): string | null;
  sideMode(): string;
  sideUp(): boolean;
}

/** The server search's commands and questions. */
/** The server search's state owner, as olderwalks reads it (core/search owns it). */
export interface ServerSearchOwnerLike {
  state(): { key: string; ticket: number; loading: boolean; failed: string; hits: Message[]; scopes: SearchScope[]; examined: number; resume: number | null };
  allChannels(): boolean;
  schedule(key: string, open: boolean): void;
  chooseAllChannels(on: unknown): void;
  run(more: boolean): Promise<void>;
  more(): Promise<void>;
  forget(): void;
}

/** A gathering of older history, as olderwalks reads it (core/search owns it). */
export interface GatheringLike {
  stop(): void;
  start(channel: string): Promise<void>;
  state(): { channel: string; failed: string; state: { reached_ms?: number; oldest_ms?: number; eta_s?: number; complete?: boolean; held_reason?: string } | null };
}

/** The cards' replies walk, as olderwalks reads it (core/search owns it). */
export interface RepliesWalksLike {
  current(): { stopping: boolean; arrived: number; pages: number } | null;
  search(): void;
  end(): void;
  holds(walk: unknown): boolean;
  noteFor(key: () => string): string;
  stop(): void;
  pressed(): unknown;
  forgetNote(): void;
}

export interface ServerSearch {
  readonly owner: ServerSearchOwnerLike;
  readonly gathering: GatheringLike;
  readonly repliesWalks: RepliesWalksLike;
  serverCopyOpen(): boolean;
  searchFurtherRunning(): boolean;
  stopRepliesWalk(): void;
  repliesControlPressed(): unknown;
  sideCardsShown(): boolean;
  cardsFound(): string[];

}

/** No search sent, no gathering under way. */
export function createServerSearchCopy(deps: ServerSearchDependencies): ServerSearch {
  const leftEl = deps.leftById;

  const serverSearchOwner = deps.factories.createServerSearch({
    clock: deps.clock(),
    pathFor: (more: boolean, resume: unknown) => {
      const channel = String(deps.selectedChannelId() || "");
      const zone = readerZone();
      const below = more ? resume : (serverSearchOwner as { allChannels(): boolean }).allChannels() ? null : deps.oldestLoadedMs();
      const params = [`q=${encodeURIComponent(deps.query())}`, ...(zone ? [`tz=${encodeURIComponent(zone)}`] : []),
        ...(below !== null && below !== undefined ? [`below=${below}`] : [])].join("&");
      return (serverSearchOwner as { allChannels(): boolean }).allChannels() ? `/api/v1/search?${params}`
        : `/api/v1/channels/${encodeURIComponent(channel)}/search?${params}`;
    },
    fetchAnswer: (path: string) => deps.fetchAnswer(path),
    render: () => deps.renderServerSearch(),
    guard: (fn: () => unknown) => deps.guardQuietly(fn)(),
  } as never) as ServerSearchOwnerLike;

  const serverCopyOpen = (): boolean => deps.cacheOpen() && deps.tokenScope() !== "read";

  function readerZone(): string {
    try {
      return String(Intl.DateTimeFormat().resolvedOptions().timeZone || "");
    } catch (_error) {
      return "";
    }
  }

  const gathering = deps.factories.createGathering({
    clock: deps.clock(),
    prioritize: (channel: string) => deps.prioritize(channel),
    fetchState: (channel: string) => deps.fetchHistoryState(channel),
    render: () => deps.renderServerSearch(),
    grew: () => {
      serverSearchOwner.forget();
      deps.scheduleServerSearch();
    },
    guard: (fn: () => unknown) => deps.guardQuietly(fn)(),
  } as never) as { state(): { channel: string } };

  const searchFurtherRunning = (): boolean => gathering.state().channel !== "" &&
    gathering.state().channel === String(deps.selectedChannelId() || "");

  const sideCardsShown = (): boolean => deps.cardsOnScreen() === leftEl("side-cards");

  const cardsFound = (): string[] => ([...leftEl("side-cards").children] as HTMLElement[])
    .filter((row) => row.hasAttribute("data-card") && rowShown(row))
    .map((row) => String(row.getAttribute("data-id")));

  function foldRepliesPage(payload: unknown, newest: boolean, held: unknown, walk: { channel: string; canon: unknown }): void {
    if (deps.currentCanon() !== walk.canon || String(deps.selectedChannelId()) !== walk.channel) return;
    if (newest) {
      const number = deps.sentNumberOf(payload);
      const read = { context: JSON.stringify([walk.channel, "flat", null]), channel: walk.channel, view: "flat",
        thread: null, canon: walk.canon, held };
      if (!deps.newerPage(deps.currentCanon(), "flat", number)) deps.foldLateTimelinePage(payload, read);
      else if (deps.placeFromPage(payload, walk.canon, number, held)) deps.followPlacement();
    } else {
      deps.foldTimelinePage(payload, true, "flat", null);
      const cover = deps.heldViews().get(deps.flatViewKey());
      const edge = deps.timeOfFirst(payload);
      if (cover && cover.more === true && Number.isFinite(edge) && cover.floor === edge) {
        cover.cursor = (payload as { next_before?: string }).next_before || null;
      }
      deps.inColumn(deps.mainColumn(), deps.syncUnreadReplies);
      deps.saveChannelScope();
    }
    deps.renderSideColumn();
  }

  function repliesWalkNote(walk: { matched: Set<string> }, reason: string, failure: unknown = null): string {
    const said = repliesEndNote(walk, reason, failure);
    const unlisted = repliesUnlisted(walk);
    if (unlisted === 0) return said;
    const more = deps.olderThreadsToRead() ? ": Older threads… loads more" : "";
    return `${said} ${unlisted} more thread${unlisted === 1 ? " with a matching reply is" : "s with a matching reply are"} ` +
      `not among the threads loaded${more}.`;
  }

  function repliesUnlisted(walk: { matched: Set<string> }): number {
    const carded = new Set([...leftEl("side-cards").children].filter((row) => row.hasAttribute("data-card"))
      .map((row) => String(row.getAttribute("data-id"))));
    return [...walk.matched].filter((id) => !carded.has(id)).length;
  }

  function repliesEndNote(walk: unknown, reason: string, failure: unknown = null): string {
    return repliesWalkWords(walk as never, reason, {
      service: deps.serviceWords(),
      budgetSeconds: Math.round(deps.searchOlderMs / 1000),
      floor: deps.repliesFloor(),
      failure: () => deps.failureWords(failure),
    } as never) as string;
  }

  const repliesWalks = deps.factories.createRepliesWalk({
    now: () => Date.now(),
    pages: deps.searchOlderPages,
    budgetMs: deps.searchOlderMs,
    hits: deps.searchOlderHits,
    tapMs: deps.searchOlderTapMs,
    offered: () => deps.repliesOffered(),
    endOthers: () => {
      const others = [deps.endSearchWalk(deps.mainColumn()), deps.endSearchWalk(deps.sideColumn())].filter((one) => one !== null);
      deps.endFilterWalk();
      return others;
    },
    cursor: () => {
      const cover = deps.allCover();
      return cover && cover.live && cover.more === true ? cover.cursor || null : null;
    },
    key: () => deps.repliesKey(),
    canon: () => deps.canon(),
    channel: () => String(deps.selectedChannelId()),
    cardsFound: () => cardsFound(),
    controller: () => (typeof AbortController === "function" ? new AbortController() : null),
    matcher: () => {
      const node = deps.searchParsed(deps.query()).node;
      return (message: Message) => node !== null && deps.messageMatches(node, message);
    },
    threadOf: (message: unknown) => deps.threadOf(message),
    isRoot: (message: Message) => (message as { thread?: { is_root?: boolean } }).thread?.is_root as boolean,
    allLoaded: () => deps.allLoaded(),
    holds: (walk: { key: string; canon: unknown }) => deps.repliesKey() === walk.key && deps.currentCanon() === walk.canon &&
      deps.sideUp() && deps.sideMode() === "list",
    heldNow: () => deps.heldNow(deps.currentCanon()),
    read: (channel: string, before: string | null, signal: unknown) => deps.readTimeline(
      `/api/v1/channels/${encodeURIComponent(channel)}/timeline?view=flat&limit=${deps.pageLimit}` +
      `${before === null ? "" : `&before=${encodeURIComponent(before)}`}&reason=search`,
      signal ? { signal } : undefined),
    fold: (page: unknown, newest: boolean, held: unknown, walk: { channel: string; canon: unknown }) => foldRepliesPage(page, newest, held, walk),
    render: () => deps.renderRepliesSearch(),
    failed: () => deps.setStatus("Could not search further back — try again."),
    ending: (walk: { matched: Set<string> }, reason: string, failure: unknown) => repliesWalkNote(walk, reason, failure),
  } as never) as { stop(): void; pressed(): unknown };

  function stopRepliesWalk(): void {
    repliesWalks.stop();
  }

  function repliesControlPressed(): unknown {
    return repliesWalks.pressed();
  }

  return { owner: serverSearchOwner, serverCopyOpen, searchFurtherRunning, stopRepliesWalk, repliesControlPressed, sideCardsShown, cardsFound,
    gathering: gathering as unknown as GatheringLike, repliesWalks: repliesWalks as unknown as RepliesWalksLike };
}

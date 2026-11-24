// Searching what is on screen, and the lines between rows (`#624 voice-page-modularity`, moved
// from the legacy script into view/channel): the search language on the page, hiding every row
// that does not match, the counts and empty sentences, the hits drawn where they are, and one
// pass drawing every seam — unmatched gaps and days. Why each behaves as it does is told where
// the legacy script held them. The field itself, the server's search, and the read runs stay
// their owners'; this asks for them.

import type { Message } from "../../contract/index.ts";
import { linksIn } from "../../foundation/index.ts";
import { LINK_KINDS, seamPlan } from "../../present/rows/index.ts";
import { dayClock, dayKeyOf, messageTimeLabel } from "../../present/time/index.ts";
import { classWords, setClassWord, setTextIfChanged } from "../dom/index.ts";
import { anchorsAmong, findableParts, textHits, textRunsOf } from "./hits.ts";
import { idsOf, isMessageRow, rowMessages, rowSentMs, rowShown } from "./rows.ts";
import { createSeamLines } from "./seams.ts";
import { renderedText } from "./text.ts";
import { rowTime } from "./time.ts";
import type { SeamLines } from "./seams.ts";

/** The lists a filter may touch, in document order. */
export const SEARCH_LISTS = ["transcript", "thread-list", "discord-log", "pinned-log", "outgoing-log"];

const MAIN_REPLIES_SEARCHED = "Main shows a thread when a reply held here matches; All may hold more replies.";
const SEARCH_EMPTY_LOADED = "Nothing in the messages loaded so far matches that search.";
const SEARCH_HIT_CLASS = "search-hit";
const SEARCH_HIT_LINK_CLASS = "search-hit-link";
const EXCERPT_SURELY_SHOWN = 60;
const EXCERPT_CONTEXT = 30;
const EXCERPT_PLACES = 3;

/** The search field, as far as the filter reads it (core/search owns it; the page instantiates it). */
export interface SearchFieldLike {
  open(): boolean;
  query(): string;
  marked(): string;
  links(): boolean;
  markSoon(): void;
  markNow(): void;
}

/** A row carrying what the search put on it. */
type SearchRow = HTMLElement & {
  searchMarks?: {
    key: string; links: Node | null; marks: HTMLElement[]; outlined: Element[];
    author: Element | null; excerpt: HTMLElement | null;
  } | null;
  linkView?: { node?: Node } | null;
  foldEntry?: { body: HTMLElement } | null;
  linkText?: string;
  /** `#1913`: the row's findable text, folded once when the row is drawn (`searchFold`), so the
   * typing-fast pass matches with substring checks and never re-folds or re-reads a message. */
  foldedSearch?: string;
  /** `#1984 vibe-talk-search-typing-changed-only`: the `search-hidden` word as this owner last
   * left it, so a pass writes the class only on rows whose visibility actually changed. */
  searchHidden?: boolean;
  runDays?: string;
  seamLine?: unknown;
};

/** One column's count from its last pass of the search. */
export interface SearchCount {
  matched: number;
  loaded: number;
  /** `#2200 vibe-talk-search-tab-counts-match-cards`: the loaded half of the one
   * search-result fold — matched loaded messages per disjoint bucket
   * (Main = top-level, Threads = replies only, Transcripts = classed). */
  buckets: { main: number; threads: number; transcripts: number };
  tally: {
    linked: number; kept: number; saidLinked: number;
    everywhere: Map<string, number>; matching: Map<string, number>;
  };
  pinned: boolean;
}

/** The shared search language, as far as the filter uses it (core/search owns it). */
export interface SearchLanguage {
  parsedQueries: { parsed(query: string): { node: unknown; terms: string[] } };
  openedGaps: { ids(key: string, view: string, create: boolean): Set<string> | null; cards(key: string, create: boolean): { threads: Set<string>; ids: Set<string> } | null };
  searchSubject(parts: unknown): unknown;
  searchQueryMatches(node: unknown, subject: unknown, dayStart: unknown): boolean;
  searchQueryTree(node: unknown): unknown;
  searchFold(text: unknown): string;
}

/** What searching the rows asks of the page. */
export interface SearchRowsDependencies {
  readonly searchField: SearchFieldLike;
  readonly language: SearchLanguage;
  /** `#1825`: expand a parsed query's `label:` slugs through the label groups. */
  expandLabelQuery(node: unknown): unknown;
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  view(): string;
  screen(): string;
  side(): boolean;
  sideShowing(): boolean;
  sideColumnUp(): boolean;
  inColumn<T>(column: unknown, fn: () => T): T;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  mainColumn(): unknown;
  sideColumnRecord(): { searched: SearchCount | null };
  mainColumnRecord(): { searched: SearchCount | null };
  setSearched(count: SearchCount): void;
  searchWalkHeld(): boolean;
  preservingScroll(fn: () => void): void;
  /** `#2200`: bring a row into view through the scroll owner (`#1251`), so an
   * opened run's first row lands fully below the chip panel. Optional for doubles. */
  intoView?(element: Element, block: string): void;
  applyReadRuns(): void;
  threading(): boolean;
  heldMessages(): Message[];
  currentChannel(): string;
  selectedChannelId(): string;
  threadOf(message: unknown): string | null;
  messageText(message: Message): string;
  messageLinks(message: Message): Array<{ href: unknown; name?: unknown }>;
  rawTextOf(message: Message): string;
  selectionView(): string;
  authorChoosing(): string;
  authorPasses(row: Element, author: string): boolean;
  authorEmptySentence(author: string, typed: boolean, pinned: boolean): string;
  gatheredParent(): unknown;
  putRepliesBack(): void;
  pinsOnly(): boolean;
  /** `#1853`: whether a message id is in the channel's pin set, read fresh at
   * match time (pinning never replaces the message object, so the cached
   * subject cannot carry it). Optional for test doubles. */
  isPinnedId?(id: unknown): boolean;
  showRowLinks(row: Element): number;
  takeDownRowLinks(row: Element): number;
  heldLinks(row: Element): Array<{ kind: string }>;
  hiddenKindNames(tally: Map<string, number>): string;
  renderLinkKindCounts(counts: Map<string, number>): void;
  drawCardGaps(list: HTMLElement): void;
  openedCardGaps(create: boolean): { threads: Set<string> } | null;
  openUnpackGap(row: Element, below: boolean): HTMLElement | null;
  openCardGap(row: Element, below: boolean): HTMLElement | null;
  renderReplyLinks(): void;
  renderOlderControl(): void;
  scheduleServerSearch(): void;
  /** `#1913`: retire bucket chips earned by an older query (O(1), no fetch). Optional for doubles. */
  retireStaleBuckets?(): void;
  /** `#2200`: whether a bucket's results stand in the timeline, or null where the
   * bucket fold is not standing for this search (no server search in play). */
  bucketOn?(id: string): boolean | null;
  /** `#2200`: the one search-result fold's counts per bucket chip, or null where
   * the fold is not standing. `total` is their sum. */
  foldCounts?(): {
    counts: Map<string, number>; total: number;
    loadedSelected: number; freshSelected: number;
  } | null;
  placeInlineReply(): void;
  placeRowsInPlaceAgain(): void;
  replyDocked(): boolean;
  changeChannelView(view: string): unknown;
  channelViewWords(): string;
  historyReach(): { whole: boolean; start: boolean; topLevel: boolean; saved: boolean; proven: boolean; dense: number; from: number | null; threads: number; full: number };
  historyReadToStart(): boolean;
  messagesWords(count: number): string;
  searchMomentWords(at: number): string;
  reachBackWords(from: number | null): string;
  reachClause(reach: unknown): string;
  oldestLoadedMs(): number | null;
  holdsWholeHistory(): boolean;
  hidingRead(): boolean;
  threadListOnScreen(): boolean;
  cardCount(): { cards: number; shown: number };
  pinProblem(): string;
  pinRevision(): unknown;
  pinChannel(): string | null;
  currentChannelId(): string | null;
  isFolded(entry: { li: Element }): boolean;
  readRunsSuspended(): boolean;
  searchKey(): string;
  viewKey(): string;
  runDaysOf(row: SearchRow): string;
  placeInReadRun(row: SearchRow, role: string, count: number): void;
  readRunCount(row: SearchRow): number;
  changing(): boolean;
  currentFilterHold(): { row: unknown } | null;
  keepFilterHold(): void;
  visibleList(): Element | null;
}

/** The search's commands and questions. */
export interface SearchRows {
  markSearchSoon(): void;
  markSearchNow(): void;
  searchParsed(query: string): { node: unknown; terms: string[] };
  searchTyped(query: string): boolean;
  searchIdentity(query: string): string;
  searchable(row: Element, ...parts: unknown[]): void;
  applySearch(): void;
  /** `#1913`: the typing-fast pass — row visibility from cached folded text only. */
  applySearchFast(): void;
  searchInAll(): unknown;
  renderSearchInAll(): void;
  renderSearchCount(typed: boolean, links: boolean, author?: boolean): void;
  refreshSearchCount(): void;
  listEmptied(): boolean;
  drawSeams(list: HTMLElement): void;
  seamsAfterMidnight(): void;
  openSearchGap(row: Element, below: boolean): HTMLElement | null;
  inOpenedGap(row: Element, opened: Set<string> | null): boolean;
  rowsChosen(): boolean;
  rowDay(row: Element): number | null;
  markSearchHits(row: Element, terms: string[]): void;
  messageMatches(node: unknown, message: unknown): boolean;
  sayCardCount(): void;
  readonly openedGaps: SearchLanguage["openedGaps"];
  placeSeam(row: Element, seam: unknown, below: boolean): void;
  seamDrawn(row: Element, seam: unknown, below: boolean): boolean;
  threadsWithHits(node: unknown): Set<string>;
  rowMatches(row: Element, node: unknown, text: string | null): boolean;
  /** `#1825`, on the search bar since `#1852`: "Whole threads" — a thread with any hit shows whole. Off by default. */
  setEntireThreads(on: boolean): void;
  entireThreads(): boolean;
  /** `#1895 vibe-talk-link-search-semantics`: the query's folded terms while Links is on, else null. */
  linkTerms(): string[] | null;
  /** `#1895`: threads holding a held message whose link address or name holds every term. */
  threadsWithLinkHits(terms: readonly string[]): Set<string>;
  /** `#1895`: threads holding any link at all, for Links with no query. */
  threadsWithLinks(): Set<string>;
  /** `#1895`: whether a row's own links hold every term, in address or name. */
  rowLinkMatch(row: Element, terms: readonly string[]): boolean;
  /** `#1895`: whether a row holds any link at all. */
  rowHasLink(row: Element): boolean;
}

/** No search open, no seams drawn. */
export function createSearchRows(deps: SearchRowsDependencies): SearchRows {
  const el = deps.byId;
  const leftEl = deps.leftById;
  const searchField = deps.searchField;

  function markSearchSoon(): void {
    searchField.markSoon();
  }

  function markSearchNow(): void {
    searchField.markNow();
  }

  /** `#1984`: put `search-hidden` on `row` only where its visibility changed. The per-row bit is
   * reconciled with the class the row actually wears, so a row another writer (the side cards)
   * changed is never left stale: a matching class costs no DOM write, a differing one is written
   * once and the bit follows it. */
  function setSearchHidden(row: SearchRow, hidden: boolean): void {
    const wears = classWords(row).includes("search-hidden");
    if (row.searchHidden === hidden && wears === hidden) return;
    if (wears !== hidden) setClassWord(row as never, "search-hidden", hidden);
    row.searchHidden = hidden;
  }

  const parsedQueries = deps.language.parsedQueries;

  function searchParsed(query: string): { node: unknown; terms: string[] } {
    return parsedQueries.parsed(query);
  }

  const searchTyped = (query: string): boolean => searchParsed(query).node !== null;
  const searchIdentity = (query: string): string => JSON.stringify(deps.language.searchQueryTree(searchParsed(query).node));
  const searchDayStart = (day: { year: number; month: number; day: number }): number =>
    new Date(day.year, day.month - 1, day.day).getTime();

  const subjectByMessage = new WeakMap<object, unknown>();

  // `#1895 vibe-talk-link-search-semantics`. Every link a message holds, for
  // matching: the drawn anchors and the addresses read from its text as
  // written, joined by address, the fuller name kept. The drawn anchors alone
  // lose a name the writing holds (a Slack `<url|label>` nested in a markdown
  // `[label](url)`, whose drawn words may be shortened), and the text alone
  // loses what only the rendering drew; together they are the links the
  // reader could follow.
  const matchLinksByMessage = new WeakMap<object, Array<{ href: string; name: string }>>();

  function messageMatchLinks(message: Message): Array<{ href: string; name: string }> {
    let held = matchLinksByMessage.get(message);
    if (!held) {
      const byAddress = new Map<string, { href: string; name: string }>();
      const add = (href: unknown, name: unknown): void => {
        const address = String(href || "");
        if (!address) return;
        const words = String(name || "") || address;
        const kept = byAddress.get(address);
        // The fuller name is kept: a drawn name may be the written one cut
        // short, and the cut words are exactly what a search may ask for.
        if (!kept) byAddress.set(address, { href: address, name: words });
        else if (words.length > kept.name.length) kept.name = words;
      };
      for (const link of deps.messageLinks(message)) add(link.href, link.name);
      for (const link of linksIn(deps.rawTextOf(message))) add(link.href, link.name);
      held = [...byAddress.values()];
      matchLinksByMessage.set(message, held);
    }
    return held;
  }

  const linkTextByMessage = new WeakMap<object, string>();

  /** A message's link addresses and names, folded as one text (`#1895`). */
  function messageLinkText(message: Message): string {
    let text = linkTextByMessage.get(message);
    if (text === undefined) {
      text = deps.language.searchFold(messageMatchLinks(message).map((link) => `${link.href}\n${link.name}`).join("\n"));
      linkTextByMessage.set(message, text);
    }
    return text;
  }

  /** Whether every term is a folded substring of a message's link addresses or names (`#1895`). */
  const messageLinkMatch = (message: Message, terms: readonly string[]): boolean =>
    terms.length > 0 && terms.every((term) => messageLinkText(message).includes(term));

  function messageSubject(message: Message): unknown {
    let subject = subjectByMessage.get(message);
    if (!subject) {
      subject = deps.language.searchSubject({
        author: String(message.author || ""),
        authorId: String((message as { author_id?: unknown }).author_id || ""),
        text: deps.messageText(message),
        // `#1895`: a link's name is searchable text beside its address, so a
        // word held only in a label is found, with Links on or off.
        links: messageMatchLinks(message).flatMap((link) => [link.href, link.name]),
        createdMs: Date.parse(String((message as { timestamp?: unknown }).timestamp)) || 0,
        inThread: Boolean((message as { thread?: { is_root?: boolean } }).thread && (message as { thread?: { is_root?: boolean } }).thread?.is_root === false),
        // `#1825`: the labels the server derived and served with the message.
        labels: ((message as { labels?: unknown }).labels as string[] | undefined) || [],
      } as never);
      subjectByMessage.set(message, subject);
    }
    return subject;
  }

  const messageMatches = (node: unknown, message: unknown): boolean => {
    // `#1853`: `pinned` is read fresh (see `isPinnedId`), never from the cache.
    const base = messageSubject(message as Message) as { pinned?: boolean };
    const subject = { ...base, pinned: deps.isPinnedId ? deps.isPinnedId((message as Message).id) : false };
    return deps.language.searchQueryMatches(node, subject as never, searchDayStart as never);
  };

  function rowMatches(row: Element, node: unknown, text: string | null): boolean {
    const messages = rowMessages(row);
    if (messages.length > 0) return messages.some((message) => messageMatches(node, message));
    return text !== null && deps.language.searchQueryMatches(node, deps.language.searchSubject({ text }), searchDayStart as never);
  }

  function threadsWithHits(node: unknown): Set<string> {
    const hits = new Set<string>();
    if (!node || !deps.threading() || deps.currentChannel() !== String(deps.selectedChannelId())) return hits;
    for (const message of deps.heldMessages()) {
      const id = deps.threadOf(message);
      const thread = (message as { thread?: { is_root?: boolean } }).thread;
      if (id && thread && thread.is_root === false && messageMatches(node, message)) hits.add(String(id));
    }
    return hits;
  }

  // `#1895 vibe-talk-link-search-semantics`. Under Links the query is read
  // against the links themselves: a thread holds a hit when any held message
  // of it — its root or a reply — has a link whose address or name holds the
  // terms, and with no query every thread holding a link at all is listed.
  function threadsWithLinkHits(terms: readonly string[]): Set<string> {
    const hits = new Set<string>();
    if (terms.length === 0 || !deps.threading() || deps.currentChannel() !== String(deps.selectedChannelId())) return hits;
    for (const message of deps.heldMessages()) {
      const id = deps.threadOf(message);
      if (id && messageLinkMatch(message, terms)) hits.add(String(id));
    }
    return hits;
  }

  function threadsWithLinks(): Set<string> {
    const hits = new Set<string>();
    if (!deps.threading() || deps.currentChannel() !== String(deps.selectedChannelId())) return hits;
    for (const message of deps.heldMessages()) {
      const id = deps.threadOf(message);
      if (id && messageMatchLinks(message).length > 0) hits.add(String(id));
    }
    return hits;
  }

  function linkTerms(): string[] | null {
    if (!searchField.open() || !searchField.links()) return null;
    return searchParsed(searchField.query()).terms;
  }

  function rowLinkMatch(row: Element, terms: readonly string[]): boolean {
    if (terms.length === 0) return false;
    const messages = rowMessages(row);
    if (messages.length > 0) return messages.some((message) => messageLinkMatch(message, terms));
    const text = (row as SearchRow).linkText;
    if (typeof text !== "string") return false;
    const folded = deps.language.searchFold(linksIn(text).map((link) => `${link.href}\n${link.name}`).join("\n"));
    return terms.every((term) => folded.includes(term));
  }

  function rowHasLink(row: Element): boolean {
    const messages = rowMessages(row);
    if (messages.length > 0) return messages.some((message) => messageMatchLinks(message).length > 0);
    const text = (row as SearchRow).linkText;
    return typeof text === "string" && linksIn(text).length > 0;
  }

  // `#1825` "Whole threads" (the search-bar toggle since `#1852`): off,
  // individual hits as always; on, every held message of a thread with a hit
  // stands. It ANDs with the #1797 buckets: buckets choose which scopes
  // contribute hits, this chooses whether a hit brings its thread with it.
  // The one parsed query drives it, so a plain word and a `label:` search
  // (group-expanded in `applySearch`) behave the same.
  let entireThreadsOn = false;

  /** Every thread (by `threadOf`) holding a held message that matches. */
  function entireThreadHits(node: unknown): Set<string> {
    const hits = new Set<string>();
    if (!node || !deps.threading()) return hits;
    for (const message of deps.heldMessages()) {
      const id = deps.threadOf(message);
      if (id && messageMatches(node, message)) hits.add(String(id));
    }
    return hits;
  }

  const rowInHitThread = (row: Element, hits: Set<string>): boolean => rowMessages(row).some((message) => {
    const id = deps.threadOf(message);
    return Boolean(id) && hits.has(String(id));
  });

  const rowHeadsHitThread = (row: Element, hits: Set<string>): boolean => rowMessages(row).some((message) => {
    const id = deps.threadOf(message);
    const thread = (message as { thread?: { is_root?: boolean } }).thread;
    return Boolean(id) && Boolean(thread) && (thread as { is_root?: boolean }).is_root === true && hits.has(String(id));
  });

  function searchable(row: Element, ...parts: unknown[]): void {
    const text = parts.filter(Boolean).join(" ");
    row.setAttribute("data-search", text.toLowerCase());
    // `#1913`: folded once here, at draw time, so a keystroke never pays to fold it again.
    (row as SearchRow).foldedSearch = deps.language.searchFold(text);
  }

  /** Whether `tree` (the corpus notation of a parsed query) is only plain words/phrases joined
   * by AND — the shape the typing-fast pass can answer exactly from cached folded text. Any other
   * shape (OR, negation, field operators) passes through fast and is settled by the full pass. */
  function plainAndOnly(tree: unknown): boolean {
    if (!Array.isArray(tree) || tree.length === 0) return false;
    const kind = tree[0];
    if (kind === "word" || kind === "phrase") return true;
    if (kind === "and") return tree.slice(1).every((child) => plainAndOnly(child));
    return false;
  }

  /**
   * `#1913 vibe-talk-search-typing-lag`: the pass that runs inside the input event. It only
   * toggles each row's `search-hidden` word from the row's cached folded text (substring checks,
   * no message re-read, no link/thread scan, no seam draw, no layout read). Everything slower —
   * the count and empty sentence, thread and link lookups, tallies and link-kind counts, seams,
   * hit marks, the server ask and the place-keeping hold — is the full `applySearch`, which the
   * field's typing schedules once the typing pauses, in the same words it uses today. A query
   * the fast pass cannot answer exactly (operators, Links, an author or Pinned narrowing, Whole
   * threads) is shown through here and settled exactly by that full pass moments later.
   */
  function applySearchFast(): void {
    const node = searchField.open() ? searchParsed(searchField.query()).node : null;
    const typed = node !== null;
    const links = searchField.open() && searchField.links();
    const author = deps.authorChoosing();
    const terms = typed ? searchParsed(searchField.query()).terms : [];
    const exact = typed && !links && author === "everyone" && !deps.pinsOnly() && !entireThreadsOn &&
      plainAndOnly(deps.language.searchQueryTree(node));
    for (const id of SEARCH_LISTS) {
      if (deps.side() && id === "transcript") continue;
      const list = el(id);
      for (const row of list.children) {
        // `#2200`: server-placed rows follow their bucket even on the fast
        // pass, so an edited query never leaves a toggled-off bucket's rows
        // standing while the full pass is still to come.
        if (row.hasAttribute("data-server-hit")) {
          const bucket = row.getAttribute("data-bucket");
          const on = !deps.bucketOn || bucket === null ? null : deps.bucketOn(bucket);
          if (on === false) setSearchHidden(row as SearchRow, true);
          continue;
        }
        const text = row.getAttribute("data-search");
        if (text === null) continue;
        let hit = true;
        if (typed && exact) {
          const folded = (row as SearchRow).foldedSearch ?? text;
          hit = terms.every((term) => folded.includes(term));
        }
        setSearchHidden(row as SearchRow, !hit);
      }
    }
    // `#1913`: retire the previous query's bucket chips at once (O(1)); the server ask and
    // its counts stay debounced with the full pass — no fetch, no recompute leaves this event.
    if (!deps.side() && deps.retireStaleBuckets) deps.retireStaleBuckets();
  }

  /** `#2200`: the one disjoint bucket a message counts in: Transcripts when its
   * thread is classed, Threads for a reply only, Main for everything else
   * (top-level and thread roots). The same rule `hitBucket` partitions the
   * server's fresh hits by, so a message is never in two buckets. */
  function messageBucket(message: Message): "main" | "threads" | "transcripts" {
    const thread = (message as { thread?: { is_root?: boolean; class?: unknown } }).thread;
    if (thread && thread.class) return "transcripts";
    if (thread && thread.is_root === false) return "threads";
    return "main";
  }

  /** `#2200`: whether `row`'s bucket stands in the timeline under the fold:
   * true where the fold is not standing (older behaviour), else the bucket's
   * own toggle. A row's bucket is its own messages' bucket: a thread root
   * standing for its matching replies is a Main card, as it always was —
   * the replies themselves are the Threads hits, and they have no row in
   * Main. (`viaThread` is kept for the tally's sake, not the gate's.) */
  function bucketPasses(row: Element, viaThread: boolean): boolean {
    void viaThread;
    if (!deps.bucketOn) return true;
    const probe = deps.bucketOn("main");
    if (probe === null) return true;
    const messages = rowMessages(row);
    let bucket: string = "main";
    if (!viaThread && messages.length > 0) {
      const buckets = new Set(messages.map((message) => messageBucket(message)));
      bucket = buckets.size === 1 ? [...buckets][0]! : buckets.has("threads") && !buckets.has("main") ? "threads" : "main";
    }
    // In a thread view every loaded row is this thread's, whatever its shape.
    if (bucket !== "transcripts" && deps.selectionView() === "thread") bucket = "thread";
    return deps.bucketOn(bucket) === true;
  }

  function searchScope(): string[] {
    return SEARCH_LISTS.filter(
      (id) => !el(id).hidden && (id === "transcript") === (deps.view() === "voice")
    );
  }

  function applySearch(): void {
    deps.applyReadRuns();
    const node = searchField.open() ? searchParsed(searchField.query()).node : null;
    // `#1825`: matching sees the group-expanded query; identity/terms stay raw.
    const matchNode = node !== null ? deps.expandLabelQuery(node) : null;
    const typed = node !== null;
    const marking = searchField.open() ? searchParsed(searchField.marked()).terms : [];
    const replyHits = typed && deps.threading() && (deps.selectionView() === "main" || deps.selectionView() === "threads")
      ? threadsWithHits(matchNode) : null;
    const wholeThreads = typed && entireThreadsOn ? entireThreadHits(matchNode) : null;
    const links = searchField.open() && searchField.links();
    // `#1895`: under Links, the query against the links themselves, and the
    // threads holding a link, so cards and roots are judged by their threads.
    const termsForLinks = links ? searchParsed(searchField.query()).terms : [];
    const linkThreadHits = links && typed ? threadsWithLinkHits(termsForLinks) : null;
    const linkThreads = links && !typed ? threadsWithLinks() : null;
    const author = deps.authorChoosing();
    if (deps.gatheredParent() !== null && (typed || links || deps.pinsOnly() || author !== "everyone")) deps.putRepliesBack();
    const scope = searchScope();
    let matched = 0;
    let loaded = 0;
    const buckets = { main: 0, threads: 0, transcripts: 0 };
    const tally = {
      linked: 0,
      kept: 0,
      saidLinked: 0,
      everywhere: new Map<string, number>(LINK_KINDS.map(({ kind }) => [kind, 0])),
      matching: new Map<string, number>(LINK_KINDS.map(({ kind }) => [kind, 0])),
    };
    const opened = openedGapIds(false);
    const openedCards = deps.openedCardGaps(false);
    for (const id of SEARCH_LISTS) {
      if (deps.side() && id === "transcript") continue;
      const list = el(id);
      const counts = scope.includes(id);
      for (const row of list.children) {
        // `#2200`: a server-placed row was judged by the server's search, so
        // this pass neither re-judges nor counts it — but it is no longer
        // exempt from the filter: it stands exactly when its bucket stands,
        // so a stale placed row of an edited query or a toggled-off bucket
        // never lingers in the timeline.
        if (row.hasAttribute("data-server-hit")) {
          const bucket = row.getAttribute("data-bucket");
          const on = !deps.bucketOn || bucket === null ? null : deps.bucketOn(bucket);
          setSearchHidden(row as SearchRow, on === false);
          continue;
        }
        const text = row.getAttribute("data-search");
        const shown = links && text !== null ? deps.showRowLinks(row) : deps.takeDownRowLinks(row);
        // `#2200`: judged once, here, and only while a search is typed (the
        // pass's old short-circuit): a second `rowMatches` per row per pass
        // re-renders bodies (the rendered-body test counts template parses).
        const direct = typed && text !== null && rowMatches(row, matchNode, text);
        const said = !typed || (text !== null && (direct ||
          (links && rowLinkMatch(row, termsForLinks)) || (replyHits !== null &&
          (id === "thread-list" ? replyHits.has(String(row.getAttribute("data-context-id")))
            : id === "discord-log" && rowHeadsHitThread(row, replyHits))) ||
          (linkThreadHits !== null &&
            (id === "thread-list" ? linkThreadHits.has(String(row.getAttribute("data-context-id")))
              : id === "discord-log" && rowHeadsHitThread(row, linkThreadHits))) ||
          (wholeThreads !== null && wholeThreads.size > 0 &&
            (id === "thread-list" ? wholeThreads.has(String(row.getAttribute("data-context-id")))
              : rowInHitThread(row, wholeThreads)))));
        // `#1895`: a card, and a root heading a thread, hold no link list of
        // their own; under Links they stand when their thread holds the link.
        const threadLinkHit = id === "thread-list"
          ? (linkThreadHits !== null && linkThreadHits.has(String(row.getAttribute("data-context-id")))) ||
            (linkThreads !== null && linkThreads.has(String(row.getAttribute("data-context-id"))))
          : id === "discord-log" && linkThreadHits !== null && rowHeadsHitThread(row, linkThreadHits);
        // `#2200`: the bucket fold gates loaded rows too — a row stands only
        // when its bucket's chip stands. A row that stands only because a
        // reply of its thread matched is a Threads hit, not a Main one.
        const viaThread = !direct && replyHits !== null &&
          (id === "thread-list" ? replyHits.has(String(row.getAttribute("data-context-id")))
            : id === "discord-log" && rowHeadsHitThread(row, replyHits));
        const matchedRow = said && (!links || shown > 0 || threadLinkHit) && deps.authorPasses(row, author);
        const hit = matchedRow && bucketPasses(row, viaThread);
        const revealed = !hit && text !== null && (id === "thread-list"
          ? openedCards !== null && openedCards.threads.has(String(row.getAttribute("data-context-id")))
          : inOpenedGap(row, opened));
        setSearchHidden(row as SearchRow, !(hit || revealed));
        markSearchHits(row as SearchRow, hit && text !== null ? marking : []);
        // `#1786`: rows the server search placed are not loaded messages, so
        // neither the denominator nor the match count speaks for them.
        if (counts && text !== null && !row.hasAttribute("data-absorbed") && !row.hasAttribute("data-server-hit")) {
          const weight = Math.max(1, idsOf(row).length);
          loaded += weight;
          const held = links ? deps.heldLinks(row) : [];
          if (held.length > 0) tally.linked += 1;
          if (shown > 0) tally.kept += 1;
          if (said && held.length > 0) tally.saidLinked += 1;
          for (const link of held) {
            tally.everywhere.set(link.kind, (tally.everywhere.get(link.kind) || 0) + 1);
            if (said) tally.matching.set(link.kind, (tally.matching.get(link.kind) || 0) + 1);
          }
          if (hit) matched += weight;
          if (matchedRow) {
            // `#2200`: the loaded half of the fold, per disjoint bucket,
            // tallied BEFORE the bucket gate: an off bucket's chip still
            // says how many it holds, and toggling it on changes only which
            // cards stand, never the counts. Each message counts in its
            // own bucket (a root in Main, its replies in Threads), so the
            // breakdown is disjoint and sums to the fold's total.
            void viaThread;
            const messages = rowMessages(row);
            if (messages.length > 0) for (const message of messages) buckets[messageBucket(message)] += 1;
            else buckets.main += weight;
          }
        }
      }
    }
    drawSeams(el("discord-log"));
    drawSeams(el("pinned-log"));
    deps.drawCardGaps(el("thread-list"));
    deps.renderReplyLinks();
    // `#1762 vibe-talk-server-hits-collapsed-line`. The signpost over the loaded
    // cards' matches: one element, shown while a search is typed and never
    // reworded here, so this pass pays nothing for it per keystroke.
    if (!deps.side()) {
      const sep = el("search-screen-sep");
      const show = typed && deps.view() === "discord";
      if (sep.hidden === show) sep.hidden = !show;
    }
    const pinned = deps.pinsOnly() && deps.view() === "discord";
    deps.setSearched({ matched, loaded, tally, pinned, buckets });
    renderSearchCount(typed, links, author !== "everyone");
    if (deps.view() === "discord") deps.renderOlderControl();
    if (!deps.side()) renderSearchInAll();
    if (!deps.side()) deps.scheduleServerSearch();
    if (pinned) {
      const sentence = pinnedEmptySentence(loaded, matched, typed, links ? tally : null, author);
      el("search-empty").textContent = sentence;
      el("search-empty").hidden = sentence === "";
      return;
    }
    if (links) {
      const sentence = linksEmptySentence(loaded, tally, matched, author);
      el("search-empty").textContent = sentence;
      el("search-empty").hidden = sentence === "";
      return;
    }
    if (author !== "everyone") {
      const sentence = deps.view() === "discord" && deps.selectionView() === "threads"
        ? "This is a list of threads, and who wrote what is in their messages — open a thread to see it."
        : matched > 0 || loaded === 0 ? "" : deps.authorEmptySentence(author, typed, false);
      el("search-empty").textContent = sentence;
      el("search-empty").hidden = sentence === "";
      return;
    }
    const empty = !typed || matched > 0 || loaded === 0;
    if (!deps.searchWalkHeld()) setTextIfChanged(el("search-empty") as never, empty ? SEARCH_EMPTY_LOADED : searchEmptySentence(loaded));
    el("search-empty").hidden = empty;
    if (deps.replyDocked()) deps.placeInlineReply();
    else deps.placeRowsInPlaceAgain();
  }

  function searchInAllOffered(searched: SearchCount | null): boolean {
    return searched !== null && !searched.pinned && searched.matched === 0 && searched.loaded > 0 && searchField.open() &&
      !searchField.links() && deps.authorChoosing() === "everyone" && searchTyped(searchField.query()) &&
      deps.view() === "discord" && deps.threading() && !deps.sideColumnUp() && deps.selectionView() === "main";
  }

  function renderSearchInAll(): void {
    deps.inColumn(deps.mainColumn(), () => {
      const all = leftEl("search-in-all");
      const offered = searchInAllOffered(deps.mainColumnRecord().searched);
      if (all.hidden === offered) all.hidden = !offered;
    });
  }

  function searchInAll(): unknown {
    if (!deps.inColumn(deps.mainColumn(), () => searchInAllOffered(deps.mainColumnRecord().searched))) {
      renderSearchInAll();
      return undefined;
    }
    return deps.inColumn(deps.mainColumn(), () => deps.changeChannelView("flat"));
  }

  function searchEmptySentence(loaded: number): string {
    if (deps.view() !== "discord" || deps.selectionView() === "threads") return SEARCH_EMPTY_LOADED;
    const view = deps.channelViewWords();
    const reach = deps.historyReach();
    let said: string;
    if (reach.whole && deps.historyReadToStart()) {
      said = `Searched all ${deps.messagesWords(loaded)} of ${view} in this browser, back to the first one` +
        `${reach.from === null ? "" : ` (${deps.searchMomentWords(reach.from)})`}. None match.`;
    } else if (reach.saved) {
      said = `Searched ${deps.messagesWords(loaded)} saved on this device${deps.reachBackWords(reach.from)}, not yet checked ` +
        "for gaps. None match.";
    } else if (reach.proven) {
      said = `Searched ${deps.messagesWords(reach.dense)} in this browser${reach.topLevel ? ":" : ","} ${deps.reachClause(reach)}. None match.`;
    } else {
      said = `Searched the ${deps.messagesWords(loaded)} loaded in this browser${deps.reachBackWords(deps.oldestLoadedMs())}. None match.`;
    }
    if (deps.holdsWholeHistory()) {
      said += " This server holds the complete history of this channel, which this search does not reach yet.";
    }
    if (deps.threading() && deps.selectionView() === "main") return `${said} ${MAIN_REPLIES_SEARCHED}`;
    if (deps.hidingRead() && !deps.threading()) return `${said} Hide read is on — press h for Show read to search older messages.`;
    return said;
  }

  function renderSearchCount(typed: boolean, links: boolean, author: boolean = false): void {
    if (deps.threadListOnScreen()) {
      sayCardCount();
      return;
    }
    // `#2200`: in a thread view under a standing search, the first result
    // stands below the chip panel. The thread header stands over the
    // list there, and both the carried scroll and the entry's own
    // landing can leave results under the panel (only their last lines
    // showing), so this is checked on every pass: while the first shown
    // row's top is above the panel's bottom, it is landed through the
    // scroll owner, whose margin parks it below the panel.
    if (deps.selectionView() === "thread" && deps.intoView && searchField.open()) {
      const panel = el("search-bucket-row");
      const first = [...el("discord-log").children].find((row) => isMessageRow(row) && rowShown(row));
      if (first && panel && !panel.hidden &&
        first.getBoundingClientRect().top < panel.getBoundingClientRect().bottom - 1) {
        deps.intoView(first, "start");
      }
    }
    const left = deps.mainColumnRecord().searched;
    if (!left) return;
    const counted = [left, ...(deps.sideShowing() && deps.sideColumnRecord().searched && !left.pinned ? [deps.sideColumnRecord().searched as SearchCount] : [])];
    if (links) {
      deps.renderLinkKindCounts(new Map<string, number>(LINK_KINDS.map(({ kind }) =>
        [kind, counted.reduce((sum, one) => sum + (one.tally.matching.get(kind) || 0), 0)])));
    }
    const matched = counted.reduce((sum, one) => sum + one.matched, 0);
    const loaded = counted.reduce((sum, one) => sum + one.loaded, 0);
    const filtering = typed || links || author;
    // `#2200`: while the bucket fold stands, the first number is the sum
    // of the SELECTED chips, and the number of result cards the selected
    // buckets stand in the timeline. When the server brought hits beyond
    // the loaded set, the line says so in the owner's words — "5 found
    // (4 loaded, 1 older)" — never "5 of 4 loaded".
    const fold = !links && deps.foldCounts ? deps.foldCounts() : null;
    // The long form ("N found (L loaded, M older)") gets its own line
    // under the field on a phone (liaison, #2200); the short form keeps
    // its corner. The stylesheet keys on this attribute.
    const countEl = el("search-count");
    const form = fold !== null && fold.freshSelected > 0 ? "found" : "";
    if (countEl.getAttribute("data-form") !== form) countEl.setAttribute("data-form", form);
    el("search-count").textContent = left.pinned
      ? (!filtering ? `${loaded} pinned` : `${matched} of ${loaded} pinned`)
      : (!filtering ? "" : fold !== null
        ? (fold.freshSelected > 0
          ? `${fold.total} found (${fold.loadedSelected} loaded, ${fold.freshSelected} older)`
          : `${fold.total} of ${loaded} loaded`)
        : `${matched} of ${loaded} loaded`);
  }

  function refreshSearchCount(): void {
    renderSearchCount(searchField.open() && searchTyped(searchField.query()), searchField.open() && searchField.links(),
      deps.authorChoosing() !== "everyone");
  }

  function sayCardCount(): void {
    const typed = searchField.open() && searchTyped(searchField.query());
    const { cards, shown } = deps.cardCount();
    setTextIfChanged(el("search-count") as never, typed ? `${shown} of ${cards} thread${cards === 1 ? "" : "s"}` : "");
  }

  function pinnedEmptySentence(loaded: number, matched: number, searching: boolean, tally: SearchCount["tally"] | null = null, author: string = "everyone"): string {
    if (loaded > 0) {
      if (tally !== null && tally.linked === 0) return "None of this channel's pinned messages has a link.";
      if (tally !== null && tally.kept === 0) {
        return `None of this channel's pinned messages has a link left once ${deps.hiddenKindNames(tally.everywhere)} ` +
          "are hidden — turn them back on under the Links button.";
      }
      if (matched > 0) return "";
      if (author !== "everyone" && tally === null) return deps.authorEmptySentence(author, searching, true);
      if (!searching) return "";
      if (tally === null) return "None of this channel's pinned messages match that search.";
      return tally.saidLinked > 0
        ? "None of this channel's pinned messages with a link match that search once " +
          `${deps.hiddenKindNames(tally.matching)} are hidden.`
        : "None of this channel's pinned messages with a link match that search.";
    }
    if (deps.pinProblem()) return `This channel's pinned messages could not be loaded: ${deps.pinProblem()}`;
    if (deps.pinRevision() === null || deps.pinChannel() !== deps.currentChannelId()) return "Loading this channel's pinned messages…";
    return "No pinned messages in this channel yet — pin one from its ⋯ menu.";
  }

  function linksEmptySentence(loaded: number, tally: SearchCount["tally"], matched: number, author: string = "everyone"): string {
    if (deps.view() === "discord" && deps.selectionView() === "threads") {
      return "This is a list of threads, and links are in their messages — open a thread to see its links.";
    }
    if (loaded === 0) return "";
    if (tally.linked === 0) return "No links in the messages loaded so far.";
    if (tally.kept === 0) {
      return `No links left once ${deps.hiddenKindNames(tally.everywhere)} are hidden — turn them back on under the Links button.`;
    }
    if (matched > 0) return "";
    if (author !== "everyone" && !searchTyped(searchField.query())) {
      return author === "mine" ? "None of your messages loaded so far has a link." :
        "None of the messages from anyone else loaded so far has a link.";
    }
    return tally.saidLinked > 0
      ? `None of the loaded messages with a link match that search once ${deps.hiddenKindNames(tally.matching)} are hidden.`
      : "None of the loaded messages with a link match that search.";
  }

  function listEmptied(): boolean {
    return !el("search-empty").hidden;
  }

  function hitMark(text: string): HTMLElement {
    const mark = document.createElement("mark");
    mark.className = SEARCH_HIT_CLASS;
    mark.textContent = text;
    return mark;
  }

  function markHitsIn(part: Element, terms: string[]): HTMLElement[] {
    const marks: HTMLElement[] = [];
    for (const run of textRunsOf(part as never) as Array<Array<{ node: Node; own: boolean }>>) {
      const texts = run.map((piece) => String(piece.node.textContent));
      const ranges = textHits(texts.join(""), terms) as Array<[number, number]>;
      if (ranges.length === 0) continue;
      let from = 0;
      let next = 0;
      run.forEach((piece, index) => {
        const text = texts[index] as string;
        const to = from + text.length;
        while (next < ranges.length && (ranges[next] as [number, number])[1] <= from) next += 1;
        const pieces: Node[] = [];
        let cursor = 0;
        for (let at = next; at < ranges.length && (ranges[at] as [number, number])[0] < to; at += 1) {
          const a = Math.max((ranges[at] as [number, number])[0], from) - from;
          const b = Math.min((ranges[at] as [number, number])[1], to) - from;
          if (a > cursor) pieces.push(document.createTextNode(text.slice(cursor, a)));
          const mark = hitMark(text.slice(a, b));
          pieces.push(mark);
          marks.push(mark);
          cursor = b;
        }
        from = to;
        if (pieces.length === 0) return;
        if (cursor < text.length) pieces.push(document.createTextNode(text.slice(cursor)));
        if (piece.own) {
          (piece.node as Element).replaceChildren(...pieces);
          return;
        }
        const parent = piece.node.parentNode as Node;
        for (const made of pieces) parent.insertBefore(made, piece.node);
        parent.removeChild(piece.node);
      });
    }
    return marks;
  }

  function unmarkHits(marks: HTMLElement[]): void {
    const parents = new Set<Node>();
    for (const mark of marks) {
      const parent = mark.parentNode;
      if (!parent) continue;
      parents.add(parent);
      parent.replaceChild(document.createTextNode(String(mark.textContent)), mark);
    }
    for (const parent of parents) {
      const kids = [...parent.childNodes];
      if (kids.every((kid) => kid.nodeType === 3)) {
        const text = kids.map((kid) => kid.textContent).join("");
        (parent as Element).replaceChildren();
        parent.textContent = text;
        continue;
      }
      let joined: Node | null = null;
      for (const kid of kids) {
        if (kid.nodeType !== 3) {
          joined = null;
        } else if (joined === null) {
          joined = kid;
        } else {
          joined.textContent = `${joined.textContent}${kid.textContent}`;
          parent.removeChild(kid);
        }
      }
    }
  }

  function excerptSource(row: SearchRow): string {
    const messages = rowMessages(row);
    if (messages.length) return messages.map((m) => deps.messageText(m)).join("\n");
    return typeof row.linkText === "string" ? row.linkText : "";
  }

  function searchExcerpt(text: string, terms: string[]): HTMLElement | null {
    const hits = textHits(text, terms) as Array<[number, number]>;
    const firstBreak = text.indexOf("\n");
    const shown = Math.min(firstBreak < 0 ? text.length : firstBreak, EXCERPT_SURELY_SHOWN);
    const late = hits.filter(([, end]) => end > shown);
    if (late.length === 0) return null;
    const edge = (at: number, step: number): number => {
      let to = at;
      for (let moved = 0; moved < 12 && to > 0 && to < text.length && !/\s/.test(text[step < 0 ? to - 1 : to] as string); moved += 1) {
        to += step;
      }
      return /\s/.test(text[step < 0 ? to - 1 : to] || " ") ? to : at;
    };
    const places: Array<{ from: number; to: number }> = [];
    for (const [start, end] of late) {
      const from = start - EXCERPT_CONTEXT <= 0 ? 0 : edge(start - EXCERPT_CONTEXT, -1);
      const to = end + EXCERPT_CONTEXT >= text.length ? text.length : edge(end + EXCERPT_CONTEXT, 1);
      const last = places[places.length - 1];
      if (last && from <= last.to) last.to = Math.max(last.to, to);
      else places.push({ from, to });
    }
    const line = document.createElement("div");
    line.className = "search-excerpt";
    const lead = document.createElement("span");
    lead.className = "sr-only";
    lead.textContent = "Further down: ";
    line.append(lead);
    places.slice(0, EXCERPT_PLACES).forEach((place, index) => {
      if (index > 0) line.append(document.createTextNode(" · "));
      const words = text.slice(place.from, place.to).replace(/\s+/g, " ");
      const said = document.createElement("span");
      said.className = "search-excerpt-place";
      said.textContent = `${place.from > 0 ? "…" : ""}${words.trim()}${place.to < text.length ? "…" : ""}`;
      line.append(said);
    });
    const more = places.slice(EXCERPT_PLACES).reduce((sum, place) =>
      sum + hits.filter(([start, end]) => start >= place.from && end <= place.to).length, 0);
    if (more > 0) {
      const tail = document.createElement("span");
      tail.className = "search-excerpt-more";
      tail.textContent = ` +${more} more`;
      line.append(tail);
    }
    for (const said of [...line.children].filter((kid) => classWords(kid as never).includes("search-excerpt-place"))) {
      markHitsIn(said, terms);
    }
    return line;
  }

  function markSearchHits(row: SearchRow, terms: string[]): void {
    const key = terms.join("\u0000");
    const links = row.linkView && row.linkView.node && row.linkView.node.parentNode === row ? row.linkView.node : null;
    const held = row.searchMarks;
    if (held ? held.key === key && held.links === links : key === "") return;
    if (held) {
      unmarkHits(held.marks);
      for (const anchor of held.outlined) setClassWord(anchor as never, SEARCH_HIT_LINK_CLASS, false);
      if (held.author && held.author.hasAttribute("data-search-hit")) held.author.removeAttribute("data-search-hit");
      if (held.excerpt && held.excerpt.parentNode === row) row.removeChild(held.excerpt);
      row.searchMarks = null;
    }
    if (key === "") return;
    const { parts, author } = findableParts(row as never) as { parts: Element[]; author: Element | null };
    const outlined = (anchorsAmong(parts as never) as Element[]).filter((anchor) => {
      const href = String(anchor.getAttribute("href") || "").toLowerCase();
      const words = (anchor.childNodes.length ? renderedText(anchor as HTMLElement) : String(anchor.textContent)).toLowerCase();
      return terms.some((term) => href.includes(term) && !words.includes(term));
    });
    for (const anchor of outlined) setClassWord(anchor as never, SEARCH_HIT_LINK_CLASS, true);
    const marks = parts.flatMap((part) => markHitsIn(part, terms));
    const named = author && terms.some((term) => String(author.textContent).toLowerCase().includes(term)) ? author : null;
    if (named) named.setAttribute("data-search-hit", "");
    const entry = row.foldEntry;
    const excerpt = entry ? searchExcerpt(excerptSource(row), terms) : null;
    if (excerpt && entry) {
      excerpt.hidden = !deps.isFolded(entry as never);
      const after = [...row.children].find((kid) => classWords(kid as never).includes("elided")) || entry.body;
      row.insertBefore(excerpt, after.nextSibling);
    }
    row.searchMarks = { key, links, marks, outlined, author: named, excerpt };
  }

  const rowsChosen = (): boolean => deps.readRunsSuspended();
  const openedGaps = deps.language.openedGaps;

  function openedGapIds(create: boolean): Set<string> | null {
    return openedGaps.ids(deps.searchKey(), JSON.stringify([deps.viewKey(), deps.side(), String(deps.selectedChannelId())]), create) as Set<string> | null;
  }

  const seamLines: SeamLines = createSeamLines({
    gapListener: (row: never, below: boolean, line: never) => deps.inThisColumn((event: Event) => {
      if (event) event.stopPropagation();
      const lineEl = line as unknown as HTMLElement & { gapUnpack?: boolean; gapCards?: boolean };
      const focused = document.activeElement === lineEl && lineEl.matches(":focus-visible");
      const first = lineEl.gapUnpack ? deps.openUnpackGap(row as unknown as Element, below)
        : lineEl.gapCards ? deps.openCardGap(row as unknown as Element, below) : openSearchGap(row as unknown as Element, below);
      if (first) {
        // `#2200` seam landing: what was just opened stands fully in view —
        // through the scroll owner, whose scroll-margin stops it below the
        // floating chip panel, never under it with only its last line
        // showing. (Before: the row was only refocused, preventScroll, so
        // an opened run's first row could sit under the panel.)
        if (deps.intoView) deps.intoView(first, "start");
        if (focused) {
          first.setAttribute("tabindex", "-1");
          first.focus({ preventScroll: true });
        }
      }
    }) as never,
  });
  const { seamDrawn, placeSeam } = seamLines;

  function inOpenedGap(row: Element, opened: Set<string> | null): boolean {
    if (!opened || opened.size === 0) return false;
    const held = idsOf(row);
    return held.length > 0 && held.every((id) => opened.has(id));
  }

  function openSearchGap(row: Element, below: boolean): HTMLElement | null {
    const list = row.parentNode;
    if (!list) return null;
    const rows = [...list.children].filter((one) => isMessageRow(one) && !one.hasAttribute("data-card")) as HTMLElement[];
    const at = rows.indexOf(row as HTMLElement);
    if (at < 0) return null;
    const opened = openedGapIds(true);
    if (!opened) return null;
    let first: HTMLElement | null = null;
    const take = (one: HTMLElement): void => {
      for (const id of idsOf(one)) opened.add(id);
      if (first === null || !below) first = one;
    };
    if (below) {
      for (let next = at + 1; next < rows.length && !rowShown(rows[next] as HTMLElement); next += 1) take(rows[next] as HTMLElement);
    } else {
      for (let next = at - 1; next >= 0 && !rowShown(rows[next] as HTMLElement); next -= 1) take(rows[next] as HTMLElement);
    }
    if (first === null) return null;
    if (deps.screen() === "main" && deps.view() === "discord") deps.preservingScroll(applySearch);
    else applySearch();
    return first;
  }

  function rowDay(row: SearchRow): number | null {
    const gathered = row.getAttribute("data-coalesce");
    if ((gathered !== null && gathered !== "parent") || row.hasAttribute("data-stack-under")) return null;
    const sent = rowSentMs(row);
    return Number.isFinite(sent) ? dayKeyOf(new Date(sent)) : null;
  }

  interface Seam { kind: string; words: string }

  function seamsFor(list: HTMLElement): { rows: SearchRow[]; wanted: Map<SearchRow, { above: Seam | null; below: Seam | null }>; runDays: Map<SearchRow, string>; today: number } {
    // `#1786`: server-placed rows open no gaps of their own and count in none.
    const rows = ([...list.children] as SearchRow[]).filter((row) => isMessageRow(row) && !row.hasAttribute("data-card") && !row.hasAttribute("data-server-hit"));
    const gaps = rowsChosen();
    const clock = dayClock();
    const plan = seamPlan(rows.map((row) => ({
      role: row.getAttribute("data-read-run"), shown: rowShown(row), day: rowDay(row), count: idsOf(row).length,
    })) as never, gaps, clock as never) as { wanted: Map<number, { above: Seam | null; below: Seam | null }>; runDays: Map<number, string> };
    const wanted = new Map<SearchRow, { above: Seam | null; below: Seam | null }>();
    for (const [at, lines] of plan.wanted) wanted.set(rows[at] as SearchRow, lines);
    const runDays = new Map<SearchRow, string>();
    for (const [at, days] of plan.runDays) runDays.set(rows[at] as SearchRow, days);
    return { rows, wanted, runDays, today: (clock as { today: number }).today };
  }

  const seamsDrawnOn = new WeakMap<object, number>();
  const timesWordedOn = new WeakMap<object, string>();

  function rewordRowTimes(list: HTMLElement, rows: SearchRow[], today: number): void {
    // A row's words are reworded when the day turns, and when the device's zone moves under
    // them (`#1229 keyed-row-patching`: a kept row is not redrawn, so its time would otherwise
    // keep the zone it was drawn in).
    const wording = `${today}|${new Date().getTimezoneOffset()}`;
    const was = timesWordedOn.get(list);
    timesWordedOn.set(list, wording);
    if (was === undefined || was === wording) return;
    for (const row of rows) {
      const node = rowTime(row as never) as HTMLElement | null;
      if (node) setTextIfChanged(node as never, messageTimeLabel({ timestamp: node.getAttribute("datetime") } as never));
    }
  }

  function drawSeams(list: HTMLElement): void {
    const { rows, wanted, runDays, today } = seamsFor(list);
    seamsDrawnOn.set(list, today);
    rewordRowTimes(list, rows, today);
    const none = { above: null, below: null };
    const changed = rows.some((row) => {
      const want = wanted.get(row) || none;
      return !seamDrawn(row as never, want.above as never, false) || !seamDrawn(row as never, want.below as never, true) ||
        (runDays.has(row) && deps.runDaysOf(row) !== runDays.get(row));
    });
    if (!changed) return;
    const apply = (): void => {
      for (const row of rows) {
        const want = wanted.get(row) || none;
        placeSeam(row as never, want.above as never, false);
        placeSeam(row as never, want.below as never, true);
        if (runDays.has(row) && deps.runDaysOf(row) !== runDays.get(row)) {
          row.runDays = runDays.get(row);
          deps.placeInReadRun(row, "head", deps.readRunCount(row));
        }
      }
    };
    if (!deps.side() && !deps.changing() && deps.screen() === "main" && deps.view() === "discord" && list === deps.visibleList()) {
      const held = deps.currentFilterHold();
      if (held !== null) {
        apply();
        if (held.row !== null) deps.keepFilterHold();
      } else {
        deps.preservingScroll(apply);
      }
    } else {
      apply();
    }
  }

  function seamsAfterMidnight(): void {
    const today = (dayClock() as { today: number }).today;
    for (const list of [el("discord-log"), el("pinned-log")]) {
      const drawn = seamsDrawnOn.get(list);
      if (drawn !== undefined && drawn !== today) drawSeams(list);
    }
  }

  return {
    markSearchSoon, markSearchNow, searchParsed, searchTyped, searchIdentity, searchable,
    applySearch, applySearchFast, searchInAll, renderSearchInAll, renderSearchCount, refreshSearchCount,
    listEmptied, drawSeams, seamsAfterMidnight, openSearchGap, inOpenedGap,
    rowsChosen, rowDay, markSearchHits, messageMatches, threadsWithHits, rowMatches,
    linkTerms, threadsWithLinkHits, threadsWithLinks, rowLinkMatch, rowHasLink,
    setEntireThreads: (on: boolean) => { entireThreadsOn = on; },
    entireThreads: () => entireThreadsOn,
    sayCardCount, openedGaps,
    placeSeam: (row: Element, seam: unknown, below: boolean) => placeSeam(row as never, seam as never, below),
    seamDrawn: (row: Element, seam: unknown, below: boolean) => seamDrawn(row as never, seam as never, below),
  };
}

// The thread selector and older threads (`#624 voice-page-modularity`, moved from the legacy
// script into view/channel): what threads the store knows and what they are called, the view
// picker and heading, opening and closing a thread, the walk back through older threads, the
// thread cards, and a row's thread decoration. Why each behaves as it does is told where the
// legacy script held them. Changing the view and the selection stay the page's, injected; the
// unanswered-thread group stays with the page until the reply-counts move.

import type { Message } from "../../contract/index.ts";
import { namedRecord, threadOf } from "../../core/channels/index.ts";
import { firstLineOf, threadChoice, threadFacts } from "../../present/rows/index.ts";
import type { ThreadChoice } from "../../present/rows/index.ts";


/** How long a background read of one channel's thread list is reused by the selector. */
export const THREAD_DIRECTORY_REFRESH_MS = 60000;
/** How many threads the selector lists, newest activity first. */
export const THREAD_SELECT_LIMIT = 8;
/** Longest name the selector shows, in characters. */
export const THREAD_TITLE_CHARS = 28;

/** What the thread list asks of the page. */
export interface ThreadListDependencies {
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  inColumn<T>(column: unknown, fn: () => T): T;
  side(): boolean;
  threading(): boolean;
  currentCanon(): { channel: string; scope: string; threads: Array<{ id: unknown }>; messages: Message[]; views: Map<string, { more: boolean; cursor: string | null; floor: number | null; live?: boolean; newest?: string | null; fullAt?: number }> };
  selectedChannelId(): string;
  selection(): { view: string; threadId: unknown; thread: unknown; origin: string };
  pendingThreadTimes(): Map<string, number>;
  messageText(message: Message): string;
  timeOf(message: Message): number;
  sideColumnUp(): boolean;
  renderSideColumn(): void;
  threadListOffered(): boolean;
  layoutFit(): boolean;
  layoutColumns(): boolean;
  layoutThreadList(): boolean;
  changeChannelView(view: string, id?: string, summary?: unknown): unknown;
  defaultChannelView(): string;
  chooseView(view: string): void;
  openThreadFromList(id: string, summary: unknown): unknown;
  selectSideThread(id: string, summary: unknown): unknown;
  closeSideThread(): unknown;
  setLayout(name: string, on: boolean): unknown;
  paintCard(row: HTMLElement, choice: ThreadChoice, tally: unknown): void;
  fetchTimeline(path: string): Promise<unknown>;
  foldTimelineDelta(payload: unknown, sent: string, view: string, thread: unknown): boolean;
  foldTimelinePage(payload: unknown, older: boolean, view: string, thread: unknown): void;
  isCursorRefusal(error: unknown): boolean;
  refreshFailed(payload: unknown): boolean;
  saveChannelScope(): void;
  syncUnreadReplies(): void;
  renderSideFreshness(): void;
  sideCardReplies(choice: ThreadChoice, tally: unknown): string;
  sideCardUnread(choice: ThreadChoice, tally: unknown): string;
  sideCardRepliesText?: never;
  threadIsRead(choice: ThreadChoice, unread: string): boolean;
  cardOpener(prefix: string, replies: string, unread: string, open: () => void): HTMLElement;
  toggleLitCard(id: string, row: HTMLElement): void;
  paintLitCard(row: HTMLElement, id: string): void;
  enterCard(id: string, summary: unknown, row: HTMLElement): unknown;
  cardMessageCount(choice: ThreadChoice, tally: unknown): { messages: number; exact: boolean };
  searchable(row: Element, ...parts: unknown[]): void;
  threadHue(id: unknown): string;
  threadReplyCount(id: string, message: Message): number;
  threadCount(count: number | null, exact: boolean): string;
  repliesOf(message: Message): { count: number; exact: boolean };
  paintThreadBadge(badge: HTMLElement, id: string, replies: number, unanswered: boolean): void;
  unansweredRoot(message: Message, reported: number, held: number, exact: unknown): boolean;
  startThread(id: string): unknown;
  openReply(messages: Message[], row: HTMLElement): unknown;
  sayRowShortcut(el: Element, name: string): void;
  guardQuietly<T>(fn: () => T): (...args: unknown[]) => Promise<unknown>;
  toggleGathered(id: string, row: HTMLElement): unknown;
  rootOverflowsCard(root: Message | null | undefined): boolean;
  threadsViewKey(): string;
  setTextIfChanged(el: HTMLElement, text: string): void;
  mainColumn(): unknown;
  replyTally(): unknown;
  pageLimit: number;
  currentComplete(): boolean;
  reconcileMs(): number;
  withPendingThreads(summaries: readonly unknown[]): unknown[];
  cardView(choice: ThreadChoice): { mode: string; text: string };
  openReplyFromRow(row: HTMLElement): unknown;
  threadReplies(id: string, message: Message): { count: number | null; exact: boolean };
}

/** The thread list's commands and questions. */
export interface ThreadList {
  noteThreadListLanded(channel: string): void;
  threadWords(choice: ThreadChoice): { named: string; said: string };
  threadName(choice: ThreadChoice | null, chars?: number): string;
  threadChoices(): ThreadChoice[];
  openThreadChoice(): ThreadChoice | null;
  knownThreadChoice(id: string, summary: unknown): ThreadChoice;
  openThreadName(): string;
  olderThreadsOnServer(): boolean;
  renderThreadSelect(): void;
  renderThreadHeading(): unknown;
  onThreadSelect(): unknown;
  refreshThreadDirectory(): Promise<void>;
  openThread(id: unknown, summary?: unknown): unknown;
  closeThread(): unknown;
  olderThreadsToRead(): boolean;
  loadOlderThreads(): Promise<void>;
  olderThreadsFailure(): string;
  threadCards(summaries: readonly unknown[]): HTMLElement[];
  addThreadDecoration(meta: HTMLElement, message: Message, row: HTMLElement, everyThread?: boolean): void;
  gatherCount(id: string, message: Message, held?: number): { count: number | null; exact: boolean };
  threadPagesLoading(): boolean;
  noteNewestThreadPage(channel: string, payload: { has_more?: boolean; next_before?: string }): void;
  threadListFailure(channel: string): string;
  readonly directoryReadAt: Map<string, number>;
  threadListLandedAt(channel: string): number | undefined;
  forgetDirectoryRead(channel: string): void;
}

/** No thread list read, no walk back under way. */
export function createThreadList(deps: ThreadListDependencies): ThreadList {
  const el = deps.byId;
  const leftEl = deps.leftById;

  const threadDirectoryReadAt = new Map<string, number>();
  const threadDirectoryInFlight = (() => {
    let current = "";
    return {
      isFor: (channel: string): boolean => current === channel,
      begin: (channel: string): void => { current = channel; },
      end: (channel: string): void => { if (current === channel) current = ""; },
    };
  })();
  const threadListFailures = new Map<string, string>();
  const threadListLandedAt = new Map<string, number>();

  function noteThreadListLanded(channel: string): void {
    threadListFailures.delete(channel);
    threadListLandedAt.set(channel, Date.now());
  }

  function threadWords(choice: ThreadChoice): { named: string; said: string } {
    let named = "";
    const fromRoot = choice && choice.named && rootNamesShown(choice) ? choice.named : null;
    for (const source of [choice && choice.summary, fromRoot] as Array<{ display_name?: unknown; summary?: unknown } | null>) {
      const name = source && typeof source.display_name === "string" ? source.display_name.trim() : "";
      const said = source && typeof source.summary === "string" ? source.summary.trim() : "";
      if (name && said) return { named: name, said };
      if (!named) named = name;
    }
    return { named, said: "" };
  }

  function rootNamesShown(choice: ThreadChoice): boolean {
    const record = choice.named as { reply_count_exact?: boolean; reply_count?: number } | null;
    if (record && record.reply_count_exact === false) return true;
    const replies = Math.max(typeof choice.count === "number" ? choice.count : 0,
      record && typeof record.reply_count === "number" ? record.reply_count : 0);
    return replies + 1 >= (deps.rootOverflowsCard(choice.root) ? 2 : 3);
  }

  function threadName(choice: ThreadChoice | null, chars: number = THREAD_TITLE_CHARS): string {
    const summary = choice ? choice.summary as { title?: unknown } | null : null;
    const named = choice ? threadWords(choice).named : "";
    const title = summary && typeof summary.title === "string" && summary.title.trim() !== "Thread"
      ? summary.title : "";
    for (const source of [named, title, choice && choice.root ? deps.messageText(choice.root) : ""]) {
      const name = firstLineOf(source as string, chars);
      if (name) return name;
    }
    return "Thread";
  }

  function threadLabel(choice: ThreadChoice, chars: number = THREAD_TITLE_CHARS): string {
    return [threadFacts(choice as never) as string, threadName(choice, chars)].filter((part) => part !== "").join(" · ");
  }

  function threadIndex(): Map<string, ThreadChoice> {
    const byId = new Map<string, ThreadChoice>();
    if (!deps.threading() || deps.currentCanon().channel !== String(deps.selectedChannelId())) return byId;
    const pending = deps.pendingThreadTimes();
    const answered = new Set(deps.currentCanon().messages.filter((held: Message) => {
      const thread = (held as { thread?: { is_root?: boolean; id?: unknown } }).thread;
      return threadOf(held as never) && thread && thread.is_root !== true;
    }).map((held: Message) => String(((held as { thread?: { id?: unknown } }).thread as { id?: unknown }).id)));
    for (const summary of deps.currentCanon().threads) {
      const id = String((summary as { id: unknown }).id);
      if (id !== deps.currentCanon().scope) byId.set(id, threadChoice(id, summary as never));
    }
    for (const message of deps.currentCanon().messages) {
      const id = threadOf(message as never);
      if (!id || id === deps.currentCanon().scope) continue;
      const at = deps.timeOf(message);
      const thread = (message as { thread?: { is_root?: boolean; reply_count?: number } }).thread;
      const root = thread && thread.is_root ? message : null;
      const known = byId.get(id);
      if (!known) {
        if (root && (thread?.reply_count !== 0 || pending.has(id) || answered.has(id))) {
          byId.set(id, threadChoice(id, null, root));
        }
        continue;
      }
      if (Number.isFinite(at)) known.at = Number.isFinite(known.at) ? Math.max(known.at, at) : at;
      if (root) known.root = root;
      if (namedRecord(root as never)) known.named = (root as Message).thread as never;
      if (root && !Number.isFinite(known.started)) known.started = at;
    }
    for (const [id, at] of pending) {
      const known = byId.get(id);
      if (known) known.at = Number.isFinite(known.at) ? Math.max(known.at, at) : at;
    }
    return byId;
  }

  function threadChoices(): ThreadChoice[] {
    const when = (choice: ThreadChoice): number => (Number.isFinite(choice.at) ? choice.at : 0);
    return [...threadIndex().values()].sort((a, b) => when(b) - when(a));
  }

  function openThreadChoice(): ThreadChoice | null {
    if (deps.selection().view !== "thread" || !deps.selection().threadId) return null;
    return knownThreadChoice(String(deps.selection().threadId), deps.selection().thread);
  }

  function knownThreadChoice(id: string, summary: unknown): ThreadChoice {
    const listed = threadIndex().get(id);
    if (listed) return listed;
    const root = deps.currentCanon().channel === String(deps.selectedChannelId())
      ? deps.currentCanon().messages.find((message) => {
          const thread = (message as { thread?: { is_root?: boolean } }).thread;
          return threadOf(message as never) === id && thread && thread.is_root === true;
        }) : null;
    return threadChoice(id, summary as never, root || null);
  }

  function openThreadName(): string {
    const open = openThreadChoice();
    const name = open ? threadName(open) : "Thread";
    return name === "Thread" ? "this thread" : name;
  }

  function olderThreadsOnServer(): boolean {
    if (deps.currentCanon().channel !== String(deps.selectedChannelId())) return false;
    const cover = deps.currentCanon().views.get(deps.threadsViewKey());
    return Boolean(cover && cover.more);
  }

  const VIEW_CHOICES: ReadonlyArray<[string, string]> = [["main", "Main"], ["flat", "All"], ["threads", "Thread list"], ["columns", "Two columns"]];

  function viewChoiceOnScreen(): string {
    if (deps.layoutColumns()) return "columns";
    if (deps.layoutThreadList()) return "threads";
    if (deps.selection().view === "thread" && deps.selection().threadId) return `thread:${deps.selection().threadId}`;
    return deps.selection().view === "flat" ? "flat" : "main";
  }

  function renderThreadSelect(): void {
    const select = el("thread-select") as HTMLSelectElement;
    const options: Array<[string, string]> = [["main", "Main"]];
    if (deps.threading()) {
      options.push(["flat", "All"]);
      if (deps.threadListOffered()) {
        options.push(["threads", "Thread list"]);
        if (deps.layoutFit()) options.push(["columns", "Two columns"]);
      }
      const choices = threadChoices();
      const shown = choices.slice(0, THREAD_SELECT_LIMIT);
      for (const choice of shown) options.push([`thread:${choice.id}`, threadLabel(choice)]);
      const open = openThreadChoice();
      if (open && !shown.some((choice) => choice.id === open.id)) {
        options.push([`thread:${open.id}`, threadLabel(open)]);
      }
    }
    const current = viewChoiceOnScreen();
    const signature = JSON.stringify(options);
    if (select.getAttribute("data-options") !== signature) {
      select.replaceChildren(...options.map(([value, label]) => {
        const option = document.createElement("option");
        option.value = value;
        option.textContent = label;
        return option;
      }));
      select.setAttribute("data-options", signature);
    }
    select.value = current;
    select.disabled = !deps.threading();
    const why = deps.threading()
      ? "Which view of the channel to read: Main, All, the thread list, two columns, or one thread."
      : "This channel's chat service does not offer threads here, so Main is the whole channel.";
    const shownThread = current.startsWith("thread:") ? openThreadChoice() : null;
    const title = shownThread ? `${threadName(shownThread, Infinity)}\n${why}` : why;
    if (select.title !== title) select.title = title;
    (deps.setTextIfChanged as (el: HTMLElement, text: string) => void)(leftEl("thread-select-note"), deps.threading() ? "" : why);
    if (deps.sideColumnUp()) deps.renderSideColumn();
  }

  function renderThreadHeading(): unknown {
    if (deps.side()) return deps.inColumn(deps.mainColumn(), renderThreadHeading);
    if (deps.selection().view !== "thread") return undefined;
    deps.paintCard(leftEl("thread-card"), openThreadChoice() as ThreadChoice, deps.replyTally());
    const back = leftEl("thread-back");
    const says = threadBackTitle();
    if (back.getAttribute("title") !== says) back.setAttribute("title", says);
    return undefined;
  }

  function threadBackTitle(): string {
    const to = deps.selection().origin === "list" && !deps.threadListOffered() ? deps.defaultChannelView() : deps.selection().origin;
    if (to === "list") return "Back to the thread list";
    if (to === "flat") return "Back to All";
    if (to === "threads") return "Back to the Threads list";
    return "Back to Main";
  }

  function onThreadSelect(): unknown {
    const value = String((el("thread-select") as HTMLSelectElement).value);
    if (VIEW_CHOICES.some(([view]) => view === value)) {
      deps.chooseView(value);
      return undefined;
    }
    if (!value.startsWith("thread:")) return undefined;
    const id = value.slice("thread:".length);
    if (!deps.layoutColumns() && !deps.layoutThreadList() && deps.selection().view === "thread" && String(deps.selection().threadId) === id) return undefined;
    const summary = deps.currentCanon().threads.find((held) => String((held as { id: unknown }).id) === id) || null;
    if (deps.layoutThreadList()) return deps.openThreadFromList(id, summary);
    const opened = openThread(id, summary);
    renderThreadSelect();
    return opened;
  }

  async function refreshThreadDirectory(): Promise<void> {
    const channel = String(deps.selectedChannelId());
    if (!deps.threading() || !channel || deps.currentCanon().scope) return;
    if (threadDirectoryInFlight.isFor(channel)) return;
    if (Date.now() - (threadDirectoryReadAt.get(channel) || 0) < THREAD_DIRECTORY_REFRESH_MS) return;
    threadDirectoryInFlight.begin(channel);
    threadDirectoryReadAt.set(channel, Date.now());
    const base = `/api/v1/channels/${encodeURIComponent(channel)}/timeline?view=threads&limit=${deps.pageLimit}` +
      "&reason=side";
    const cover = deps.currentCanon().channel === channel ? deps.currentCanon().views.get(deps.threadsViewKey()) : null;
    let sent = cover && cover.live && cover.newest && deps.currentComplete() !== false &&
      Date.now() - (cover.fullAt || 0) < deps.reconcileMs() ? cover.newest : null;
    try {
      let payload: unknown;
      try {
        payload = await deps.fetchTimeline(sent ? `${base}&after=${encodeURIComponent(sent as string)}` : base);
      } catch (error) {
        if (!sent || !deps.isCursorRefusal(error)) throw error;
        if (cover && cover.newest === sent) cover.newest = null;
        sent = null;
        payload = await deps.fetchTimeline(base);
      }
      if (String(deps.selectedChannelId()) !== channel) return;
      if (sent && (payload as { delta?: boolean }).delta) {
        if (!deps.foldTimelineDelta(payload, sent as string, "threads", null)) return;
      } else {
        deps.foldTimelinePage(payload, false, "threads", null);
        if (!deps.refreshFailed(payload) || (payload as { next_before?: string }).next_before) noteNewestThreadPage(channel, payload as { has_more?: boolean; next_before?: string });
      }
      noteThreadListLanded(channel);
      deps.saveChannelScope();
      renderThreadSelect();
      renderThreadHeading();
      deps.syncUnreadReplies();
    } catch (_error) {
      threadDirectoryReadAt.delete(channel);
      if (String(deps.selectedChannelId()) === channel) {
        threadListFailures.set(channel, (_error as { network?: boolean })?.network ? "offline" : "failed");
        deps.renderSideFreshness();
      }
    } finally {
      threadDirectoryInFlight.end(channel);
    }
  }

  function openThread(id: unknown, summary: unknown = null): unknown {
    if (!deps.threading() || !id) return;
    if (deps.layoutColumns()) return deps.selectSideThread(String(id), summary);
    if (deps.layoutThreadList()) return deps.openThreadFromList(String(id), summary);
    return deps.changeChannelView("thread", String(id), summary);
  }

  function closeThread(): unknown {
    if (deps.side()) return deps.layoutColumns() ? deps.closeSideThread() : undefined;
    if (deps.selection().view !== "thread") return;
    if (deps.selection().origin === "list") {
      if (deps.threadListOffered()) return deps.setLayout("list", true);
      return deps.changeChannelView(deps.defaultChannelView());
    }
    return deps.changeChannelView(deps.selection().origin);
  }

  interface ThreadPages { channel: string; cursor: string | null; loading: boolean; failed: boolean; error: string; generation: number }
  const threadPages: ThreadPages = { channel: "", cursor: null, loading: false, failed: false, error: "", generation: 0 };

  function threadPagesFor(channel: string): ThreadPages {
    if (threadPages.channel !== channel) {
      Object.assign(threadPages, { channel, cursor: null, loading: false, failed: false, error: "" });
      threadPages.generation += 1;
    }
    return threadPages;
  }

  function noteNewestThreadPage(channel: string, payload: { has_more?: boolean; next_before?: string }): void {
    const pages = threadPagesFor(channel);
    const next = payload.has_more === true ? payload.next_before || null : null;
    if (!pages.cursor || !next) pages.cursor = next;
  }

  function olderThreadsToRead(): boolean {
    const channel = String(deps.selectedChannelId());
    return olderThreadsOnServer() || (threadPages.channel === channel && threadPages.cursor !== null);
  }

  async function loadOlderThreads(): Promise<void> {
    const channel = String(deps.selectedChannelId());
    if (!deps.threading() || !channel || deps.currentCanon().scope) return;
    const pages = threadPagesFor(channel);
    if (pages.loading) return;
    const generation = ++pages.generation;
    Object.assign(pages, { loading: true, failed: false, error: "" });
    deps.renderSideColumn();
    const base = `/api/v1/channels/${encodeURIComponent(channel)}/timeline?view=threads&limit=${deps.pageLimit}`;
    const stale = (): boolean => generation !== pages.generation || String(deps.selectedChannelId()) !== channel;
    try {
      if (!pages.cursor) {
        const newest = await deps.fetchTimeline(`${base}&reason=threads`) as { has_more?: boolean; next_before?: string };
        if (stale()) return;
        deps.foldTimelinePage(newest, false, "threads", null);
        threadDirectoryReadAt.set(channel, Date.now());
        noteThreadListLanded(channel);
        noteNewestThreadPage(channel, newest);
      }
      if (pages.cursor) {
        const older = await deps.fetchTimeline(`${base}&reason=older&before=${encodeURIComponent(pages.cursor)}`) as { has_more?: boolean; next_before?: string };
        if (stale()) return;
        deps.foldTimelinePage(older, true, "threads", null);
        pages.cursor = older.has_more === true ? older.next_before || null : null;
      }
    } catch (error) {
      if (stale()) return;
      pages.failed = true;
      pages.error = ((error as { detail?: string; message?: string })?.detail || (error as { message?: string })?.message) || "the server did not answer";
    } finally {
      if (!stale()) pages.loading = false;
    }
    if (stale()) return;
    renderThreadSelect();
    renderThreadHeading();
    deps.renderSideColumn();
  }

  function olderThreadsFailure(): string {
    if (threadPages.channel !== String(deps.selectedChannelId()) || !threadPages.failed) return "";
    return `Could not load older threads: ${threadPages.error.replace(/[.\s]+$/, "")}.`;
  }

  function threadCards(summaries: readonly unknown[]): HTMLElement[] {
    const tally = deps.replyTally();
    return deps.withPendingThreads(summaries).map((summary: unknown) => threadCard(summary, tally));
  }

  function threadCard(summary: unknown, tally: unknown): HTMLElement {
    const held = summary as { id: unknown };
    const id = String(held.id);
    const choice = threadChoice(id, summary as never);
    const row = document.createElement("li");
    row.className = "thread-card";
    row.setAttribute("data-context-id", id);
    row.style.setProperty("--thread-hue", deps.threadHue(held.id));
    const button = document.createElement("button");
    button.className = "thread-open";
    button.setAttribute("type", "button");
    const title = document.createElement("strong");
    title.textContent = threadName(choice, Infinity);
    const said = threadWords(choice).said;
    const view = deps.cardView(choice);
    const first = view.mode === "name" ? "" : view.text;
    const about = document.createElement("span");
    about.className = "thread-card-text";
    about.textContent = said || (first.replace(/\s+/g, " ") === title.textContent ? "" : first);
    button.append(title, about);
    button.addEventListener("click", () => deps.toggleLitCard(id, row));
    const unread = deps.sideCardUnread(choice, tally);
    if (deps.threadIsRead(choice, unread)) row.setAttribute("data-thread-read", "");
    const open = deps.cardOpener("thread-card", deps.sideCardReplies(choice, tally), unread,
      deps.inThisColumn(() => deps.guardQuietly(() => deps.enterCard(id, summary, row))()));
    row.append(button, open);
    const size = deps.cardMessageCount(choice, tally);
    (row as { cardMessages?: number }).cardMessages = size.messages;
    (row as { cardMessagesExact?: boolean }).cardMessagesExact = size.exact;
    deps.paintLitCard(row, id);
    deps.searchable(row, title.textContent || "");
    return row;
  }

  function addThreadDecoration(meta: HTMLElement, message: Message, row: HTMLElement, everyThread: boolean = false): void {
    const id = threadOf(message as never);
    if (!deps.threading() || !id || (deps.selection().view === "thread" && !everyThread) || id === deps.currentCanon().scope) return;
    const badge = document.createElement("button");
    badge.className = "thread-badge";
    badge.setAttribute("type", "button");
    const replies = deps.threadReplyCount(id, message);
    deps.paintThreadBadge(badge, id, replies, deps.unansweredRoot(message, replies, 0, null));
    badge.addEventListener("click", deps.inThisColumn(() => deps.guardQuietly(() => {
      if (row.hasAttribute("data-main-row")) return deps.openReplyFromRow(row);
      return badge.getAttribute("data-thread-state") === "start" ? deps.startThread(id) : openThread(id);
    })()));
    deps.sayRowShortcut(badge, "thread-badge");
    meta.append(badge);
    (row as { threadBadge?: HTMLElement }).threadBadge = badge;
    const thread = (message as { thread?: { is_root?: boolean } }).thread;
    if (thread && thread.is_root) {
      const { count, exact } = gatherCount(id, message);
      if (count === 0) return;
      const gather = document.createElement("button");
      gather.className = "thread-replies";
      gather.setAttribute("type", "button");
      gather.setAttribute("title", "Gather this message's replies under it");
      gather.setAttribute("aria-pressed", "false");
      gather.style.setProperty("--thread-hue", deps.threadHue(id));
      gather.textContent = deps.threadCount(count, exact);
      gather.addEventListener("click", deps.inThisColumn(() => deps.guardQuietly(() => deps.toggleGathered(String(message.id), row))()));
      row.append(gather);
    }
  }

  function gatherCount(id: string, message: Message, held: number = 0): { count: number | null; exact: boolean } {
    const own = (message as { thread?: { reply_count?: number; reply_count_exact?: boolean } }).thread as { reply_count?: number; reply_count_exact?: boolean };
    const counted = typeof own.reply_count === "number" && own.reply_count > 0
      ? { count: own.reply_count, exact: own.reply_count_exact as boolean }
      : deps.threadReplies(id, message);
    const count = counted.count;
    const exact = counted.exact;
    return typeof count === "number" ? { count: Math.max(count as number, held), exact } : { count: held || null, exact: true };
  }

  return {
    noteThreadListLanded, threadWords, threadName, threadChoices, openThreadChoice,
    knownThreadChoice, openThreadName, olderThreadsOnServer, renderThreadSelect,
    renderThreadHeading, onThreadSelect, refreshThreadDirectory, openThread, closeThread,
    olderThreadsToRead, loadOlderThreads, olderThreadsFailure, threadCards, addThreadDecoration, gatherCount,
    threadPagesLoading: () => threadPages.loading && threadPages.channel === String(deps.selectedChannelId()),
    noteNewestThreadPage,
    forgetDirectoryRead: (channel: string) => { threadDirectoryReadAt.delete(channel); },
    directoryReadAt: threadDirectoryReadAt,
    threadListFailure: (channel: string) => threadListFailures.get(channel) || "",
    threadListLandedAt: (channel: string) => threadListLandedAt.get(channel),
  };
}

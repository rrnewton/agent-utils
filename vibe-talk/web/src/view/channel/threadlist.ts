// The thread selector and older threads (`#624 voice-page-modularity`, moved from the legacy
// script into view/channel): what threads the store knows and what they are called, the view
// picker and heading, opening and closing a thread, the walk back through older threads, the
// thread cards, and a row's thread decoration. Why each behaves as it does is told where the
// legacy script held them. Changing the view and the selection stay the page's, injected; the
// unanswered-thread group stays with the page until the reply-counts move.

import type { Message } from "../../contract/index.ts";
import { namedRecord, threadOf } from "../../core/channels/index.ts";
import { filedThreadSlug, firstLineOf, threadChoice, threadFacts, titleLineOf } from "../../present/rows/index.ts";
import type { ThreadChoice } from "../../present/rows/index.ts";
import { noteRowBuilt } from "./rows.ts";


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
  changeChannelView(view: string, id?: string, summary?: unknown, origin?: string | null, before?: unknown, drill?: { id: string; top: number } | null): unknown;
  defaultChannelView(): string;
  chooseView(view: string): void;
  /** `#1608`: open the Transcripts view (the picker's Transcripts entry). */
  openTranscripts?(): void;
  /** `#1608`: whether the Transcripts view is the one on screen. */
  transcriptsOpen?(): boolean;
  /** `#1608`: leave the Transcripts view for another picker choice. */
  closeTranscripts?(): void;
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
  paintThreadBadge(badge: HTMLElement, id: string, replies: number, unanswered: boolean, slug?: string): void;
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
  /** The channel's explicit thread-pin records (`#1550 thread-list-pins`), as the server lists them. Optional: without it the list reads and writes through the server API itself, with the page's kept token. */
  readThreadPins?(channel: string): Promise<{ records: Array<{ thread_id: string; pinned: boolean }> }>;
  /** Set one thread's pin on the server (`#1550 thread-list-pins`). Optional, as above. */
  setThreadPin?(channel: string, threadId: string, pinned: boolean): Promise<unknown>;
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
  openThread(id: unknown, summary?: unknown, drill?: { id: string; top: number } | null): unknown;
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

  /** The slug a picker row leads with: `@agent` for a bang-bound thread, else its filed name. */
  function threadSlug(choice: ThreadChoice | null): string {
    if (!choice) return "";
    const agent = bangAgent(choice);
    if (agent) return `@${agent}`;
    return threadWords(choice).named;
  }

  /** The readable title beside the slug: the server's title, else the root's title line
   * (skipping a leading quote block, `#1814`; 80 at most). */
  function threadTitle(choice: ThreadChoice | null): string {
    if (!choice) return "";
    const summary = choice.summary as { title?: unknown } | null;
    const titled = summary && typeof summary.title === "string" && summary.title.trim() !== "Thread"
      ? summary.title : "";
    const slug = threadSlug(choice);
    if (titled) {
      const title = firstLineOf(titled, Infinity);
      if (title && title !== slug) return title;
    }
    const title = titleLineOf(choice.root ? deps.messageText(choice.root) : "");
    return title && title !== slug ? title : "";
  }

  function threadName(choice: ThreadChoice | null, chars: number = THREAD_TITLE_CHARS): string {
    if (!choice) return "Thread";
    const slug = threadSlug(choice);
    const title = threadTitle(choice);
    // Full form (`Infinity`, the picker pop-up and headings): slug plus title, the
    // title alone while unnamed — never a 28-character prefix (`#1563`).
    if (!Number.isFinite(chars) || chars >= 80) {
      if (slug && title) return `${slug} · ${title}`;
      return slug || title || "Thread";
    }
    for (const source of [slug, title]) {
      const name = firstLineOf(source, chars);
      if (name) return name;
    }
    return "Thread";
  }

  function threadLabel(choice: ThreadChoice, chars: number = THREAD_TITLE_CHARS): string {
    const name = threadName(choice, chars);
    return [threadFacts(choice as never) as string, name].filter((part) => part !== "").join(" · ");
  }

  // Thread pins (`#1550 thread-list-pins`). The server's store is the truth:
  // explicit records come from `GET .../thread-pins`; a bang-bound thread
  // (root `!@NAME`, recorded by the bridge in `thread-bindings.json`, #917)
  // is pinned by default, and a record with `pinned: false` is its unpin,
  // which sticks. Never localStorage as the source of truth, never Slack.
  let pinRecords = new Map<string, boolean>();
  let pinChannel = "";
  let pinLoadInFlight = "";

  /** The agent thread `choice` is bang-bound to, from its root's `!@NAME`. */
  function bangAgent(choice: ThreadChoice | null): string | null {
    const content = choice && choice.root ? String((choice.root as { content?: unknown }).content || "") : "";
    const first = content.split(/\s+/).find((word) => word !== "") || "";
    const name = first.startsWith("!@") ? first.slice(2) : "";
    const valid = name !== "" && name.length <= 32 && /^[a-z]$/.test(name[0] as string)
      && [...name].every((ch) => /[a-z0-9-]/.test(ch));
    return valid ? name : null;
  }

  /** Whether `choice` is pinned: its explicit record, else its bang default. */
  function threadIsPinned(choice: ThreadChoice): boolean {
    const explicit = pinRecords.get(choice.id);
    if (explicit !== undefined) return explicit;
    return bangAgent(choice) !== null;
  }

  async function readThreadPins(channel: string): Promise<{ records: Array<{ thread_id: string; pinned: boolean }> }> {
    if (!deps.readThreadPins) return { records: [] };
    return deps.readThreadPins(channel);
  }

  async function writeThreadPin(channel: string, threadId: string, pinned: boolean): Promise<void> {
    if (!deps.setThreadPin) return;
    await deps.setThreadPin(channel, threadId, pinned);
  }

  /** Read the channel's pin records once per channel; a landed read re-draws. */
  function ensureThreadPins(): void {
    const channel = String(deps.selectedChannelId());
    if (!channel || pinChannel === channel || pinLoadInFlight === channel) return;
    pinLoadInFlight = channel;
    void readThreadPins(channel).then((payload) => {
      pinRecords = new Map((payload.records || []).map((r) => [String(r.thread_id), Boolean(r.pinned)]));
      pinChannel = channel;
      renderThreadSelect();
    }).catch(() => {
      // No list: defaults alone (bang-bound pinned) still order the picker.
    }).finally(() => {
      if (pinLoadInFlight === channel) pinLoadInFlight = "";
    });
  }

  /** Pin or unpin `choice` on the server, then re-draw. */
  function toggleThreadPin(choice: ThreadChoice): void {
    const channel = String(deps.selectedChannelId());
    const next = !threadIsPinned(choice);
    pinRecords.set(choice.id, next);
    renderThreadSelect();
    void writeThreadPin(channel, choice.id, next).catch(() => {
      pinRecords.delete(choice.id);
      renderThreadSelect();
    });
  }

  /** Choices with pinned threads first; each group newest activity first. */
  function orderedThreadChoices(): ThreadChoice[] {
    const choices = threadChoices();
    return [
      ...choices.filter((choice) => threadIsPinned(choice)),
      ...choices.filter((choice) => !threadIsPinned(choice)),
    ];
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
    if (deps.transcriptsOpen && deps.transcriptsOpen()) return "transcripts";
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
      // `#1608 agent-transcripts-timeline`: Transcripts, with the views,
      // before the threads — and only where the service has threads for
      // traces to sit in.
      options.push(["transcripts", "Transcripts"]);
      const choices = orderedThreadChoices();
      const shown = choices.slice(0, THREAD_SELECT_LIMIT);
      for (const choice of shown) options.push([`thread:${choice.id}`, threadLabel(choice, Infinity)]);
      const open = openThreadChoice();
      if (open && !shown.some((choice) => choice.id === open.id)) {
        options.push([`thread:${open.id}`, threadLabel(open, Infinity)]);
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
    mountThreadPickerPopup(select);
    if (pickerPopup && !pickerPopup.hidden) drawThreadPickerPopup(select);
  }

  // The picker's own pop-up (`#1550 thread-list-pins`): the native select's
  // menu cannot hold a pin button on each row, so a pointer press opens this
  // list instead — fixed options first, then pinned threads, then recent
  // threads, each group newest first. The select stays the value's owner:
  // choosing a row sets it and fires its change, as a native pick does.
  // Keyboard use keeps the native menu.
  let pickerPopup: HTMLElement | null = null;
  let pickerPopupMounted = false;

  function pinButton(choice: ThreadChoice): HTMLButtonElement {
    const pinned = threadIsPinned(choice);
    const button = document.createElement("button");
    button.setAttribute("type", "button");
    button.className = "thread-pin";
    const anySvg = typeof document.querySelector === "function" ? document.querySelector("svg") : null;
    const ns = anySvg ? anySvg.namespaceURI : null;
    const svg = (ns ? document.createElementNS(ns, "svg") : document.createElement("svg")) as unknown as SVGSVGElement;
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("class", "thread-pin-icon");
    svg.setAttribute("fill", "currentColor");
    svg.setAttribute("aria-hidden", "true");
    const path = (ns ? document.createElementNS(ns, "path") : document.createElement("path")) as unknown as SVGPathElement;
    path.setAttribute("d", "M16 9V4h1c.55 0 1-.45 1-1s-.45-1-1-1H7c-.55 0-1 .45-1 1s.45 1 1 1h1v5c0 1.66-1.34 3-3 3H4c-.55 0-1 .45-1 1s.45 1 1 1h7v6c0 .55.45 1 1 1s1-.45 1-1v-6h7c.55 0 1-.45 1-1s-.45-1-1-1h-1c-1.66 0-3-1.34-3-3z");
    svg.append(path);
    button.append(svg);
    button.setAttribute("aria-pressed", String(pinned));
    const label = bangAgent(choice) ? `@${bangAgent(choice)}` : threadName(choice, Infinity);
    button.setAttribute("aria-label", pinned ? `Unpin ${label}` : `Pin ${label}`);
    button.title = button.getAttribute("aria-label") || "";
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleThreadPin(choice);
    });
    return button;
  }

  function drawThreadPickerPopup(select: HTMLSelectElement): void {
    if (!pickerPopup) return;
    const rows: HTMLElement[] = [];
    const fixed: Array<[string, string]> = [["main", "Main"]];
    if (deps.threading()) {
      fixed.push(["flat", "All"]);
      if (deps.threadListOffered()) {
        fixed.push(["threads", "Thread list"]);
        if (deps.layoutFit()) fixed.push(["columns", "Two columns"]);
      }
    }
    if (deps.threading()) fixed.push(["transcripts", "Transcripts"]);
    for (const [value, label] of fixed) {
      const row = document.createElement("button");
      row.setAttribute("type", "button");
      row.className = "thread-picker-row";
      row.setAttribute("role", "option");
      row.textContent = label;
      row.addEventListener("click", () => choosePickerValue(select, value));
      rows.push(row);
    }
    if (deps.threading()) {
      const choices = orderedThreadChoices();
      const shown = choices.slice(0, THREAD_SELECT_LIMIT);
      const open = openThreadChoice();
      const listed = open && !shown.some((choice) => choice.id === open.id) ? [...shown, open] : shown;
      for (const choice of listed) {
        const row = document.createElement("div");
        row.className = "thread-picker-row thread-picker-thread";
        row.setAttribute("role", "option");
        if (threadIsPinned(choice)) row.setAttribute("data-pinned", "");
        const openButton = document.createElement("button");
        openButton.setAttribute("type", "button");
        openButton.className = "thread-picker-open";
        const facts = threadFacts(choice as never) as string;
        const slug = threadSlug(choice);
        const title = threadTitle(choice);
        if (facts) {
          const factsEl = document.createElement("span");
          factsEl.className = "thread-picker-facts";
          factsEl.textContent = facts;
          openButton.append(factsEl, document.createTextNode(" "));
        }
        if (slug) {
          const slugEl = document.createElement("strong");
          slugEl.className = "thread-picker-slug";
          slugEl.textContent = slug;
          openButton.append(slugEl, document.createTextNode(" "));
        }
        const titleEl = document.createElement("span");
        titleEl.className = "thread-picker-title";
        titleEl.textContent = title || (slug ? "" : "Thread");
        openButton.append(titleEl);
        openButton.setAttribute("aria-label", threadLabel(choice, Infinity));
        openButton.addEventListener("click", () => choosePickerValue(select, `thread:${choice.id}`));
        row.append(pinButton(choice), openButton);
        rows.push(row);
      }
    }
    pickerPopup.replaceChildren(...rows);
  }

  function choosePickerValue(select: HTMLSelectElement, value: string): void {
    closeThreadPickerPopup();
    if (select.value === value) return;
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function closeThreadPickerPopup(): void {
    if (pickerPopup) pickerPopup.hidden = true;
  }

  function mountThreadPickerPopup(select: HTMLSelectElement): void {
    if (pickerPopupMounted) return;
    pickerPopupMounted = true;
    const popup = document.createElement("div");
    popup.className = "thread-picker-popup";
    popup.setAttribute("role", "listbox");
    popup.setAttribute("aria-label", "view");
    popup.hidden = true;
    document.documentElement.append(popup);
    pickerPopup = popup;
    select.addEventListener("pointerdown", (event?: PointerEvent) => {
      if (select.disabled) return;
      if (event && typeof event.preventDefault === "function") event.preventDefault();
      ensureThreadPins();
      drawThreadPickerPopup(select);
      const rect = select.getBoundingClientRect();
      popup.hidden = false;
      popup.style.width = "min(560px, calc(100vw - 16px))";
      const height = popup.offsetHeight;
      const viewWidth = document.documentElement.clientWidth;
      const viewHeight = document.documentElement.clientHeight;
      const below = rect.bottom + 4;
      const top = below + height <= viewHeight ? below : Math.max(8, rect.top - height - 4);
      popup.style.top = `${top + window.scrollY}px`;
      popup.style.left = `${Math.max(8, Math.min(rect.left, viewWidth - popup.offsetWidth - 8)) + window.scrollX}px`;
    });
    document.addEventListener("pointerdown", (event) => {
      if (popup.hidden) return;
      const target = event.target as Node;
      if (!popup.contains(target) && target !== select) closeThreadPickerPopup();
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") closeThreadPickerPopup();
    });
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
    if (value === "transcripts") {
      if (deps.openTranscripts) deps.openTranscripts();
      return undefined;
    }
    if (deps.transcriptsOpen && deps.transcriptsOpen() && deps.closeTranscripts) deps.closeTranscripts();
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

  /**
   * The drill-in anchor of the message on `row`, measured before navigating (`#1818
   * vibe-talk-thread-drill-in-keeps-anchor`): its id, and its top on the screen (its
   * viewport top) — the screen's measure, not the scroll container's, because the thread
   * it opens puts a header card above the scroller. If the message is the thread's root,
   * its id is the root's, and the root is what the thread anchors. Only a drill-in from a
   * message row measures one; the picker, the cards and the unread chip (which promises
   * the first unread reply) open a thread with no anchor and land as they always have.
   */
  function drillAnchorFor(row: HTMLElement, message: Message): { id: string; top: number } {
    return { id: String(message.id), top: row.getBoundingClientRect().top };
  }

  function openThread(id: unknown, summary: unknown = null, drill: { id: string; top: number } | null = null): unknown {
    if (!deps.threading() || !id) return;
    if (deps.layoutColumns()) return deps.selectSideThread(String(id), summary);
    if (deps.layoutThreadList()) return deps.openThreadFromList(String(id), summary);
    return deps.changeChannelView("thread", String(id), summary, null, null, drill);
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
    title.textContent = threadSlug(choice) || threadTitle(choice) || "Thread";
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
    // Every row's build passes through here: note the view it is built for, so a redraw for
    // another view builds it again rather than keep what this view put on it (`patchRows`).
    noteRowBuilt(row, deps.selection().view);
    const id = threadOf(message as never);
    if (!deps.threading() || !id || (deps.selection().view === "thread" && !everyThread) || id === deps.currentCanon().scope) return;
    const badge = document.createElement("button");
    badge.className = "thread-badge";
    badge.setAttribute("type", "button");
    const replies = deps.threadReplyCount(id, message);
    // `#1819 vibe-talk-thread-button-shows-slug`: the filed slug the badge shows with room —
    // the summary's `display_name`, else the root's own, else the stored root's, held to slug
    // shape by `filedThreadSlug` (the same filed name #1796 joins onto `:thread:` refs).
    const canon = deps.currentCanon();
    const summary = canon.threads.find((held) => String((held as { id: unknown }).id) === id) as { display_name?: unknown } | undefined;
    const ownName = (message as { thread?: { display_name?: unknown } | null }).thread?.display_name;
    let slug = filedThreadSlug(summary?.display_name, ownName);
    if (!slug) {
      const root = canon.messages.find((held) => threadOf(held as never) === id
        && (held as { thread?: { is_root?: unknown } | null }).thread?.is_root === true);
      slug = filedThreadSlug((root as { thread?: { display_name?: unknown } | null } | undefined)?.thread?.display_name);
    }
    deps.paintThreadBadge(badge, id, replies, deps.unansweredRoot(message, replies, 0, null), slug);
    badge.addEventListener("click", deps.inThisColumn(() => deps.guardQuietly(() => {
      if (row.hasAttribute("data-main-row")) return deps.openReplyFromRow(row);
      return badge.getAttribute("data-thread-state") === "start" ? deps.startThread(id) : openThread(id, null, drillAnchorFor(row, message));
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

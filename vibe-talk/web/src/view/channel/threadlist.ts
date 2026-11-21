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
  /** `#1985`: repaint the channel rows' thread chips in the pass summaries land in. Optional. */
  renderChannelRows?(): void;
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
  /** `#1846` step 4: render window in force, or null/ absent for full rendering (Off = old path). */
  windowed?(): { prefetchPx: number; rowCap: number } | null;
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
  /** Redraw the thread picker's pop-up while it is open, and nothing while it is shut (`#1906`). */
  refreshOpenPicker(): void;
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
  /**
   * The thread index built this turn, and the canon it was built from (`#2066
   * vibe-talk-page-freeze`): one picker render asks for the index several times
   * (choices, the open thread, each known thread), and each asking used to rescan
   * the whole canon. Within one task the canon cannot change, so the same index
   * answers them all; it is forgotten as the turn ends, never across one.
   */
  let indexThisTurn: { canon: unknown; byId: Map<string, ThreadChoice> } | null = null;

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
  // `#1986 vibe-talk-one-page-complete-rows`: the thread-pin revision and channel summary the
  // last timeline answer carried, so a change another device made re-reads the pin records
  // here, on a read that already happened, as message pins do by their revision.
  let threadPinsRevisionSeen: number | null = null;
  let channelSummarySeen: { unread_reply_count?: unknown } | null = null;

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
    // `#1986 vibe-talk-one-page-complete-rows`: the server's resolved pin, when a record
    // carries one, before the local bang derivation it replaces. Slice 2: the root's own
    // record on the messages page, then the thread list's summary.
    if (typeof choice.root?.thread?.pinned === "boolean") return choice.root.thread.pinned;
    if (typeof choice.summary?.pinned === "boolean") return choice.summary.pinned;
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
    const canon = deps.currentCanon();
    if (indexThisTurn && indexThisTurn.canon === canon) return indexThisTurn.byId;
    const byId = new Map<string, ThreadChoice>();
    if (!deps.threading() || canon.channel !== String(deps.selectedChannelId())) return byId;
    const pending = deps.pendingThreadTimes();
    const answered = new Set(canon.messages.filter((held: Message) => {
      const thread = (held as { thread?: { is_root?: boolean; id?: unknown } }).thread;
      return threadOf(held as never) && thread && thread.is_root !== true;
    }).map((held: Message) => String(((held as { thread?: { id?: unknown } }).thread as { id?: unknown }).id)));
    for (const summary of canon.threads) {
      const id = String((summary as { id: unknown }).id);
      if (id !== canon.scope) byId.set(id, threadChoice(id, summary as never));
    }
    for (const message of canon.messages) {
      const id = threadOf(message as never);
      if (!id || id === canon.scope) continue;
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
    if (typeof queueMicrotask === "function") {
      indexThisTurn = { canon, byId };
      queueMicrotask(() => {
        if (indexThisTurn && indexThisTurn.byId === byId) indexThisTurn = null;
      });
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
        // `#1986`: the server's unread sum over the threads it counted, on the label itself.
        const unread = channelSummarySeen && typeof channelSummarySeen.unread_reply_count === "number"
          ? channelSummarySeen.unread_reply_count : 0;
        options.push(["threads", unread > 0 ? `Thread list (${unread} unread)` : "Thread list"]);
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
  // list instead — threads chronological in a scrollable pane, the fixed
  // options bigger and centred under them (`#1887`, owner thread 277). The
  // select stays the value's owner: choosing a row sets it and fires its
  // change, as a native pick does. Keyboard use keeps the native menu.
  let pickerPopup: HTMLElement | null = null;
  let pickerPopupMounted = false;
  // `#2071`: whether the popup stands above its button. A redraw while it
  // stands (the pin records landing, a fuller directory read) can grow it;
  // above the button that growth must move its top, never cover the button
  // the reader taps to close it.
  let pickerAbove = false;

  function pinButton(choice: ThreadChoice, unread = ""): HTMLButtonElement {
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
    // `#1906`: the pin itself carries the thread's unread state, so a pinned thread
    // with waiting replies is marked where the owner looks for it.
    if (unread) button.setAttribute("data-unread-replies", unread);
    button.title = unread ? `${button.getAttribute("aria-label") || ""} (${unread})` : button.getAttribute("aria-label") || "";
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleThreadPin(choice);
    });
    return button;
  }

  // The popup's two parts (`#1887 vibe-talk-thread-picker-overhaul`, owner
  // thread 277): the threads in their own scrollable pane, chronological as
  // the messages stand (most recently modified at the bottom), and under
  // them the fixed options, bigger and centred where the thumb is. Built
  // once in `mountThreadPickerPopup`; `drawThreadPickerPopup` fills them.
  let pickerThreadsPane: HTMLElement | null = null;
  let pickerFixedBar: HTMLElement | null = null;

  // `#2071 vibe-talk-thread-picker-grid-toggle`: the last drawn popup's
  // signature. A redraw with nothing changed (a timeline poll landing while
  // the popup stands, a second open with the same threads) skips the DOM
  // rebuild entirely — rebuilding every row on open was the picker's local
  // share of the interface's unresponsiveness at phone speed.
  let pickerDrawnSignature = "";

  function drawThreadPickerPopup(select: HTMLSelectElement): void {
    if (!pickerPopup || !pickerThreadsPane || !pickerFixedBar) return;
    const current = viewChoiceOnScreen();
    const fixed: Array<[string, string]> = [["main", "Main"]];
    if (deps.threading()) {
      fixed.push(["flat", "All"]);
      if (deps.threadListOffered()) {
        fixed.push(["threads", "Thread list"]);
        if (deps.layoutFit()) fixed.push(["columns", "Two columns"]);
      }
    }
    if (deps.threading()) fixed.push(["transcripts", "Transcripts"]);
    pickerFixedBar.style.setProperty("--fixed-count", String(fixed.length));
    // Derive the thread rows once: the signature below and the build under
    // it read the same values, so a pin or an unread change still redraws.
    interface DrawnChoice { choice: ThreadChoice; pinned: boolean; unread: string; read: boolean; facts: string; slug: string; title: string; hue: string; label: string }
    const drawn: DrawnChoice[] = [];
    if (deps.threading()) {
      const choices = orderedThreadChoices();
      const shown = choices.slice(0, THREAD_SELECT_LIMIT);
      const open = openThreadChoice();
      const listed = open && !shown.some((choice) => choice.id === open.id) ? [...shown, open] : shown;
      // Chronological, oldest at the top: the most recently modified thread
      // stands at the bottom of the pane, next to the fixed options. A pin
      // still marks its thread (and still toggles); it no longer reorders.
      const chronological = [...listed].sort((a, b) =>
        (Number.isFinite(a.at) ? a.at : 0) - (Number.isFinite(b.at) ? b.at : 0));
      const tally = deps.replyTally();
      for (const choice of chronological) {
        const unread = deps.sideCardUnread(choice, tally);
        drawn.push({ choice, pinned: threadIsPinned(choice), unread,
          read: deps.threadIsRead(choice, unread), facts: threadFacts(choice as never) as string,
          slug: threadSlug(choice), title: threadTitle(choice), hue: deps.threadHue(choice.id),
          label: threadLabel(choice, Infinity) });
      }
    }
    const signature = JSON.stringify([current, fixed, drawn.map((d) =>
      [d.choice.id, d.pinned, d.unread, d.read, d.facts, d.slug, d.title, d.hue, d.label])]);
    if (signature === pickerDrawnSignature && pickerThreadsPane.childElementCount > 0) return;
    pickerDrawnSignature = signature;
    pickerFixedBar.replaceChildren(...fixed.map(([value, label]) => {
      const row = document.createElement("button");
      row.setAttribute("type", "button");
      row.className = "thread-picker-row thread-picker-fixed";
      row.setAttribute("role", "option");
      row.setAttribute("data-value", value);
      if (value === current) {
        row.setAttribute("aria-selected", "true");
        row.setAttribute("data-current", "");
      }
      row.textContent = label;
      row.addEventListener("click", () => choosePickerValue(select, value));
      return row;
    }));
    const rows: HTMLElement[] = [];
    if (deps.threading()) {
      const heading = document.createElement("div");
      heading.className = "thread-picker-group";
      heading.textContent = "Threads";
      rows.push(heading);
      for (const { choice, pinned, unread, read, facts, slug, title, hue, label } of drawn) {
        const row = document.createElement("div");
        row.className = "thread-picker-row thread-picker-thread";
        row.setAttribute("role", "option");
        row.style.setProperty("--thread-hue", hue);
        if (pinned) row.setAttribute("data-pinned", "");
        if (`thread:${choice.id}` === current) {
          row.setAttribute("aria-selected", "true");
          row.setAttribute("data-current", "");
        }
        // `#1906 vibe-talk-agent-replies-to-owner-root-not-surfaced`: a pinned (or any)
        // thread with replies the owner has not read says so on its picker row, beside
        // its pin, in the same words its thread card uses (`sideCardUnread`). Without
        // it, a pinned thread the owner works from shows nothing when an agent answers
        // inside it — the owner must open it to learn a reply exists.
        if (unread) row.setAttribute("data-unread-replies", unread);
        if (read) row.setAttribute("data-read", "");
        const openButton = document.createElement("button");
        openButton.setAttribute("type", "button");
        openButton.className = "thread-picker-open";
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
        openButton.setAttribute("aria-label", unread ? `${label}, ${unread}` : label);
        openButton.addEventListener("click", () => choosePickerValue(select, `thread:${choice.id}`));
        row.append(pinButton(choice, unread), openButton);
        if (unread) {
          const badge = document.createElement("span");
          badge.className = "thread-picker-unread";
          badge.textContent = unread;
          row.append(badge);
        }
        rows.push(row);
      }
    }
    // A redraw while the popup stands (a pin toggled, a directory read
    // landed) keeps the reader's place in the pane: at the bottom it stays
    // at the bottom (the pane commonly grows just after opening, when the
    // directory read lands fuller rows than the store first drew), and
    // anywhere else the place is restored once the new rows have been laid
    // out (reading `scrollHeight` makes the browser do it — restoring into
    // a still-empty pane would be clamped to the top and stay there).
    // Opening afresh starts at the bottom, in `mountThreadPickerPopup`.
    const hold = pickerThreadsPane.scrollTop;
    const atBottom = hold + pickerThreadsPane.clientHeight >= pickerThreadsPane.scrollHeight - 2;
    pickerThreadsPane.replaceChildren(...rows);
    const max = Math.max(0, pickerThreadsPane.scrollHeight - pickerThreadsPane.clientHeight);
    pickerThreadsPane.scrollTop = atBottom ? max : Math.min(hold, max);
    if (!pickerPopup.hidden && pickerAbove) {
      const rect = select.getBoundingClientRect();
      pickerPopup.style.top = `${Math.max(8, rect.top - pickerPopup.offsetHeight - 4) + window.scrollY}px`;
    }
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
    // `#2071`: an explicit close, named and focusable, beside the pane's
    // heading — outside taps and Escape already close, but neither is
    // discoverable on a phone.
    const head = document.createElement("div");
    head.className = "thread-picker-head";
    const headLabel = document.createElement("span");
    headLabel.className = "thread-picker-headlabel";
    headLabel.textContent = "View";
    const closeButton = document.createElement("button");
    closeButton.setAttribute("type", "button");
    closeButton.className = "thread-picker-close";
    closeButton.setAttribute("aria-label", "Close thread picker");
    closeButton.textContent = "×";
    closeButton.addEventListener("click", () => {
      closeThreadPickerPopup();
      select.focus();
    });
    head.append(headLabel, closeButton);
    const pane = document.createElement("div");
    pane.className = "thread-picker-threads";
    const bar = document.createElement("div");
    bar.className = "thread-picker-fixedbar";
    popup.append(head, pane, bar);
    document.documentElement.append(popup);
    pickerPopup = popup;
    pickerThreadsPane = pane;
    pickerFixedBar = bar;
    select.addEventListener("pointerdown", (event?: PointerEvent) => {
      if (select.disabled) return;
      if (event && typeof event.preventDefault === "function") event.preventDefault();
      // `#2071`: the toolbar button toggles. A second tap closes and stops
      // here — the document handler below ignores the select itself, so
      // this same tap cannot reopen what it just closed.
      if (!popup.hidden) {
        closeThreadPickerPopup();
        return;
      }
      ensureThreadPins();
      drawThreadPickerPopup(select);
      const rect = select.getBoundingClientRect();
      popup.hidden = false;
      popup.style.width = "min(560px, calc(100vw - 16px))";
      const height = popup.offsetHeight;
      const viewWidth = document.documentElement.clientWidth;
      const viewHeight = document.documentElement.clientHeight;
      const below = rect.bottom + 4;
      pickerAbove = below + height > viewHeight;
      const top = below + height <= viewHeight ? below : Math.max(8, rect.top - height - 4);
      popup.style.top = `${top + window.scrollY}px`;
      popup.style.left = `${Math.max(8, Math.min(rect.left, viewWidth - popup.offsetWidth - 8)) + window.scrollX}px`;
      // Opens at the bottom of the thread pane: the most recently modified
      // thread, beside the fixed options (`#1887`, owner thread 277).
      pane.scrollTop = pane.scrollHeight;
    });
    document.addEventListener("pointerdown", (event) => {
      if (popup.hidden) return;
      const target = event.target as Node;
      if (!popup.contains(target) && target !== select) closeThreadPickerPopup();
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && !popup.hidden) {
        closeThreadPickerPopup();
        select.focus();
      }
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
      // `#1986`: envelope facts on the same answer: a moved thread-pin revision re-reads the
      // pin records; the channel summary feeds the picker's Thread list label below.
      const revision = (payload as { thread_pins_revision?: unknown }).thread_pins_revision;
      if (typeof revision === "number" && revision !== threadPinsRevisionSeen) {
        const first = threadPinsRevisionSeen === null;
        threadPinsRevisionSeen = revision;
        if (!first && pinChannel === channel) {
          pinChannel = "";
          ensureThreadPins();
        }
      }
      const said = (payload as { channel_summary?: unknown }).channel_summary;
      channelSummarySeen = said && typeof said === "object" ? said as { unread_reply_count?: unknown } : channelSummarySeen;
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
      // `#1985`: the summaries this read brought are what the mounted rows'
      // Thread chips paint from; repaint them in this same pass instead of
      // leaving a bare Thread label until the next timeline poll.
      if (deps.renderChannelRows) deps.renderChannelRows();
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

  /**
   * What a card was built from, stamped on it as built (`#1839`): the summary without
   * its server version (`observed_ms` is restamped on every serve, as on a message),
   * and the counts and read state the tally gave it. A card whose stamp still matches
   * is the card that would be built, so a refresh that changed nothing keeps its node.
   */
  type CardStamp = string;
  const cardStampOf = (row: HTMLElement): CardStamp | undefined =>
    (row as { cardStamp?: CardStamp }).cardStamp;
  const versionless = (value: unknown): string =>
    JSON.stringify(value, (key, held) => (key === "observed_ms" ? undefined : held)) || "";

  // `#1846` step 4: measured card heights (px by thread id) + last summaries for scroll re-slice.
  const cardHeights = new Map<string, number>();
  let lastSummaries: readonly unknown[] = [];
  let threadWinListening = false;
  const CARD_EST_PX = 76;
  function threadCards(summaries: readonly unknown[]): HTMLElement[] {
    lastSummaries = summaries;
    const tally = deps.replyTally();
    // Keyed reuse (`#1839`): the cards now on the list, by thread, stand for a summary
    // whose stamp still matches; the rest are built as before. Gone threads' cards are
    // left out of the answer, and the caller's replaceChildren drops them.
    const held = new Map<string, HTMLElement[]>();
    const list = el("thread-list");
    if (list) {
      for (const row of [...list.children]) {
        const id = row.getAttribute("data-context-id");
        if (!id) continue;
        const rows = held.get(id);
        if (rows) rows.push(row as HTMLElement);
        else held.set(id, [row as HTMLElement]);
      }
    }
    const all = deps.withPendingThreads(summaries).map((summary: unknown) => {
      const id = String((summary as { id: unknown }).id);
      const candidates = held.get(id);
      const row = candidates ? candidates.shift() : undefined;
      if (row && cardStampOf(row) === cardStamp(summary, tally)) return row;
      return threadCard(summary, tally);
    });
    // Off (default S0): exactly the old call sequence — every card, no spacers.
    const cfg = deps.windowed ? deps.windowed() : null;
    if (!cfg || !list || all.length <= cfg.rowCap) return all;
    for (const row of all) {
      const id = row.getAttribute("data-context-id");
      if (id && (row as HTMLElement).offsetHeight) cardHeights.set(id, (row as HTMLElement).offsetHeight);
    }
    const heights = all.map((row) => cardHeights.get(row.getAttribute("data-context-id") || "") || CARD_EST_PX);
    // Discuss half-pane narrows width: bucket by list width so heights re-price on change.
    const offsets: number[] = [0];
    for (const h of heights) offsets.push((offsets[offsets.length - 1] as number) + h);
    const top = Math.max(0, (list.scrollTop || 0) - cfg.prefetchPx);
    const bottom = (list.scrollTop || 0) + (list.clientHeight || 600) + cfg.prefetchPx;
    let from = 0;
    while (from < all.length && (offsets[from + 1] as number) <= top) from += 1;
    let to = from;
    while (to < all.length && (offsets[to] as number) < bottom) to += 1;
    if (to - from > cfg.rowCap) to = from + cfg.rowCap;
    // Pin the open thread's card into the window.
    const openId = deps.selection().threadId ? String(deps.selection().threadId) : "";
    const openAt = openId ? all.findIndex((r) => r.getAttribute("data-context-id") === openId) : -1;
    if (openAt >= 0 && (openAt < from || openAt >= to)) { from = Math.max(0, Math.min(openAt, all.length - cfg.rowCap)); to = Math.min(all.length, from + cfg.rowCap); }
    const spacer = (kind: string, px: number): HTMLElement => {
      const node = document.createElement("li");
      node.dataset.windowSpacer = kind;
      node.setAttribute("aria-hidden", "true");
      node.style.height = `${Math.max(0, Math.round(px))}px`;
      node.style.padding = "0"; node.style.margin = "0"; node.style.border = "none"; node.style.listStyle = "none";
      return node;
    };
    if (!threadWinListening && typeof list.addEventListener === "function") {
      threadWinListening = true;
      let queued = false;
      list.addEventListener("scroll", () => {
        if (queued) return; queued = true;
        const run = () => { queued = false; if (deps.windowed && deps.windowed()) list.replaceChildren(...threadCards(lastSummaries)); };
        if (typeof requestAnimationFrame === "function") requestAnimationFrame(run); else run();
      }, { passive: true });
    }
    const mounted = all.slice(from, to);
    mounted.forEach((row, at) => { row.setAttribute("aria-setsize", String(all.length)); row.setAttribute("aria-posinset", String(from + at + 1)); });
    return [spacer("top", offsets[from] as number), ...mounted, spacer("bottom", (offsets[offsets.length - 1] as number) - (offsets[to] as number))];
  }

  /** The stamp of the card `summary` would build now, from its summary and the tally. */
  function cardStamp(summary: unknown, tally: unknown): CardStamp {
    const id = String((summary as { id: unknown }).id);
    const choice = threadChoice(id, summary as never);
    const unread = deps.sideCardUnread(choice, tally);
    const size = deps.cardMessageCount(choice, tally);
    return [versionless(summary), unread, deps.sideCardReplies(choice, tally),
      size.messages, size.exact, deps.threadIsRead(choice, unread)].join("|");
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
    (row as { cardStamp?: CardStamp }).cardStamp = cardStamp(summary, tally);
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

  function refreshOpenPicker(): void {
    if (pickerPopup && !pickerPopup.hidden) drawThreadPickerPopup(el("thread-select") as HTMLSelectElement);
  }

  return {
    noteThreadListLanded, threadWords, threadName, threadChoices, openThreadChoice,
    knownThreadChoice, openThreadName, olderThreadsOnServer, renderThreadSelect, refreshOpenPicker,
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

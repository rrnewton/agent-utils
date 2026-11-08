// A thread unpacked under its card: its block of rows, built, dressed and folded as a thread
// draws them (`#407 unpack-row-parity`, `#405 root-under-summary`, `#532 shift-o-latency`;
// `#624 voice-page-modularity`, moved from the legacy script into view/channel; mu-readcore).
// Which held messages they are is core/channels' (`unpackheld.ts`); the gaps among them are
// cardgaps.ts's; reading a thread that still owes one is the unpack-read queue's.

import type { Message } from "../../contract/index.ts";
import type { ChannelStore, ReplyTally } from "../../core/channels/index.ts";
import { renderedHtmlOf, rawTextOf } from "../../present/rows/index.ts";
import type { UnpackChoice, UnpackFilter } from "../../core/channels/index.ts";
import type { TimelineSelection } from "./readcore.ts";

/** What building an unpacked block asks of the page. */
export interface UnpackBlockDependencies {
  channelStore: ChannelStore;
  selectedChannelId(): string;
  selection(): TimelineSelection;
  viewKey(view?: string, thread?: string | null): string;
  discordNode(messages: Message[]): HTMLElement;
  glom(messages: readonly Message[]): Message[][];
  dressChannelRow(row: HTMLElement, facts: unknown): void;
  answeredBy(rows: unknown, viewing: unknown): unknown;
  readRunPlan(rows: unknown, key: string, joins: unknown, idsFor?: unknown, forcedIds?: Set<string>): Map<Element, { role: string; count: number }>;
  placeInReadRun(row: Element, role: string, count: number): void;
  foldedOneIds(key: string): Set<string>;
  unpackFilter: UnpackFilter;
  unpackHeldMessages(canon: unknown, choice: UnpackChoice, current: boolean): Message[];
  unpackMessages(choice: UnpackChoice, held: readonly Message[], withRoot: boolean, filter: UnpackFilter): Message[];
  unpackReadSignature(held: readonly Message[], dismissed: ReadonlySet<string>, filter: UnpackFilter): string;
  isThreadRoot(message: Message, choice: UnpackChoice): boolean;
  cardView(choice: UnpackChoice): { mode: string; cut?: { head: number } | null };
  cardSplit(choice: UnpackChoice): unknown;
  rootInward(choice: UnpackChoice): boolean;
  continuationOf(root: unknown, cut: unknown): Message;
  threadHue(id: string): string;
  packButton(id: string): Node;
  bridgePiece(row: HTMLElement): Node;
  messageLook(message: Message): string;
  readModes: { mode(): string };
  readRunsSuspended(): boolean;
  rowIsRead(row: Element): boolean;
  idsOf(row: Element | null | undefined): string[];
  readRunsOpened(key: string): Iterable<string> | null;
  hideEmptyOn(): boolean;
  combineOn(): boolean;
  hidingRead(): boolean;
  readAloudNow(): { id: unknown; pending: unknown };
  unpackReadQueue: { owed(choice: UnpackChoice, replies: number, cover: unknown): boolean; state(id: string): string; queue(id: string): void };
  unpackWatch(row: HTMLElement, id: string): void;
}

/** The unpacked block's commands: the legacy names, unchanged. */
export interface UnpackBlock {
  unpackBlock(choice: UnpackChoice & { count?: number }, tally: ReplyTally): HTMLElement[];
  redrawUnpackBlock(choice: UnpackChoice, drawn: HTMLElement[], tally: ReplyTally): void;
  unpackSignature(choice: UnpackChoice & { count?: number; root?: Message | null }, held: readonly Message[]): string;
  unpackNeedsRead(choice: UnpackChoice, held: readonly Message[]): boolean;
  queueUnpackRead(id: string): void;
  dispose(): void;
}

/** The unpacked block over `deps`. */
export function createUnpackBlock(deps: UnpackBlockDependencies): UnpackBlock {
  const channelStore = deps.channelStore;
  const selectedChannelId = deps.selectedChannelId;
  const viewKey = deps.viewKey;
  const discordNode = deps.discordNode;
  const glom = deps.glom;
  const dressChannelRow = deps.dressChannelRow;
  const answeredBy = deps.answeredBy;
  const readRunPlan = deps.readRunPlan;
  const placeInReadRun = deps.placeInReadRun;
  const foldedOneIds = deps.foldedOneIds;
  const unpackFilter = deps.unpackFilter;
  const unpackHeldMessages = deps.unpackHeldMessages;
  const unpackMessages = deps.unpackMessages;
  const unpackReadSignature = deps.unpackReadSignature;
  const isThreadRoot = deps.isThreadRoot;
  const cardView = deps.cardView;
  const cardSplit = deps.cardSplit;
  const rootInward = deps.rootInward;
  const continuationOf = deps.continuationOf;
  const messageLook = deps.messageLook;
  const readModes = deps.readModes;
  const readRunsSuspended = deps.readRunsSuspended;
  const rowIsRead = deps.rowIsRead;
  const idsOf = deps.idsOf;
  const hideEmptySwitch = { on: () => deps.hideEmptyOn() };
  const combineSwitch = { on: () => deps.combineOn() };
  const hidingRead = deps.hidingRead;
  const readRuns = { opened: (key: string) => deps.readRunsOpened(key) };
  const readAloudOwner = () => { const now = deps.readAloudNow(); return { nowPlaying: now.id ? { id: now.id } : null, pendingRead: now.pending }; };
  const unpackReadQueue = deps.unpackReadQueue;
  const unpackWatch = { watch: (row: HTMLElement, id: string) => deps.unpackWatch(row, id) };
  const threadHue = deps.threadHue;
  const packButton = deps.packButton;
  const bridgePiece = deps.bridgePiece;
  let disposed = false;
  void disposed;

function unpackNeedsRead(choice: UnpackChoice, held: readonly Message[]): boolean {
  const replies = held.filter((message) => !isThreadRoot(message, choice)).length;
  return unpackReadQueue.owed(choice, replies, channelStore.current().views.get(viewKey("thread", choice.id)));
}

/**
 * Place the runs of thread `choice`'s unpacked message rows `rows`, in order: Fold read's runs of read
 * messages and the messages folded away one by one (`readRunPlan`), the root under a summarized card
 * never in one. For a block being built (`unpackBlock`), and again on a block already drawn when only
 * what was folded away changed (`redrawUnpackBlock`).
 *
 * @param {ReturnType<typeof threadChoice>} choice
 * @param {HTMLElement[]} rows
 */

function placeUnpackRuns(choice: UnpackChoice, rows: HTMLElement[]): void {
  const key = viewKey("thread", choice.id);
  const forced = foldedOneIds(key);
  const placed = (readModes.mode() === "fold" || forced.size > 0) && !readRunsSuspended()
    ? readRunPlan(rows, key, (row: Element) => !row.hasAttribute("data-root-child") &&
      ((readModes.mode() === "fold" && rowIsRead(row)) || idsOf(row).some((id) => forced.has(id))), idsOf, forced)
    : new Map();
  for (const row of rows) {
    const { role, count } = placed.get(row) || { role: null, count: 0 };
    placeInReadRun(row, role, count);
  }
}

/**
 * `#532 shift-o-latency`. Thread `choice`'s block as drawn, `drawn` its rows, brought into line where only
 * its messages' read state or what is folded away changed: each message row dressed again
 * (`dressChannelRow`), as a row built now would be, and the runs placed again (`placeUnpackRuns`). A Done,
 * an e, a put back or a Shift+O drawn on the rows already there, in the frame of the key, rather than the
 * block built again. Its loading or end line is not a message row.
 *
 * @param {ReturnType<typeof threadChoice>} choice
 * @param {HTMLElement[]} drawn
 * @param {ReplyTally} tally
 */

function redrawUnpackBlock(choice: UnpackChoice, drawn: HTMLElement[], tally: ReplyTally): void {
  const rows = drawn.filter((row) => row.hasAttribute("data-id"));
  const viewing = String(selectedChannelId());
  const pass = { tally, answered: answeredBy(rows, viewing), viewing, inMain: false, inAll: false };
  for (const row of rows) dressChannelRow(row, pass);
  placeUnpackRuns(choice, rows);
}

/** What one block would draw, as a signature, so a pass that changes nothing rebuilds nothing. */

function unpackSignature(choice: UnpackChoice & { count?: number; root?: Message | null }, held: readonly Message[]): string {
  const opened = readRuns.opened(viewKey("thread", choice.id));
  const cover = channelStore.current().views.get(viewKey("thread", choice.id));
  const view = cardView(choice);
  return JSON.stringify([choice.id, readModes.mode(), readRunsSuspended(), hideEmptySwitch.on(), combineSwitch.on(),
    view.mode, view.cut ? view.cut.head : 0, view.cut && choice.root ? renderedHtmlOf(choice.root as Message) || rawTextOf(choice.root as Message) : "",
    held.map(messageLook), hidingRead() ? unpackReadSignature(held, channelStore.current().dismissed, unpackFilter) : "",
    opened ? [...opened].sort() : [], unpackReadQueue.state(choice.id) || "", readAloudOwner().nowPlaying ? (readAloudOwner().nowPlaying as { id: unknown }).id : "", readAloudOwner().pendingRead,
    // `#532 shift-o-latency`: what decides its loading or end line (`unpackNeedsRead`, the cover), which
    // a block kept from the last pass would otherwise never ask again.
    typeof choice.count === "number" ? choice.count : null, cover ? cover.more !== false : null]);
}



/**
 * Thread `choice`'s unpacked block: its message rows under its card, bridged. They are the rows a
 * thread draws, built, dressed and folded by the code that draws a thread (`#407 unpack-row-parity`):
 * `discordNode` over `glom`, the dress of every channel row (`dressChannelRow`), what Hide read keeps
 * of a thread (`readFilterKeeps`) and the runs Fold read folds (`readRunPlan`), each opened as in
 * the thread. Built in the right column (`renderSideCards`), whose list the keys and the mouse move
 * through here (`sidePane`), so a key on a row and a press of its own button act in the same column.
 * Under a summarized card the root is the first child, kept under every read mode and never in a run
 * (`#405 root-under-summary`); under a raw card the replies alone, the card already being the root.
 * A thread still owed a read ends in its loading line, observed.
 *
 * @param {ReturnType<typeof threadChoice>} choice
 * @param {ReplyTally} tally
 * @returns {HTMLElement[]}
 */

function unpackBlock(choice: UnpackChoice & { count?: number }, tally: ReplyTally): HTMLElement[] {
  const held = unpackHeldMessages(channelStore.current(), choice, channelStore.current().channel === String(selectedChannelId()));
  // `#528 thread-card-markdown-and-cap`: and under a card waiting for its summary, or one clamped.
  const summarized = rootInward(choice);
  // `#522 thread-card-max-lines`. Under a split card the root's continuation is the first child: the
  // root's own row, alone in it, never joined to a reply by the same author.
  const split = summarized ? null : cardSplit(choice);
  const messages = unpackMessages(choice, held, summarized || split !== null, unpackFilter);
  const root = split ? messages.find((message) => isThreadRoot(message, choice)) || null : null;
  const groups: Message[][] = root
    ? [[continuationOf(root, split)], ...glom(messages.filter((message) => message !== root))]
    : glom(messages);
  const rows = groups.map((group, at) => {
    const row = discordNode(group);
    row.setAttribute("data-unpack-thread", choice.id);
    row.className = `${row.className} unpack-row`;
    if (summarized && group.some((message) => isThreadRoot(message, choice))) row.setAttribute("data-root-child", "");
    if (root && at === 0) {
      row.setAttribute("data-root-child", "");
      row.setAttribute("data-root-continuation", "");
      row.setAttribute("title", "The rest of the first message, continued from its card");
      // Drawn from its cut words, it acts as the root whole: the row's messages, and the list every
      // act drawn on it was given, become the root itself — what a copy copies, a reply answers and
      // reading aloud reads.
      group.splice(0, 1, root);
    }
    return row;
  });
  const viewing = String(selectedChannelId());
  const pass = { tally, answered: answeredBy(rows, viewing), viewing, inMain: false, inAll: false };
  rows.forEach((row, at) => {
    dressChannelRow(row, pass);
    const place = rows.length === 1 ? "only" : at === 0 ? "first" : at === rows.length - 1 ? "last" : "middle";
    row.setAttribute("data-coalesce", place);
    row.setAttribute("data-stack-under", "third");
    row.style.setProperty("--thread-hue", threadHue(choice.id));
    // `#440 thread-card-pack-toggle`. The first row's gutter holds the triangle that packs the thread,
    // where All's bridge holds its X.
    row.replaceChildren(...(at === 0 ? [packButton(choice.id)] : []), bridgePiece(row), ...row.children);
  });
  placeUnpackRuns(choice, rows);
  const cover = channelStore.current().views.get(viewKey("thread", choice.id));
  if (unpackReadQueue.state(choice.id) === "failed") {
    const failed = document.createElement("li");
    failed.className = "unpack-row unpack-loading hint";
    failed.setAttribute("data-unpack-thread", choice.id);
    failed.textContent = "Could not load this thread.";
    rows.push(failed);
  } else if (unpackNeedsRead(choice, held)) {
    const loading = document.createElement("li");
    loading.className = "unpack-row unpack-loading hint";
    loading.setAttribute("data-unpack-thread", choice.id);
    loading.textContent = "Loading thread…";
    rows.push(loading);
    watchUnpackBlock(loading, choice.id);
  } else if (cover && cover.more) {
    const older = document.createElement("li");
    older.className = "unpack-row unpack-older hint";
    older.setAttribute("data-unpack-thread", choice.id);
    older.textContent = "Older messages are in the thread.";
    rows.push(older);
  }
  return rows;
}


/** Read thread `id` for its block as `row` nears the screen; at once where nothing can watch. */

function watchUnpackBlock(row: HTMLElement, id: string): void {
  unpackWatch.watch(row, id);
}

/** Queue thread `id`'s newest page for its block, `UNPACK_READS_AT_ONCE` at a time. */

function queueUnpackRead(id: string): void {
  unpackReadQueue.queue(id);
}

// --- the main channel's messages among the cards (#419 threads-view-main-messages) -------------------
//
// The owner, 2026-10-21: "In Cmd-3 thread list view... we don't see the main channel messages that are
// not yet threads, and we cannot send messages from this view to the main channel ... a toggle to show
// Main msgs. When that is on we should see messages to the main channel that are not (yet) threads and
// can make them threads from there ... when we have Main messages turned on, we should also have the
// Reply box (to main channel) as our implicit last card in our list."
//
//   * MAIN CHANNEL (key a; "Main messages" until `#421 chip-states`), beside Unread only and Unpack msgs, over the Threads view's cards, kept on this
//     device. Pressed, the cards' list also holds the main channel's messages that head no thread a card
//     stands for: what Main draws, less its threads' roots. Each is a message row, built and dressed by the
//     code every list's rows are (`discordNode`, `dressChannelRow`), marked `data-main-row`, and placed
//     among the cards in the list's order, a card where its thread last moved and a message where it was
//     posted. The search finds them by their words, Hide read leaves out the read ones, as it does in
//     Main, and Fold read folds them into the cards' runs (`foldCardRuns`, `#432 read-mode-unify`).
//   * MADE A THREAD WHERE IT IS. Thread or Reply on one opens the reply in the list under it (`#410
//     inline-reply-editor`), bound for a new thread where the service starts one on a reply's say-so (the
//     new-thread choice ticked; Slack threads every reply anyway), not the thread view.
//   * THE MAIN CHANNEL'S BOX IS THE LAST CARD. The left column is Main behind the list (`leftColumnToMain`),
//     so its box posts to the main channel: it is moved to the foot of the cards while Main channel is on
//     (`placeMainCard`), and back when the list goes or the chip is let go. It is the one box: a reply
//     opened anywhere hides it, and putting the reply down gives it back (`dockReply`).

/** The device's choice of Main channel among the cards: "on", or anything else for cards alone. */

  return {
    unpackBlock, unpackNeedsRead, queueUnpackRead, redrawUnpackBlock, unpackSignature,
    dispose() { disposed = true; },
  };
}

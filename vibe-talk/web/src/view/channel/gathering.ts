// Gathering a message's replies under it (`#215 reply-coalesce`, `#229 desktop-two-column`,
// `#405 root-under-summary`, `#410 inline-reply-editor`, `#501 thread-bridge-gutter`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel for the plan's step 17):
// what a reply is (by pointer, and as a thread's), the stack drawn under its parent, the bridge
// and the X, and the rows put back in time order. The columns' records, the list, the rows and
// everything the walk reads are handed in; the words are the page's.

import { inTimeOrder, threadOf } from "../../core/channels/index.ts";
import { childByClass } from "../dom/index.ts";
import { idsOf, rowShown } from "./rows.ts";
import { fromGatheredThread, gutterPiece, stackUnder } from "./replies.ts";
import { REPLY_LANDING_GAP_PX } from "./replyarrows.ts";

import type { Message } from "../../contract/index.ts";

/** A message, as the gathering reads it. */
export type GatherMessage = Message;

/** A column's record of the message whose replies are gathered under it (`freshGatherState`). */
export interface GatherState {
  gathered: { parent: string; context: string; byReply?: boolean; thread?: string | null; column?: unknown } | null;
}

/** A fresh record of what is gathered in a column's list (`freshGatherState`). */
export function freshGatherState(): GatherState {
  return { gathered: null };
}

/** A column's record of what its list has drawn (`freshListState`). */
export interface ListState {
  rowsGathered: boolean;
  outgoingKey: string;
  newestId: string | null;
  runRows: number;
  backlog: number;
}

/** A fresh record of what a column's list has drawn (`freshListState`). */
export function freshListState(): ListState {
  return { rowsGathered: false, outgoingKey: "", newestId: null, runRows: 0, backlog: 0 };
}

/** A thread choice, as the gathering reads it. */
export interface GatherThreadChoice {
  id: string;
}

/** The reply links among a set of messages (`replyLinks`). */
export interface ReplyLinks {
  children: Map<string, string[]>;
  parents: Map<string, string[]>;
  heads: Map<string, string>;
}

/** What the gathering is handed by the page. */
export interface GatheringDependencies {
  el(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  threading(): boolean;
  selection(): { view: string; threadId: string | null };
  channelContextKey(): string;
  knownThreadChoice(id: string, fallback: null): GatherThreadChoice | null;
  selectedChannelId(): unknown;
  channelCanon(): { channel: string; scope: unknown; messages: GatherMessage[] };
  loadCanon(channel: string): void;
  heldMessage(id: string): GatherMessage | null;
  heldThreadRecord(message: GatherMessage, held: GatherMessage | null): { id?: unknown; is_root?: boolean } | null;
  rowMessages(row: Element): GatherMessage[];
  glom(messages: readonly GatherMessage[]): GatherMessage[][];
  discordNode(group: readonly GatherMessage[]): HTMLElement;
  unpackMessages(thread: GatherThreadChoice, copies: unknown, flag: boolean, filter: unknown): GatherMessage[];
  unpackHeldMessages(canon: unknown, thread: GatherThreadChoice, sameChannel: boolean): unknown;
  unpackNeedsRead(thread: GatherThreadChoice, copies: unknown): boolean;
  queueUnpackRead(id: string): void;
  unpackFilter(): unknown;
  layoutColumns(): boolean;
  sideColumn(): unknown;
  openThread(id: string): void;
  searchOpen(): boolean;
  setSearchOpen(open: boolean): void;
  seekMessage(target: string, arrive: () => unknown, openAll: () => unknown): Promise<void>;
  rowHolding(id: string): HTMLElement | null;
  setStatus(text: string): void;
  renderChannelRows(): void;
  renderScrollTools(): void;
  countOf(count: number): string;
  hidingRead(): boolean;
  threadReplies(thread: string, message: GatherMessage | null): { exact?: boolean; count?: number };
  timelineRows(): { messages: GatherMessage[] };
  renderJumpNewest(): void;
  renderReplyCounts(): void;
  measureReplyArrows(): void;
  cardRow(thread: string): HTMLElement | null;
  finishPackSlide(): void;
  glideBehavior(): string;
  threadHue(id: string): string;
  openThreadRootId(): string | null;
  openThreadChoice(): unknown;
  rootInward(choice: unknown): boolean;
  isOpenThreadRoot(row: HTMLElement, id: string): boolean;
  replyArrows(): { settling: unknown };
  takeClaimedClick(): boolean;
  drawnTop(row: Element): number;
  floatingClearance(area: Element): number;
  activeGather(): GatherState;
  activeList(): ListState;
}

/** The gathering, as the page calls it. */
export interface Gathering {
  gatherState(): GatherState;
  listState(): ListState;
  gatheredParent(): string | null;
  gatheredThread(): GatherThreadChoice | null;
  heldMessagesById(): Map<string, GatherMessage>;
  replyLinks(messages: readonly GatherMessage[]): ReplyLinks;
  repliesTo(parent: string, messages: readonly GatherMessage[]): Set<string>;
  conversationHead(target: string, from: string): string | null;
  gatheredRows(messages: readonly GatherMessage[], parent: string): GatherMessage[][] | null;
  listedMessages(): GatherMessage[];
  arrangeChannelRows(): void;
  gatherFromReply(reply: HTMLElement): Promise<void> | undefined;
  toggleGathered(parent: string, row: HTMLElement): Promise<void> | undefined;
  gatherReplies(parent: string, from: HTMLElement, headOf?: (id: string) => string | null): Promise<void>;
  gatherNow(parent: string, anchor: string, anchorTop: number): void;
  gatherReport(parent: string): { gathered: number; hidden: number; missing: number };
  keepOnScreen(id: string, top: number): void;
  scatterReplies(): void;
  putRepliesBack(): void;
  renderReplyLinks(): void;
  bridgeTop(row: HTMLElement): HTMLElement | null;
  glideToBridgeTop(row: HTMLElement): void;
  headOfList(area: HTMLElement, node: HTMLElement): number;
  bridgePiece(row: Element): HTMLElement;
  ungatherButton(): HTMLElement;
}

/** Make the gathering from the page's columns, list and rows. */
export function createReplyGathering(deps: GatheringDependencies): Gathering {
  // --- gathering a message's replies under it ---------------------------------------------------
  //
  // `#215 reply-coalesce`. The owner: "When I click the N Replies oval I want all the replies to move
  // to join their parent in a dense stack underneath. Other messages don't disappear, this is a
  // reorder operation." The stacked replies lose their arrows for one BRIDGE in the arrow's colour —
  // a spine out of the parent's foot down the gutter, and a short line from it into each reply — and
  // an X beside the bridge puts them back in time order. Two taps on any reply's arrow gather the
  // conversation it is in. "I don't think we need to scroll": the parent stays where it was on the
  // screen, and for two taps on an arrow, the reply that was tapped does.
  //
  // WHAT IS A REPLY, HERE. The providers carry two kinds, and both count: a message whose own reply
  // pointer names another — Discord's replies, and Google Chat's quote-replies, which arrive as the
  // same pointer — and, where a message heads a thread, every other message of that thread, which is
  // the only reply Slack has. A channel registered as one thread is not a thread here, or every
  // message in it would be a reply to its root.
  //
  // AND THE REPLIES TO THOSE, ALL THE WAY DOWN. The owner, 2026-10-08: a pointer may name any message,
  // so "B answers A, C answers B" and "B and C both answer A" are different trees — and for gathering
  // they are the same. What is gathered under a parent is every message that answers it, every message
  // that answers one of those, and so on, by either kind of reply: drawn as ONE stack in time order
  // under the parent, joined by one bridge, not as the tree (`replyGroup`). The walk visits each
  // message once, so a cycle in the pointers — a provider's data, not something a person can send —
  // ends instead of looping, and the walk is never longer than the list. It walks everything the page
  // holds of the view, so a link through a message Hide read leaves out still joins the replies drawn
  // either side of it; only what is drawn moves.
  //
  // ONE TAP STILL MEANS THE REPLY'S OWN PARENT. One tap on an arrow jumps to the message the reply
  // actually answers, which in a chain is the reply before it, not the conversation's first message.
  // TWO TAPS gather the conversation the reply is in under its HEAD: walking up from what the reply
  // answers — its pointer first, then the root of its thread — to the topmost message of it the list
  // draws (`conversationHead`). Not under what it answers, because in a chain that message is in the middle
  // of the conversation: gathering under it would pull its own replies up under it and leave it, and
  // everything above it, where they were — a piece of the conversation stacked in the middle of the
  // list, and a different piece for each reply tapped. The head's group holds the whole conversation,
  // the tapped reply too, so two taps on any arrow in it draw the same stack.
  //
  // WHERE. All, which is the one view holding every message, and the one list of a provider without
  // threads. Asked from Main or a thread, the page opens All first, exactly as the arrow's jump does,
  // so All is then what this channel reopens in (`#205 channel-view-memory`): the reader is reading
  // All, and a reopen should come back where they were left.
  //
  // WHAT IS LOADED IS WHAT MOVES, AND NOTHING IS FETCHED. Every reply is newer than its parent, and the
  // list is the newest stretch of the channel with nothing left out of it, so a parent on screen has
  // every reply it has on screen too. Two things can still keep one off, and both are SAID: Hide read,
  // which leaves out what is read, and a provider's own reply count saying a thread holds more than its
  // history handed back. Reading the thread to make up the difference would put rows into All that All's
  // own read did not return.
  //
  // ONE AT A TIME. Gathering another message's replies puts the first's back. Two stacks interleaved
  // with what is left in time order would leave the reader unsure which order any stretch of the list
  // is in — and a reply can answer a message in the other stack, so they could not both be stacks.
  //
  // A WAY OF ARRANGING THE ROWS, NOT A CHANGE TO ANYTHING. Nothing is read, marked or written. It
  // outlives a refresh, a delta and a live arrival — the rows are drawn gathered wherever they are
  // drawn, and a new reply joins the stack — and goes with a change of view or channel. A filter that
  // takes rows away (search text, Links, Pinned) puts the replies back: it shows a subset of the
  // channel in time order, and a bridge to rows it hides would be a bridge to nothing. Hide read is
  // not that kind of filter here — it decides which messages are drawn at all — so the stack is simply
  // the replies it leaves; a parent it hides takes the stack with it.

  /** The X's accessible name and tooltip: what pressing it does, not what it looks like. */
  const UNGATHER_NAME = "Show replies in time order";

  /**
   * The message whose replies are gathered under it, as `{ parent, context }`; null when none are.
   *
   * `#410 inline-reply-editor`. Reply gathers too, in any list of the channel, not only in All: `byReply`
   * says Reply did, which lets the gathering stand in Main and in a thread, where the replies by pointer
   * to a message come up under it. On a thread's root in Main, `thread` is that thread: its replies,
   * which Main does not draw, are drawn under the root as rows of the list for as long as it stands
   * (`arrangeChannelRows`), and `column` is the column it was made in.
   *
   * @type {{parent: string, context: string, byReply?: boolean, thread?: string | null, column?: any} | null}
   *
   * A column's `gatherState().gathered` (`#624 voice-page-modularity`, plan step 15, as
   * `freshOlderPages`): its own record, made by `freshGatherState`.
   */

  /**
   * A column's own record of the message whose replies are gathered under it (`gathered`, above).
   *
   * @returns {{gathered: {parent: string, context: string, byReply?: boolean, thread?: string | null, column?: any} | null}}
   */

  /** The column being worked on's record of what is gathered in its list (`freshGatherState`). */
  function gatherState(): GatherState {
    return deps.activeGather();
  }

  /**
   * A column's own record of what its list has drawn (`#624 voice-page-modularity`, plan step 15, as
   * `freshOlderPages`), each said where it was declared: `rowsGathered`, whether the channel's rows are
   * drawn gathered now; `outgoingKey`, `outgoingReplyKey` as it stood when the Reply counts were last
   * drawn; `newestId`, the id of the newest message the channel list is showing; `runRows`, how many rows
   * the last pass left in collapsed runs; `backlog`, how many the backlog control is about to clear.
   *
   * @returns {{rowsGathered: boolean, outgoingKey: string, newestId: string | null, runRows: number, backlog: number}}
   */

  /** The column being worked on's record of what its list has drawn (`freshListState`). */
  function listState(): ListState {
    return deps.activeList();
  }

  // Whether the channel's rows are drawn gathered now, so putting them back has something to undo. Kept per column: `listState().rowsGathered` (`freshListState`, `#624 voice-page-modularity`).

  /** Whether the list on screen is one replies are gathered in: All, or a provider's only list. */
  const gatheringView = () => !deps.threading() || deps.selection().view === "flat";

  /** The message whose replies are gathered in the list on screen, or null. */
  function gatheredParent(): string | null {
    const gathered = gatherState().gathered;
    if (gathered === null || gathered.context !== deps.channelContextKey() || !(gatheringView() || gathered.byReply)) return null;
    return gathered.parent;
  }

  /**
   * The thread whose replies are drawn under its root in this list, gathered by Reply on the root in Main
   * (`#410 inline-reply-editor`), as its choice; null when none is.
   */
  function gatheredThread(): GatherThreadChoice | null {
    const gathered = gatherState().gathered;
    if (gatheredParent() === null || !gathered || !gathered.thread) return null;
    return deps.knownThreadChoice(String(gathered.thread), null);
  }

  /** The store's copies of this channel's messages by id, for a pass that asks after many of them. */
  function heldMessagesById(): Map<string, GatherMessage> {
    const channel = String(deps.selectedChannelId());
    if (deps.channelCanon().channel !== channel) deps.loadCanon(channel);
    return new Map(deps.channelCanon().messages.map((held) => [String(held.id), held]));
  }

  /**
   * The reply links among `messages`, both kinds (see the head of this section). `children` maps a
   * message's id to the ids of the messages that answer it: by their pointer, whether or not the
   * message it names is among `messages`, and as the rest of the thread it heads. `parents` maps a
   * message's id to the ids, among `messages`, of what it answers: its pointer's first, then its
   * thread's root. `heads` maps each thread a message among them heads to that message's id.
   */
  function replyLinks(messages: readonly GatherMessage[]): ReplyLinks {
    const ids = new Set(messages.map((message) => String(message.id)));
    const children = new Map();
    const parents = new Map();
    const heads = new Map();
    const link = (child: string, parent: string): void => {
      if (!parent || child === parent) return;
      if (!children.has(parent)) children.set(parent, []);
      children.get(parent).push(child);
      if (!ids.has(parent)) return;
      if (!parents.has(child)) parents.set(child, []);
      parents.get(child).push(parent);
    };
    if (deps.threading()) {
      const held = heldMessagesById();
      for (const message of messages) {
        const record = deps.heldThreadRecord(message, held.get(String(message.id)) || null);
        if (record && record.is_root === true && String(record.id) !== String(deps.channelCanon().scope)) {
          heads.set(String(record.id), String(message.id));
        }
      }
    }
    for (const message of messages) {
      const own = String(message.id);
      link(own, String(message.reply_to || ""));
      const thread = deps.threading() ? threadOf(message) : null;
      if (thread !== null && heads.has(thread)) link(own, heads.get(thread));
    }
    return { children, parents, heads };
  }

  /**
   * What the reply links are walked over: `messages`, and, where the provider has threads, every other
   * message the page holds of the view — the ones Hide read leaves out — so that a link through one of
   * them still joins the messages drawn either side of it.
   */
  function withHeldMessages(messages: readonly GatherMessage[]): GatherMessage[] {
    if (!deps.threading()) return [...messages];
    const ids = new Set(messages.map((message) => String(message.id)));
    return [...messages, ...deps.timelineRows().messages.filter((message) => !ids.has(String(message.id)))];
  }

  /**
   * Message `parent`'s group, from `messages` and `withHeldMessages`: as `replies`, the ids of every
   * message that answers it, every message that answers one of those, and so on, by either kind of
   * reply (see the head of this section); as `threads`, each thread the parent or one of its replies
   * heads, by its id, to the id of the message heading it.
   *
   * A breadth-first walk from the parent in which a message already found is not walked again and the
   * parent is never its own reply: a cycle in the pointers ends where it comes back round, and the walk
   * is one step per link, of which a message has at most two — its pointer and its thread.
   */
  function replyGroup(parent: string, messages: readonly GatherMessage[]): { replies: Set<string>; threads: Map<string, string> } {
    const id = String(parent);
    const walked = withHeldMessages(messages);
    const root = walked.some((message) => String(message.id) === id) ? null : deps.heldMessage(id);
    const { children, heads } = replyLinks(root ? [...walked, root] : walked);
    const replies = new Set<string>();
    const queue: string[] = [id];
    for (let i = 0; i < queue.length; i += 1) {
      for (const child of children.get(queue[i] as string) || []) {
        if (child === id || replies.has(child)) continue;
        replies.add(child);
        queue.push(child);
      }
    }
    return { replies, threads: new Map([...heads].filter(([, head]) => head === id || replies.has(head))) };
  }

  /** The ids, among `messages`, of message `parent`'s replies, to any depth: see `replyGroup`. */
  function repliesTo(parent: string, messages: readonly GatherMessage[]): Set<string> {
    const among = new Set(messages.map((message) => String(message.id)));
    return new Set([...replyGroup(parent, messages).replies].filter((one) => among.has(one)));
  }

  /**
   * The head of the conversation that message `target` is in, for two taps on the arrow of `from`, a
   * reply to it: the topmost message of it that the list draws, walking up from `target` by what each
   * message answers, its pointer first and then its thread's root (see the head of this section). Null
   * when neither `target` nor anything above it is drawn.
   *
   * Walked over what the page holds of the view, so a message Hide read leaves out is passed through,
   * though it cannot be the head, since it is not drawn. No message is walked twice and `from` never
   * is, so a cycle in the pointers ends where it would come back round, at most a step per message.
   */
  function conversationHead(target: string, from: string): string | null {
    const drawn = [...deps.el("discord-log").children].flatMap(deps.rowMessages);
    const shown = new Set(drawn.map((message) => String(message.id)));
    const { parents } = replyLinks(withHeldMessages(drawn));
    const seen = new Set(String(from) === String(target) ? [] : [String(from)]);
    let head: string | null = null;
    for (let at: string | undefined = String(target); at !== undefined && !seen.has(at);
      at = (parents.get(at) || []).find((one) => !seen.has(one))) {
      seen.add(at);
      if (shown.has(at)) head = at;
    }
    return head;
  }

  /**
   * `messages`, oldest first, as the rows to draw with `parent`'s replies gathered under the row that
   * holds it; null when no row would hold it. The replies keep their time order among themselves and
   * everything else keeps its place.
   *
   * The combining rule (`glom`) applies within three runs — what is up to the end of the parent's row,
   * the stack, and what follows — and never across their ends. THE PARENT'S ROW IS THE ROW TIME ORDER
   * DRAWS IT IN, less any reply in it: a message of the parent's author that only a reply kept out of
   * that row must not join it once the reply has moved, or it would sit above the stack, newer than the
   * replies under it, inside the box the bridge comes out of as though it were what they answer. What
   * follows the parent's row does combine across the gaps the replies leave: those rows are not where
   * the bridge comes from, and two of one author's messages with nothing now between them are one row
   * anywhere else in the list.
   */
  function gatheredRows(messages: readonly GatherMessage[], parent: string): GatherMessage[][] | null {
    const id = String(parent);
    const replies = repliesTo(parent, messages);
    const own = deps.glom(messages).find((group) => group.some((message) => String(message.id) === id));
    if (!own) return null;
    const end = messages.indexOf(own[own.length - 1] as GatherMessage) + 1;
    const kept = (message: GatherMessage): boolean => !replies.has(String(message.id));
    const rows = [...deps.glom(messages.slice(0, end).filter(kept)), ...deps.glom(messages.slice(end).filter(kept))];
    const at = rows.findIndex((group) => group.some((message) => String(message.id) === id));
    if (at < 0) return null;
    rows.splice(at + 1, 0, ...deps.glom(messages.filter((message) => replies.has(String(message.id)))));
    return rows;
  }

  /**
   * The messages of the channel's rows, oldest first, however the rows are arranged on screen. What
   * everything that means "the channel's messages in order" reads — a saved snapshot, the boundary of
   * a bulk clear — so that gathering, which only rearranges the rows, cannot change any of them.
   */
  function listedMessages(): GatherMessage[] {
    // `#410 inline-reply-editor`. Less the replies a gathered thread drew in Main, which are its thread's.
    const messages = [...deps.el("discord-log").children].filter((row) => !fromGatheredThread(row)).flatMap(deps.rowMessages);
    return listState().rowsGathered ? inTimeOrder(messages, "timestamp") : messages;
  }

  /**
   * Put the channel's rows in the order they are meant to be in: gathered while a message's replies
   * are, in time order otherwise. The first thing `renderChannelRows` does, because every path that
   * draws rows ends there — a newest page, a delta, a step back, a live arrival, the combining setting
   * — and each of those draws in time order, knowing nothing of this.
   *
   * Rows that come out the same are KEPT, not rebuilt: a row keeps its fold and its open menu, and
   * the row a scroll is being held by stays the same element. Only the rows the gathering splits or
   * joins are drawn again.
   */
  function arrangeChannelRows(): void {
    const parent = gatheredParent();
    if (parent === null && !listState().rowsGathered) return;
    const list = deps.el("discord-log");
    const rows = [...list.children] as HTMLElement[];
    // `#410 inline-reply-editor`. A thread gathered under its root in Main brings its replies, as the
    // thread draws them (`unpackMessages`), as rows of this list while it stands; and they go with it.
    const thread = gatheredThread();
    const own = rows.filter((row) => !fromGatheredThread(row)).flatMap(deps.rowMessages);
    const ids = new Set(own.map((message) => String(message.id)));
    const brought = thread === null ? [] : deps.unpackMessages(thread, deps.unpackHeldMessages(deps.channelCanon(), thread, deps.channelCanon().channel === String(deps.selectedChannelId())), false, deps.unpackFilter())
      .filter((message) => !ids.has(String(message.id)));
    const extra = new Set(brought.map((message) => String(message.id)));
    if (thread !== null && deps.unpackNeedsRead(thread, deps.unpackHeldMessages(deps.channelCanon(), thread, deps.channelCanon().channel === String(deps.selectedChannelId())))) deps.queueUnpackRead(thread.id);
    const messages = inTimeOrder([...own, ...brought], "timestamp");
    let groups = parent === null ? null : gatheredRows(messages, parent);
    // The parent has left the list — Hide read hides it, or a read no longer holds it — and the
    // stack goes with it.
    if (groups === null && parent !== null) gatherState().gathered = null;
    listState().rowsGathered = groups !== null;
    if (groups === null) groups = deps.glom(messages);
    const keyOf = (ids: readonly string[]): string => ids.join(" ");
    const wanted = groups.map((group) => keyOf(group.map((message) => String(message.id))));
    if (wanted.length === rows.length && wanted.every((key, i) => key === keyOf(idsOf(rows[i])))) return;
    const held = new Map(rows.map((row) => [keyOf(idsOf(row)), row]));
    list.replaceChildren(...groups.map((group, i) => {
      const row = held.get(wanted[i] as string) || deps.discordNode(group);
      const brings = group.every((message) => extra.has(String(message.id)));
      if (row.hasAttribute("data-gathered-thread") !== brings) row.toggleAttribute("data-gathered-thread", brings);
      return row;
    }));
  }

  /**
   * Two taps on a reply's arrow: gather the conversation it is in under its head, the topmost message
   * of it the list draws (`conversationHead`), and keep the reply that was tapped where it is on the
   * screen. The message it answers is looked for first, as the jump looks for it, because the walk up
   * starts there.
   */
  function gatherFromReply(reply: HTMLElement): Promise<void> | undefined {
    const target = String(reply.getAttribute("data-reply-to") || "");
    const from = String(reply.getAttribute("data-id") || "");
    if (target) return gatherReplies(target, reply, (id) => conversationHead(id, from));
    return undefined;
  }

  /**
   * The N replies chip on `row`: gather that message's replies under it, or, with them gathered
   * already, put them back. A toggle, and it says which it is (`aria-pressed`): a second tap that did
   * nothing would be a control that looks broken.
   */
  function toggleGathered(parent: string, row: HTMLElement): Promise<void> | undefined {
    // `#229 desktop-two-column`. On Main in two columns the replies are the thread on the right: the
    // chip selects it there rather than gathering them here, where they would show twice side by side.
    const root = deps.layoutColumns() && !deps.sideColumn() ? deps.rowMessages(row)[0] : null;
    if (root && threadOf(root) && root.thread && root.thread.is_root === true) { deps.openThread(threadOf(root) as string); return undefined; }
    if (gatheredParent() === String(parent)) {
      scatterReplies();
      return undefined;
    }
    return gatherReplies(String(parent), row);
  }

  /**
   * Find message `parent` and gather a group under the message `headOf` names for it once it is in the
   * list: by default the parent itself, while its row is drawn; for two taps on an arrow, the head of
   * its conversation. Null from `headOf` is "not found yet", and the walk goes on looking. `from` is the
   * row the reader's eye is on — the parent's own for its chip, the reply's for two taps on its arrow —
   * and that row is where it was on the screen afterwards: measured now, before anything moves, and put
   * back once the rows have.
   */
  async function gatherReplies(parent: string, from: HTMLElement, headOf: (id: string) => string | null = (id) => (deps.rowHolding(id) ? id : null)): Promise<void> {
    const anchor = String(from.getAttribute("data-id") || parent);
    const anchorTop = from.getBoundingClientRect().top;
    // A filter that takes rows away would hide replies from the stack the reader just asked for, and
    // putting the replies back is what turning one on does: it is closed, as the jump closes a search
    // that hides the message it is for.
    if (deps.searchOpen()) deps.setSearchOpen(false);
    await deps.seekMessage(parent, () => {
      const head = gatheringView() ? headOf(parent) : null;
      if (head === null) return false;
      gatherNow(head, anchor, anchorTop);
      return true;
    }, () => deps.threading() && deps.selection().view !== "flat");
  }

  /** The gathering itself, once the parent's row is in the list. */
  function gatherNow(parent: string, anchor: string, anchorTop: number): void {
    const report = gatherReport(parent);
    if (report.gathered === 0) {
      deps.setStatus(report.hidden > 0 ? "Hide read is hiding its replies." : "No replies to it are loaded.");
      return;
    }
    gatherState().gathered = { parent, context: deps.channelContextKey() };
    deps.renderChannelRows();
    deps.renderScrollTools();
    keepOnScreen(anchor, anchorTop);
    deps.setStatus(report.hidden > 0 ? `Gathered ${report.gathered}; Hide read hides ${report.hidden} more.`
      : report.missing > 0 ? `Gathered ${deps.countOf(report.gathered)}; ${report.missing} not loaded.`
        : `Gathered ${deps.countOf(report.gathered)} under it.`);
  }

  /**
   * How many of `parent`'s replies, to any depth, are drawn to be gathered, how many Hide read leaves
   * out, and how many more the threads in the group — the parent's, and any a reply heads — hold by
   * their own exact counts than are loaded: see the head of this section.
   */
  function gatherReport(parent: string): { gathered: number; hidden: number; missing: number } {
    const drawn = [...deps.el("discord-log").children].flatMap(deps.rowMessages);
    const shown = new Set(drawn.map((message) => String(message.id)));
    const group = replyGroup(parent, drawn);
    const gathered = [...group.replies].filter((id) => shown.has(id)).length;
    // Every reply not drawn is one the page holds of the view, and in All only Hide read leaves one out.
    const hidden = deps.hidingRead() ? group.replies.size - gathered : 0;
    let missing = 0;
    for (const [thread, head] of group.threads) {
      const said = deps.threadReplies(thread, deps.heldMessage(head) ||
        drawn.find((message) => String(message.id) === head) || null);
      const loaded = deps.timelineRows().messages.filter((message) => threadOf(message) === thread &&
        String(message.id) !== head).length;
      if (said.exact === true && typeof said.count === "number") missing += Math.max(0, said.count - loaded);
    }
    return { gathered, hidden, missing };
  }

  /** Scroll so that the row holding message `id` has its top at `top` on the screen again. */
  function keepOnScreen(id: string, top: number): void {
    const row = deps.rowHolding(id);
    if (row && Number.isFinite(top)) {
      deps.el("scroll-area").scrollTop += row.getBoundingClientRect().top - top;
    }
    deps.renderJumpNewest();
  }

  /** The X beside the bridge: the replies go back to their places in time, and the parent stays put. */
  function scatterReplies(): void {
    const parent = gatheredParent();
    const row = parent === null ? null : deps.rowHolding(parent);
    const top = row ? row.getBoundingClientRect().top : NaN;
    gatherState().gathered = null;
    deps.renderChannelRows();
    deps.renderScrollTools();
    if (parent !== null) keepOnScreen(parent, top);
    deps.setStatus("Replies back in time order.");
  }

  /**
   * Put the replies back without holding anything on the screen: the list is about to be another
   * one — a change of view, which keeps these rows to come back to and must keep them in time order —
   * or a filter is about to take rows out of it.
   */
  function putRepliesBack(): void {
    gatherState().gathered = null;
    if (listState().rowsGathered) deps.renderChannelRows();
  }

  /**
   * Everything in a reply's gutter that depends on the rows around it: which arrow it draws (`#214
   * reply-arrow`), and, while replies are gathered, which rows are the stack, where each sits in it,
   * the bridge and the X (`#215 reply-coalesce`). Run whenever those rows can have changed: after
   * every redraw, and after every filter pass, which changes what the reader can see above a row.
   */
  function renderReplyLinks(): void {
    const list = deps.el("discord-log");
    const rows = [...list.children] as HTMLElement[];
    const parent = gatheredParent();
    const parentRow = parent === null ? null : deps.rowHolding(parent);
    const stack = [];
    if (parentRow) {
      const replies = repliesTo(parent as string, rows.flatMap(deps.rowMessages));
      for (let i = rows.indexOf(parentRow) + 1; i < rows.length; i += 1) {
        const ids = idsOf(rows[i]);
        if (ids.length === 0 || !ids.every((one) => replies.has(one))) break;
        stack.push(rows[i]);
      }
    }
    const under = parentRow ? stackUnder(parentRow) : "";
    // `#405 root-under-summary`. In a thread headed by a summarized card, its root row is the card's
    // first child: one bridged row at the head, drawn as a gathered reply is. A raw card's root is
    // an ordinary row, the card already saying it, so nothing is duplicated.
    const rootId = deps.selection().view === "thread" ? deps.openThreadRootId() : null;
    const rootChild = rootId !== null && deps.rootInward(deps.openThreadChoice())
      ? rows.find((row) => deps.isOpenThreadRoot(row, rootId as string)) || null : null;
    let above = null;
    for (const row of rows) {
      const at = stack.indexOf(row);
      const child = row === rootChild;
      const place = at < 0 ? (child ? "only" : row === parentRow && stack.length > 0 ? "parent" : "")
        : stack.length === 1 ? "only" : at === 0 ? "first" : at === stack.length - 1 ? "last" : "middle";
      if (place) row.setAttribute("data-coalesce", place);
      else row.removeAttribute("data-coalesce");
      if (child) {
        row.setAttribute("data-root-child", "");
        (row as HTMLElement).style.setProperty("--thread-hue", deps.threadHue(String(deps.selection().threadId)));
      } else if (row.hasAttribute("data-root-child")) row.removeAttribute("data-root-child");
      // No stack inset and no bridge for the child here: in a thread its rows stay one width, level
      // with the card, with the link's lane beside them (`#385 thread-line-overlap`); the hue edge
      // (web/voice.css) is what joins it to the card. Unpacked blocks indent theirs (`unpackBlock`).
      if (at >= 0) row.setAttribute("data-stack-under", under);
      else row.removeAttribute("data-stack-under");
      gutterPiece(row, "reply-bridge", at >= 0, bridgePiece);
      gutterPiece(row, "reply-ungather", at === 0, ungatherButton);
      const arrow = childByClass(row, "reply-jump");
      if (arrow) {
        // Not to the line of a collapsed run of read messages, which stands for messages it does not
        // draw: the message answered is behind it. `#221 read-modes`.
        const adjacent = above !== null && above.getAttribute("data-read-run") !== "head" &&
          idsOf(above).includes(String(row.getAttribute("data-reply-to") || ""));
        arrow.setAttribute("data-reply-style", adjacent ? "adjacent" : "disconnected");
        if (!adjacent) {
          arrow.removeAttribute("data-reply-reach");
          arrow.style.removeProperty("--reply-shift");
          arrow.style.removeProperty("--reply-run");
        }
      }
      const chip = childByClass(row, "thread-replies");
      if (chip) {
        // Pressed while its replies are gathered, even when none of them is drawn just now — Hide read
        // may be leaving them all out — because a tap on it then puts the gathering away.
        const pressed = row === parentRow;
        chip.setAttribute("aria-pressed", pressed ? "true" : "false");
        chip.setAttribute("title", pressed ? UNGATHER_NAME : "Gather this message's replies under it");
      }
      if (rowShown(row)) above = row;
    }
    // `#228 reply-visible`. The other end of each arrow, from the arrows just decided.
    deps.renderReplyCounts();
    deps.measureReplyArrows();
  }

  /**
   * `#501 thread-bridge-gutter`. What `row`'s bridge hangs from, drawn: a row unpacked under its
   * card, that thread's card; a gathered reply, the first row above the stack that is not of it —
   * the message the replies were gathered under, which the stack's own drawing puts there. Null
   * where neither stands, as for a row a redraw has taken out of its list.
   */
  function bridgeTop(row: HTMLElement): HTMLElement | null {
    const thread = row.getAttribute("data-unpack-thread");
    if (thread) return deps.cardRow(thread);
    const list = row.parentNode;
    if (!list) return null;
    const rows = [...list.children] as HTMLElement[];
    for (let at = rows.indexOf(row) - 1; at >= 0; at -= 1) {
      const above = rows[at] as HTMLElement;
      if (!above.hasAttribute("data-stack-under")) return above;
    }
    return null;
  }

  /**
   * `#501 thread-bridge-gutter`. Glide the list holding `row` back to what its bridge hangs from
   * (`bridgeTop`), at its head as a landing is put there (`revealRow`): just under the floating
   * line, so the card stands wholly below the heading wherever the list can scroll to it. A glide,
   * smoothly or at once as the stylesheet says for this reader (`glideBehavior`), and measured
   * with every pack slide finished, where the rows are laid out rather than where a slide moment
   * ago drew them (`#460 pack-gap-slide`). Nothing is packed, folded or marked: the stack stays
   * hanging where it was, under its card again.
   */
  function glideToBridgeTop(row: HTMLElement): void {
    const top = bridgeTop(row);
    if (!top || top === row) return;
    let area = row.parentNode as HTMLElement | null;
    while (area && area.id !== "side-scroll" && area.id !== "scroll-area") {
      area = area.parentNode as HTMLElement | null;
    }
    if (!area) return;
    deps.finishPackSlide();
    const to = headOfList(area, top);
    if (typeof area.scrollTo === "function") area.scrollTo({ top: to, behavior: deps.glideBehavior() as ScrollBehavior });
    else area.scrollTop = to;
  }

  /** Where `area` scrolls to put `node` at its head, as a landing is put there: just under the floating line. */
  function headOfList(area: HTMLElement, node: HTMLElement): number {
    return area.scrollTop + deps.drawnTop(node) - deps.floatingClearance(area) - REPLY_LANDING_GAP_PX;
  }

  /**
   * The bridge's part in one gathered reply's gutter: drawn by web/voice.css, and, since
   * `#501 thread-bridge-gutter`, the way back up it: a tap on the line itself glides to what the
   * stack hangs from (`glideToBridgeTop`), where the X beside the first part of it — and the pack
   * triangle the cards dress that X as — keeps the square over the spine for its own press. No
   * control of its own for the keyboard or a screen reader, which reach the card and the parent
   * as rows of the list; the line stays decoration to them, as it was.
   */
  function bridgePiece(row: Element): HTMLElement {
    const bridge = document.createElement("span");
    bridge.className = "reply-bridge";
    bridge.setAttribute("aria-hidden", "true");
    bridge.setAttribute("title", row && row.hasAttribute("data-unpack-thread")
      ? "Back to this thread's card" : "Back to the message these replies hang from");
    bridge.addEventListener("click", (event) => {
      // The row's own tap would fold the reply or read it aloud; this tap is the line's.
      if (event) event.stopPropagation();
      // ...unless it is the click behind a gesture on the row that already acted, a hold or a
      // swipe, whose claim the row's own tap would honour (`tapRow`): one act per finger.
      if (deps.takeClaimedClick()) return;
      // ...or the end of the taps on an arrow that put the bridge here, as with the X.
      if (deps.replyArrows().settling !== null) return;
      const holder = (bridge.parentNode || row) as HTMLElement;
      if (holder) glideToBridgeTop(holder);
    });
    return bridge;
  }

  /**
   * The X, in the first gathered reply's gutter, left of the bridge. A button a touch target square,
   * named for what it does, with its X drawn on an empty span as the arrows are.
   */
  function ungatherButton(): HTMLElement {
    const button = document.createElement("button");
    button.className = "reply-ungather";
    button.setAttribute("type", "button");
    button.setAttribute("aria-label", UNGATHER_NAME);
    button.setAttribute("title", UNGATHER_NAME);
    const mark = document.createElement("span");
    mark.className = "reply-ungather-mark";
    mark.setAttribute("aria-hidden", "true");
    button.append(mark);
    button.addEventListener("click", deps.inThisColumn((event) => {
      // The row's own tap would fold the reply or read it aloud; this tap is the X's.
      if (event) event.stopPropagation();
      // ...unless it is the end of the taps on an arrow that put the X here (`replyArrows().settling`).
      if (deps.replyArrows().settling !== null) return;
      scatterReplies();
    }));
    return button;
  }

  // --- how many replies a message has, and the way down to them ----------------------------------
  //
  // `#228 reply-visible`. The owner, 2026-10-09: "When I press Reply on a message and write a reply it
  // gets marked as read. Fine. But there's still a button that says Reply ... I would like the button
  // to change to say Reply (1), Reply (2) if there are already replies so that it is clear that it is
  // already replied to." And: "a blue circle about the size of our arrow head centered on the lower
  // left corner of the message box ... the other end of the disconnected arrow pointing up/left from
  // the reply lower in the view. If we click it we should jump to the child, just as clicking the


  return {
    gatherState, listState, gatheredParent, gatheredThread, heldMessagesById, replyLinks, repliesTo,
    conversationHead, gatheredRows, listedMessages, arrangeChannelRows, gatherFromReply, toggleGathered,
    gatherReplies, gatherNow, gatherReport, keepOnScreen, scatterReplies, putRepliesBack, renderReplyLinks,
    bridgeTop, glideToBridgeTop, headOfList, bridgePiece, ungatherButton,
  };
}

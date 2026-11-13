// Done's own act and its put back: which loaded messages stand Done in the column worked on, the
// drawing of the moment of marking, the writes kept until the server has them, and what a refused
// Done or put back takes back (`#624 voice-page-modularity`, moved from the legacy script). Words,
// order and timing are as they were in the script; the Undo offer and the inbox ledger are made in
// the script and handed in, as their factories live in layers this one may not import.

import { createTodoView } from "../../core/channels/index.ts";
import type { TodoView } from "../../core/channels/index.ts";
import { createSerialQueue } from "../../foundation/index.ts";
import { createArming } from "./arming.ts";
import { DISMISS_BATCH } from "./channelrows.ts";
import type { ClockPort } from "../../ports/index.ts";

/** A column's record of what it shows as Done. */
export interface DoneState { archived: Set<string> }

/** A dismissal remembered for Undo, as this owner reads it. */
export interface Dismissal {
  channel?: unknown; messages: string[]; bump?: unknown; name?: string | null; act?: unknown; at?: unknown;
  [key: string]: unknown;
}

/** The Undo offer, as this owner uses it (view/shell `createUndoOffer`). */
export interface DismissalOffer {
  current(): Dismissal | null;
  set(offer: Dismissal | null): void;
}

/** One write left over by a page that went, as this owner reads it. */
export interface LeftOverWrite { kind: string; channel: string; messages: readonly string[]; owner: string; at: number }

/** The inbox write ledger, as this owner uses it (core/outbox `createInboxWriteLedger`). */
export interface DoneLedger {
  add(kind: string, channel: unknown, messages: readonly string[], owner: string, at?: number): unknown;
  settle(kept: unknown): void;
  beat(): void;
  takeLeftOver(owner: string): LeftOverWrite[];
}

/** A read's answer, as this owner reads it. */
export interface DonePayload { dismissed?: unknown[]; messages?: unknown[]; upstream_read_notice?: unknown }

/** What a dismissal asks for: named messages, or through a message. */
export interface DismissBody { messages?: readonly unknown[]; through?: unknown; [key: string]: unknown }

type Row = Element;
type Ids = readonly string[];

/** What Done's act asks of the page. */
export interface DoneActDependencies {
  api(path: string, options?: unknown): Promise<DonePayload>;
  clockOnUse(): ClockPort;
  dismissStatus(): void;
  doneHeldHere(ids: Ids): boolean;
  doneSettlesHere(): unknown;
  el(id: string): HTMLElement & { hidden: boolean };
  followDoneEverywhere(ids: Ids, column: unknown): void;
  guardQuietly(fn: () => unknown): () => void;
  heldDonePart(held: unknown, ids: string[]): unknown;
  hidingRead(): boolean;
  holdingMessage(act: () => void): void;
  holdingReplyArrows(act: () => void): void;
  idsOf(row: unknown): string[];
  inColumn(column: unknown, act: () => void): void;
  inOtherColumn(act: () => void): void;
  listState(): { backlog: number; rowsGathered: boolean };
  listedMessages(): { id: unknown }[];
  loadTodo(options: unknown): Promise<void>;
  markHeldDone(ids: string[], done: boolean, settles?: unknown): unknown;
  messageText(message: unknown): string;
  nextUndoAt(): unknown;
  pinsView(): { only: boolean };
  readSpeaksFor(payload: unknown, id: string): boolean;
  refreshAfterLiveMutation(): void;
  renderCachedTimeline(): void;
  renderChannelRows(): void;
  renderChannelSeam(summary: unknown): void;
  renderReadModeControl(): void;
  renderReplyLinks(): void;
  renderScrollTools(): void;
  renderSideColumn(): void;
  retryingWrite<T>(fn: () => Promise<T>, opts: unknown): Promise<T>;
  rowMessages(row: unknown): unknown[];
  saveChannelScope(): void;
  selectedChannelId(): string;
  selection(): { view: string };
  sendBookmark(staged: unknown, opts: unknown): unknown;
  setStatus(text: string): void;
  settleHeldDone(held: unknown, done: boolean): void;
  singleDoneBump(messages: readonly unknown[]): { channel: unknown; to: unknown } | null;
  stageBookmark(channel: unknown, to: unknown): unknown;
  statusSays(text: string): boolean;
  syncUnreadReplies(): void;
  threadForMessageId(id: unknown): string | null;
  timeOf(message: unknown, field: string): number;
  todoSummary(): unknown;
  token(): string;
  tokenFingerprint(token: string): string;
  undoLabel(dismissal: Dismissal | null): string;
  undoName(text: string): string;
  voiceTranscript(): { CLEAR_ARMED_MS: number };
  writeInBatches(route: string, ids: string[], landed: string[]): Promise<string[]>;
  channelStore(): { current(): { channel: string; dismissed: Set<string> } };
  providerFacts: { threading(): boolean };
  columnWork(): { active(): { done: DoneState }; side(): boolean };
  columns(): { side: { mode: string } };
  readModes(): { hiding(): boolean; mode(): string };
  readLedger(): { writtenIds(): Iterable<string> };
  lastDismissal(): DismissalOffer;
  inboxLedger(): DoneLedger;
}

/** Done's act: the state, the drawing, and the writes. */
export interface DoneAct {
  freshDoneState(): DoneState;
  doneState(): DoneState;
  noteArchived(payload: DonePayload, replace: boolean): void;
  dismissalName(ids: Ids): string | null;
  drawUndoLabel(dismissal: Dismissal | null): void;
  renderTodoControls(): void;
  backlogIsArmed(): boolean;
  disarmBacklog(): void;
  armBacklog(): void;
  todoView: TodoView;
  toggleArchived(ids: Ids, row?: Row | null): Promise<void>;
  markRowDone(ids: Ids, row?: Row | null): Promise<void>;
  forgetDoneFlash(ids: Ids): boolean;
  drawDoneFlash(row: Row): void;
  drawDoneFlashes(): void;
  unflashIds(ids: Ids): void;
  unflashDone(row: Row | null): void;
  flashDone(row: Row | null): void;
  refreshAfterInboxChange(): Promise<void>;
  renderDoneChange(): void;
  putBackRowsOff(listed: Element[], taken: Set<string>): void;
  queueInboxWrite<T>(task: () => Promise<T>): Promise<T>;
  inboxWriteOwner(): string;
  inboxPost(route: string, body: unknown): Promise<DonePayload>;
  sendLeftOverInboxWrites(): void;
  noteInboxWrite(ids: Ids): void;
  settleInboxWrite(ids: Ids, done: boolean, saved: Set<string>, sent?: boolean): Set<string>;
  noteDoneSaving(ids: Ids): () => void;
  doneSaved(ids: Ids, done?: () => boolean): Promise<boolean>;
  dismissMessages(body: DismissBody, bump?: { channel: unknown; to: unknown } | null): Promise<void>;
  dealtWithWords(count: number): string;
  markReadUpstream(messageId: unknown): Promise<void>;
  restoreMessages(ids: Ids): Promise<void>;
  mirrorArchived(ids: Ids, done: boolean): string[];
  doneSaving: Map<string, Promise<void>>;
  DONE_FLASH_HOLD_MS: number;
  DONE_FLASH_FADE_MS: number;
  INBOX_WAITING_WORDS: string;
}

/**
 * Which LOADED messages the reader has archived, as the server reported them.
 *
 * Held here rather than read off the rows because it is the server's answer, not a fact about the
 * DOM: a row is greyed BECAUSE it is in this set, and the set survives the list being rebuilt by a
 * poll. The ordinary channel view dims these; the To do filter never shows them at all, so in that
 * mode this stays empty and nothing consults it.
 *
 * Only ever the ids in the window on screen — see `ops::dismissed_within`. The store holds every
 * dismissal the channel ever had, and sending the lot would grow without bound.
 *
 * A column's own record (`#624 voice-page-modularity`, plan step 15, as `freshOlderPages`): `archived`,
 * the set just described, a fresh empty one for each column.
 *
 * @returns {{archived: Set<string>}}
 */
export function freshDoneState(): DoneState {
  return { archived: new Set<string>() };
}

/** Done's act over `deps`. */
export function createDoneAct(deps: DoneActDependencies): DoneAct {
  /**
   * Which LOADED messages the reader has archived, as the server reported them.
   *
   * Held here rather than read off the rows because it is the server's answer, not a fact about the
   * DOM: a row is greyed BECAUSE it is in this set, and the set survives the list being rebuilt by a
   * poll. The ordinary channel view dims these; the To do filter never shows them at all, so in that
   * mode this stays empty and nothing consults it.
   *
   * Only ever the ids in the window on screen — see `ops::dismissed_within`. The store holds every
   * dismissal the channel ever had, and sending the lot would grow without bound.
   *
   * A column's own record (`#624 voice-page-modularity`, plan step 15, as `freshOlderPages`): `archived`,
   * the set just described, a fresh empty one for each column.
   *
   * @returns {{archived: Set<string>}}
   */
  function freshDoneState() {
    return { archived: new Set<string>() };
  }

  /** The column being worked on's record of what it shows as Done (`freshDoneState`). */
  function doneState() {
    return deps.columnWork().active().done;
  }

  /**
   * Take down what a page said about which of its messages are archived.
   *
   * `replace` for the newest page, which REPLACES the list; additive for a step back, which prepends
   * to it. Getting that backwards would either forget the archive on every poll or accumulate ids
   * for rows that are no longer anywhere.
   */
  function noteArchived(payload: DonePayload, replace: boolean) {
    // Not over a Done or a put back made here since the read was sent (`readSpeaksFor`, `#222
    // unread-replies`): the read answers for the state before it.
    if (replace) {
      doneState().archived = new Set([...doneState().archived].filter((id: string) => !deps.readSpeaksFor(payload, id)));
    }
    for (const id of payload.dismissed || []) {
      if (deps.readSpeaksFor(payload, String(id))) doneState().archived.add(String(id));
    }
    // `#327 done-races-remaining`. A message the read does not speak for keeps the state of the act made
    // here, which this copy holds only if it was there for it: a view drawn for the first time while a
    // Done or a put back made in another view was on its way — a thread opened on the right while a Done
    // made on Main was saved — held nothing for it, and drew it as the read did, not Done, and so it
    // stayed once that Done was saved. The channel's store took the act at once (`markHeldDone`), so a
    // copy takes its state for each such message from there. The walk visits every message this page has
    // written Done or put back since it was loaded (`localReadWrites` only grows): a few hundred at most in
    // a long session, against a read that has just drawn its page, so it is not bounded further.
    if (deps.providerFacts.threading() && deps.channelStore().current().channel === String(deps.selectedChannelId())) {
      for (const id of deps.readLedger().writtenIds()) {
        if (deps.readSpeaksFor(payload, id)) continue;
        if (deps.channelStore().current().dismissed.has(id)) doneState().archived.add(id);
        else doneState().archived.delete(id);
      }
    }
  }


  /**
   * The name a dismissal of `ids` gives its Undo: the message's opening words when the ids are ONE
   * row's (a Done, a swipe), or null when they span rows (Done through here, Clear the backlog), which
   * the chip then counts instead. Read off the row while it is still drawn: under Hide read it is gone
   * the moment the dismissal lands.
   */
  function dismissalName(ids: Ids) {
    const rows = [...deps.el("discord-log").children, ...deps.el("pinned-log").children];
    const row = rows.find((one: Element) => deps.idsOf(one).includes(ids[0]!));
    if (!row || !ids.every((id: string) => deps.idsOf(row).includes(id))) return null;
    const [first] = deps.rowMessages(row);
    return first ? deps.undoName(deps.messageText(first)) || null : null;
  }

  /**
   * Draw the Undo chip's words. The name and its quotation marks are in a span of their own, which
   * web/voice.css leaves out on a row of chips too narrow for a name, where the chip says only "Undo";
   * inside it the name is in a span of its own again, which the stylesheet cuts short with an ellipsis
   * to the width the row has left, the quotation marks outside it so the cut reads as a cut. Whatever
   * the chip shows, the whole label is its accessible name and its tooltip, so a screen reader and a
   * long press have the whole name: set as attributes, never as markup, as a link's address is
   * (`linkList`).
   */
  function drawUndoLabel(dismissal: Dismissal | null) {
    const label = deps.el("undo-dismiss-label");
    const button = deps.el("undo-dismiss");
    const words = deps.undoLabel(dismissal);
    button.setAttribute("title", words);
    label.replaceChildren();
    if (dismissal === null || !dismissal.name) {
      button.removeAttribute("aria-label");
      label.textContent = words;
      return;
    }
    button.setAttribute("aria-label", words);
    const name = document.createElement("span");
    name.className = "undo-name";
    name.textContent = dismissal.name;
    const named = document.createElement("span");
    named.className = "undo-named";
    named.append(document.createTextNode("“"), name, document.createTextNode("”"));
    label.replaceChildren(document.createTextNode("Undo "), named);
  }

  // How many the backlog control is about to clear, so it can say so before it does it. Kept per column: `deps.listState().backlog` (`freshListState`, `#624 voice-page-modularity`).

  function renderTodoControls() {
    // The read mode, said by the button in its word, its icon and its pressed state. `#221
    // read-modes`: three modes, so the word names the mode the list is in — never the act, which
    // would be the ambiguity every mute button in history has had, with a third state to be wrong
    // about. web/voice.css draws each mode off the same attribute a screen reader is told.
    deps.renderReadModeControl();
    deps.el("inbox-note").hidden = !deps.hidingRead();
    const clear = deps.el("clear-backlog");
    clear.hidden = deps.providerFacts.threading() || !deps.readModes().hiding() || deps.listState().backlog === 0;
    clear.textContent = backlogIsArmed()
      ? `Clear ${deps.listState().backlog}?`
      : `Clear the backlog (${deps.listState().backlog})`;
    clear.className = backlogIsArmed() ? "chip armed" : "chip";
    deps.el("undo-dismiss").hidden = deps.lastDismissal().current() === null;
    drawUndoLabel(deps.lastDismissal().current());
  }

  // Bulk and destructive, so it asks twice — the same armed idiom the Clear control on the dock
  // uses, and the same window, because a reader who has learnt one has learnt the other
  // (web/src/view/channel `createArming`, `#624 voice-page-modularity`).
  const backlogArming = createArming({
    clock: deps.clockOnUse(),
    ms: () => deps.voiceTranscript().CLEAR_ARMED_MS,
    drawn: () => renderTodoControls(),
  });
  const backlogIsArmed = () => backlogArming.armed();

  function disarmBacklog() {
    backlogArming.disarm();
  }

  function armBacklog() {
    backlogArming.arm();
  }


  /**
   * What the last `/todo` read said, held so that anything which changes the list can RESTATE it.
   *
   * Kept rather than recomputed from the payload at the call site, because the payload is not the
   * only thing that changes the list: a message arriving on the live stream adds a row without any
   * read at all, and the head of the list, the backlog count and the bulk control all have to move
   * with it or the view contradicts itself. One owner in web/src/core/channels (`createTodoView`, `#624
   * voice-page-modularity`).
   */
  const todoView = createTodoView();



  /**
   * Put one row at the end of the channel list, and keep everything that DESCRIBES the list in step.
   *
   * TWO paths add a row without a read: a message arriving on the live stream, and a reply this page
   * has just posted. Both are the same fact — the list on screen is now longer than the last `/todo`
   * answer said — and both are wrong in the same way if only the row moves. In to-do mode a message
   * that has just arrived has by definition not been dealt with, so it is one more thing to do, and
   * the head of the list, the backlog count and the bulk control have to say so in the same moment
   * the row appears. Otherwise the view contradicts itself: "nothing left to deal with" written
   * directly above a message, a bulk control hidden while there is something to clear, or a count
   * that says two over a list of three — which `clearBacklog` would then clear all three of.
   *
   * Outside the mode this is an ordinary append, because nothing on screen is claiming a count.
   *
   * `live` says which of the two it is: only the stream's copy can leave a message's thread unknown.
   */


  /**
   * Archive or unarchive ONE row, whichever it is asking for.
   *
   * Both the swipe and the row's button come through here, so the two cannot drift into two notions
   * of what the gesture means.
   */
  async function toggleArchived(ids: Ids, row: Row | null = null) {
    const all = ids.map(String);
    // The row's own reading of itself, and it has to be the SAME `every` the row is drawn with:
    // a half-archived row shows as unread, so the act it offers is Done, and Done must archive the
    // part that is still outstanding rather than putting the whole row back.
    if (all.length > 0 && all.every((id: string) => doneState().archived.has(id))) {
      await restoreMessages(all);
    } else {
      await markRowDone(all, row);
    }
  }

  /**
   * Mark ONE row's messages Done: the half of `toggleArchived` that marks, and the Done of Shift+E's read
   * (`#295 shift-e-read-collapse`), so the key's Done cannot drift from the Done button's. The page's
   * state, the server's record and the bookmark's slide are `dismissMessages`'s, the same for every way
   * in. The chat service's own read marker is not this function's: Shift+E asks for it after this has
   * taken (`markReadInService`), as a sent reply does; `e` and the Done button leave it.
   */
  async function markRowDone(ids: Ids, row: Row | null = null) {
    // `#245 done-feedback`. BEFORE the dismissal redraws the row as read, so the change is one the
    // browser can animate rather than a jump; taken back if the Done did not take.
    flashDone(row);
    try {
      await dismissMessages({ messages: ids });
    } catch (error) {
      unflashDone(row);
      throw error;
    }
  }

  // --- what marking a message Done looks like ------------------------------------------------------
  //
  // `#245 done-feedback`. The owner likes that a read message lights up again when it is selected, and
  // asked for "SOME indication of what just happened" when he marks the selected one read: the keys
  // move the selection straight on to the next unread message (`#224`), so the row he acted on went
  // from lit to greyed out of the corner of his eye, and a lit read row looked exactly like a lit
  // unread one.
  //
  //   * ONE RULE WHEREVER A ROW IS SELECTED (web/voice.css): a selected unread message is ringed in the
  //     accent, a selected read one in grey, on the same lit surface so it is still easy to read. The
  //     ring is the state, so it is the same after a Done, after a reload, and under the mouse.
  //   * THE MOMENT OF MARKING is drawn on the row that was marked, even though the selection has moved
  //     on: its ring turns from blue to grey over a quarter of a second while the row fades to the
  //     read look, holds there, then lets the ring go. That is `data-done-flash`, "marking" and then
  //     "fading", which web/voice.css animates FROM the selected unread look; an animation rather than
  //     a transition, because on a threaded channel a redraw from the store can draw the row as a new
  //     element, which a transition has no earlier style to start from. A row still selected keeps its grey ring: the
  //     fading phase lets go only of a ring the selection is not drawing anyway.
  //   * Every way of marking comes through it: the Done button, the e key and a swipe through
  //     `toggleArchived`, Shift+E's read and collapse (`markRowDone`), Done through here (the ⋯ menu)
  //     on the row it was asked from, and a sent reply's Done (`markAnsweredDone`) on the row standing
  //     for the message it answers.
  //   * EVERY WAY BACK ENDS IT AT ONCE, though its timers have most of a second to run: a Done the
  //     server refused, the Undo chip, z, the row's own button, or another device putting the message
  //     back. A row drawn unread never wears the grey ring (`drawDoneFlash`).
  //   * UNDER REDUCED MOTION the change is instant (web/voice.css); the grey ring still holds, so the
  //     reader still sees which row it was.
  //   * The way back is the Undo chip that was already there, and it now names the message it would
  //     put back (`undoLabel`). There is no second Undo.

  /** How long the grey ring stays on a row just marked Done, the quarter-second change included. */
  const DONE_FLASH_HOLD_MS = 900;

  /** ...and how long its letting go takes, after which the row is an ordinary read row. */
  const DONE_FLASH_FADE_MS = 300;

  /**
   * Which messages are being drawn as just marked Done, and in which phase: "marking" or "fading".
   * Kept BY ID, not on the row, because on a threaded channel a redraw from the store
   * (`renderCachedTimeline`: under Hide read, a read, a mode change) draws every row as a new element, and
   * may do so while the moment runs. `renderChannelRows` puts the phase on whichever row now stands for
   * the message. (A Done itself dresses the rows where they stand since `#892 voice-page-optimistic-done`.)
   */
  const doneFlashing = new Map();

  /**
   * Each flashing row's messages and the timer its phase is waiting on, under the row's first id: a
   * second Done on the row starts it again, and a put back finds it by any of its messages.
   *
   * @type {Map<string, {ids: string[], timer: unknown}>}
   */
  const doneFlashGroups = new Map();

  /**
   * Forget the moment of every Done that marked any of `ids`, and its timers. Answers whether there was
   * one. Draws nothing; see `drawDoneFlashes`.
   *
   * @param {string[]} ids
   */
  function forgetDoneFlash(ids: Ids) {
    const named = new Set(ids.map(String));
    let found = false;
    for (const [key, group] of [...doneFlashGroups]) {
      if (!group.ids.some((id: string) => named.has(id))) continue;
      clearTimeout(group.timer);
      doneFlashGroups.delete(key);
      for (const id of group.ids) doneFlashing.delete(id);
      found = true;
    }
    for (const id of named) found = doneFlashing.delete(id) || found;
    return found;
  }

  /**
   * Draw `row`'s phase, or nothing: what `renderChannelRows` does for every row it draws.
   *
   * A ROW DRAWN UNREAD IS NEVER DRAWN AS JUST MARKED DONE, whatever the timers still say: its message
   * was put back — by the Undo chip, z, the row's own button, a Done the server refused, or another
   * device, whose put back arrives with a read — and a grey ring would say it is read. Its moment is
   * forgotten there and then.
   */
  function drawDoneFlash(row: Row) {
    const ids = deps.idsOf(row);
    let phase = ids.map((id: string) => doneFlashing.get(id)).find(Boolean);
    if (phase && row.getAttribute("data-archived") !== "true") {
      forgetDoneFlash(ids);
      phase = undefined;
    }
    if (phase) row.setAttribute("data-done-flash", phase);
    else if (row.hasAttribute("data-done-flash")) row.removeAttribute("data-done-flash");
  }

  /** Every drawn row of every list brought into line with `doneFlashing`, after a phase changes. */
  function drawDoneFlashes() {
    // `#407 unpack-row-parity`. And the rows unpacked under the cards, which are message rows too.
    for (const id of ["discord-log", "side-log", "pinned-log", "side-cards"]) {
      const list = document.getElementById(id);
      for (const row of list ? [...list.children] : []) drawDoneFlash(row);
    }
  }

  /**
   * Take the moment back from messages that are unread again — a Done the server refused, or any put
   * back (`restoreMessages`) — and from every row drawing it, at once rather than at the next redraw.
   *
   * @param {string[]} ids
   */
  function unflashIds(ids: Ids) {
    if (forgetDoneFlash(ids)) drawDoneFlashes();
  }

  /** ...the messages of `row`. */
  function unflashDone(row: Row | null) {
    if (row) unflashIds(deps.idsOf(row));
  }

  /** Draw the moment `row` was marked Done; see the section note. Nothing without a row. */
  function flashDone(row: Row | null) {
    const ids = row ? deps.idsOf(row) : [];
    if (ids.length === 0) return;
    forgetDoneFlash(ids);
    const group: { ids: Ids; timer: unknown } = { ids, timer: null };
    const phase = (value: string | null) => {
      for (const id of ids) {
        if (value) doneFlashing.set(id, value);
        else doneFlashing.delete(id);
      }
    };
    phase("marking");
    doneFlashGroups.set(ids[0], group);
    // On the row as it is NOW as well, before the Done redraws it: where the row is kept, the change
    // to the read look then happens under the attribute rather than a frame before it. Set directly,
    // because the row is still drawn unread, which `drawDoneFlash` takes for a put back.
    row!.setAttribute("data-done-flash", "marking");
    group.timer = setTimeout(() => {
      phase("fading");
      drawDoneFlashes();
      group.timer = setTimeout(() => {
        phase(null);
        doneFlashGroups.delete(ids[0]);
        drawDoneFlashes();
      }, DONE_FLASH_FADE_MS);
    }, DONE_FLASH_HOLD_MS);
  }

  /**
   * What an archive or an unarchive does to the list, which is NOT the same in the two modes.
   *
   * In the To do filter the row genuinely leaves, so the list has to be re-read. In the channel view
   * it stays and merely changes colour, so re-deriving the rows already on screen is the whole of
   * it — and doing that instead of a re-read is why a swipe greys the row instantly rather than
   * after a round trip to Discord.
   */
  async function refreshAfterInboxChange() {
    // `#407 unpack-row-parity`. An act on a row unpacked under the cards is made in the right column,
    // which draws no thread of its own while it shows them: what changed is drawn in the cards, and its
    // left-over thread is neither drawn nor written over the store.
    if (deps.columnWork().side() && deps.columns().side.mode !== "thread") {
      deps.renderSideColumn();
      return;
    }
    if (deps.providerFacts.threading()) {
      renderDoneChange();
      deps.saveChannelScope();
      return;
    }
    if (deps.readModes().hiding()) {
      const kept = [...deps.el("discord-log").children].filter((row: Row) => {
        const ids = deps.idsOf(row);
        return ids.length === 0 || !ids.every((id: string) => doneState().archived.has(id));
      });
      deps.el("discord-log").replaceChildren(...kept);
      // A row gone from the list is a row gone from above another (`#214 reply-arrow`): the arrows are
      // decided again. Gathered replies (`#215 reply-coalesce`) have their stack drawn again, by the
      // whole pass, which puts them back in time order if what went was their parent — before the
      // rows are counted, since that can join two of them.
      if (deps.listState().rowsGathered) deps.renderChannelRows();
      else deps.renderReplyLinks();
      deps.listState().backlog = deps.el("discord-log").children.length;
      todoView.setLeft(deps.listState().backlog);
      deps.renderChannelSeam(deps.todoSummary());
      renderTodoControls();
      // The Pinned filter's rows are drawn from the archive too: a pin dealt with there greys there.
      if (deps.pinsView().only) deps.renderChannelRows();
      // `#240 reading-bookmark`. The line was drawn on a row this took away, and a Done right under it
      // has just moved it: drawn again, with the chips, which no redraw of the rows does here.
      deps.renderScrollTools();
    } else {
      deps.renderChannelRows();
      deps.saveChannelScope();
    }
  }

  /**
   * A Done or a put back, drawn on a threaded channel's list (`#892 voice-page-optimistic-done`). It changes
   * which rows the list holds only under Hide read; otherwise every row stays, and dressing the rows on screen
   * again (`renderChannelRows`), the reader held on the same message, is the whole of it. Rebuilding every row
   * from the store (`renderCachedTimeline`), as every Done did before, put a whole-list redraw in the key's own
   * task: about 100 ms of every e and Shift+E over 300 rows. Without threads, nothing (`renderCachedTimeline`).
   */
  function renderDoneChange() {
    if (!deps.providerFacts.threading() || deps.hidingRead() || deps.selection().view === "threads") {
      deps.renderCachedTimeline();
      return;
    }
    deps.holdingMessage(() => deps.holdingReplyArrows(deps.renderChannelRows));
    deps.renderScrollTools();
  }

  /**
   * `#327 done-races-remaining`. Under Hide read without threads a Done takes its row off the list at once
   * (`refreshAfterInboxChange`), and nothing else holds that row: the list is the to-do read's, and no
   * store keeps its messages. So a Done the server refused left the row off the list until the next read.
   * `listed` is the list as the Done found it, and `taken` the messages the refusal took back; each row of
   * `listed` holding one of those, whose messages are not all Done now, and none of which the list draws
   * again already, goes back where it stood (never a row a read took off since, dealt with elsewhere):
   * before the first row that stood below it and is drawn now, else after the last that stood above it,
   * else among the rows by when its first message was sent. Drawn now is by message, not by element: a
   * to-do read that landed while the Done was on its way (the poll) drew every row afresh, and none of
   * `listed` is on the list itself. Such a read, the Done not yet taken, also listed the message again,
   * and drew its row Done: that row is drawn again as it is now.
   *
   * @param {Element[]} listed
   * @param {Set<string>} taken
   */
  function putBackRowsOff(listed: Element[], taken: Set<string>) {
    const list = deps.el("discord-log");
    const drawn = new Set([...list.children].flatMap(deps.idsOf));
    const back = listed.filter((row: Row) => row.parentNode !== list && deps.idsOf(row).some((id: string) => taken.has(id)) &&
      !deps.idsOf(row).some((id: string) => drawn.has(id)) && !deps.idsOf(row).every((id: string) => doneState().archived.has(id)));
    /** The row on the list now that draws a message of `row`, which stood in `listed`, or undefined. */
    const drawing = (row: Row) => {
      const ids = deps.idsOf(row);
      return [...list.children].find((one: Element) => deps.idsOf(one).some((id: string) => ids.includes(id)));
    };
    for (const row of back) {
      const at = listed.indexOf(row);
      let next = /** @type {Element | null | undefined} */ (undefined);
      for (const below of listed.slice(at + 1)) {
        next = drawing(below);
        if (next) break;
      }
      if (!next) {
        for (const above of listed.slice(0, at).reverse()) {
          const found = drawing(above);
          if (!found) continue;
          const now = [...list.children];
          next = now[now.indexOf(found) + 1] || null;
          break;
        }
      }
      if (next === undefined) {
        const sent = deps.timeOf(deps.rowMessages(row)[0], "timestamp");
        next = [...list.children].find((one: Element) => deps.timeOf(deps.rowMessages(one)[0], "timestamp") > sent) || null;
      }
      list.insertBefore(row, next);
    }
    const stale = [...list.children].some((one: Element) => deps.idsOf(one).length > 0 &&
      (one.getAttribute("data-archived") === "true") !== deps.idsOf(one).every((id: string) => doneState().archived.has(id)));
    if (stale) deps.renderChannelRows();
  }

  // Keep writes ordered while every visible result happens immediately. This prevents a quick run
  // of Done taps from occupying every provider-adapter worker even against an older server. The queue
  // is web/src/foundation's (`createSerialQueue`, `#624 voice-page-modularity`), made where its tail stood.
  const inboxWrites = createSerialQueue();

  /**
   * @template T
   * @param {() => Promise<T>} task
   * @returns {Promise<T>}
   */
  function queueInboxWrite<T>(task: () => Promise<T>): Promise<T> {
    return inboxWrites.run(task) as Promise<T>;
  }

  // --- Done and its put back, kept until the server has them ---------------------------------------
  //
  // `#892 voice-page-optimistic-done`. The owner: "This comes back to our model of local state plus
  // synchronization." A Done or a put back is drawn the moment it is made, and its write is kept on the
  // device until the server has answered it (web/src/core/outbox `createInboxWriteLedger`). A write the
  // server never heard (offline, a dropped connection, a proxy answering for a restarting server) goes again
  // by itself, 1 s, then doubling to every 30 s, in order, the writes behind it waiting behind it
  // (`inboxPost`); the status line says so once, and takes it back once it is answered. A page closed
  // meanwhile sends what it kept when it next opens, or once the connection comes back, and reads the channel
  // again once the server has it (`sendLeftOverInboxWrites`); a page still open on the device, sending its own,
  // is left to send them, and a page going hands its own over at once (`deps.inboxLedger().leave`). What the server
  // answers and refuses is taken back as it always was. Writing a Done twice is safe where sending a message
  // twice is not: the server keeps a set.


  /** Whose writes are made now: the credential's fingerprint, never the credential. */
  function inboxWriteOwner() {
    try {
      return deps.tokenFingerprint(deps.token());
    } catch (_error) {
      return "";
    }
  }

  /** What the status line says while a write the server has not answered waits to go again. */
  const INBOX_WAITING_WORDS = "Kept on this device: the server has not answered, and it goes again by itself.";

  /**
   * POST `body` to `route`, again by itself while the server does not answer (`retryingWrite`): the server's
   * answer, or its refusal thrown. Said once on the status line while it waits, and taken back once answered,
   * if nothing was said since.
   *
   * @param {string} route
   * @param {unknown} body
   */
  async function inboxPost(route: string, body: DismissBody) {
    let waited = false;
    try {
      const answer = await deps.retryingWrite(() => deps.api(route, { method: "POST", body }), {
        wait: (ms: number) => new Promise<void>((resolve) => { deps.clockOnUse().setTimeout(resolve as () => void, ms); }),
        waiting: () => {
          // Still here, and still sending: another page open on the device leaves these writes to this one.
          deps.inboxLedger().beat();
          if (!waited) deps.setStatus(INBOX_WAITING_WORDS);
          waited = true;
        },
      });
      // Answered after the server had gone quiet: the writes a page that went meanwhile left can go too.
      if (waited) sendLeftOverInboxWrites();
      return answer;
    } finally {
      if (waited && deps.statusSays(INBOX_WAITING_WORDS)) deps.dismissStatus();
    }
  }

  /**
   * Send the writes a page closed before the server answered, oldest first, behind any made since, and read
   * the channel on screen again once the server has one of its own (`#892 voice-page-optimistic-done`). Each
   * is kept until answered, as a write made here is. Only this credential's, and none older than a day.
   */
  function sendLeftOverInboxWrites() {
    for (const left of deps.inboxLedger().takeLeftOver(inboxWriteOwner())) {
      const kept = deps.inboxLedger().add(left.kind, left.channel, left.messages, left.owner, left.at);
      const route = `/api/v1/channels/${encodeURIComponent(left.channel)}/${left.kind}`;
      queueInboxWrite(() => deps.writeInBatches(route, [...left.messages], []))
        .then(() => {
          if (String(deps.selectedChannelId()) === left.channel) deps.refreshAfterLiveMutation();
        }, () => {})
        .finally(() => deps.inboxLedger().settle(kept));
    }
  }

  // --- what a refused Done or put back takes back --------------------------------------------------
  //
  // `#326 refused-undo-restores-done`. Found in the final reviews of #306 done-stale-across-views and
  // #311 sent-read-waits-for-done. A Done pressed, then Undo while the Done was still being saved: the
  // Undo's write waits behind the Done's (`queueInboxWrite`). Each refusal took back exactly what its own
  // act had changed, so with both refused, the Undo's took the message back to Done — a Done the server
  // had refused as well. Every view drew it Done while the server held none, the next write saved it into
  // the device's snapshot, and a Reply on it wrote no Done and asked the chat service, whose marker cannot
  // be moved back, to mark it read after_reply.
  //
  // So a refused write takes a message back to what the SERVER holds for it by this page's writes: the
  // last of them it saved, or, none saved, what the page held before the first. Writes settle in the order
  // they were made, so that is known by the time each is answered. Refused while a later write of the
  // message is still on its way, it takes nothing back: that later act is what is drawn, and its own
  // answer settles it. A refused Undo of a refused Done leaves the message not Done; a refused Done
  // pressed again over a refused Undo of a saved one leaves it Done.

  /**
   * By message id, how many of this page's Done and put-back writes naming it are on their way (`writes`),
   * and whether the server holds it Done by them (`server`): by the last of them it saved, or as the page
   * held it before the first (`noteInboxWrite`). `#326 refused-undo-restores-done`.
   *
   * @type {Map<string, {writes: number, server: boolean}>}
   */
  const inboxWritesOnTheWay = new Map();

  /**
   * Note a Done or a put back of `ids` as on its way (`inboxWritesOnTheWay`). Called as the act is made,
   * before it changes anything here: a message no write is on its way for already is held by the server
   * as the page holds it now (`doneHeldHere`).
   *
   * @param {string[]} ids
   */
  function noteInboxWrite(ids: Ids) {
    for (const id of ids) {
      const write = inboxWritesOnTheWay.get(id) || { writes: 0, server: deps.doneHeldHere([id]) };
      write.writes += 1;
      inboxWritesOnTheWay.set(id, write);
    }
  }

  /**
   * The write of `ids`, a Done when `done` or else a put back, has been answered; `saved`, the ids the
   * server took. Answers the refused ids whose take-back is left out (`#326 refused-undo-restores-done`):
   * each one a later write is on its way for, whose act is what is drawn, and each one the server holds as
   * this write would have made it — a put back of a Done the server refused as well, or a Done over a put
   * back it refused of one it had saved.
   *
   * `sent` false: the write was never queued — a slip in the page before it (`dismissMessages`,
   * `restoreMessages`) — so it is only no longer on its way, and nothing is left out. The server never
   * heard of it, and it settles out of the order writes are answered in: one made before it may still be
   * on its way, and decides what the server holds. Its act takes back exactly what it changed.
   *
   * @param {string[]} ids
   * @param {boolean} done
   * @param {Set<string>} saved
   * @param {boolean} [sent]
   * @returns {Set<string>}
   */
  function settleInboxWrite(ids: Ids, done: boolean, saved: Set<string>, sent: boolean = true) {
    /** @type {Set<string>} */
    const stands = new Set<string>();
    for (const id of ids) {
      const write = inboxWritesOnTheWay.get(id);
      if (!write) continue;
      write.writes -= 1;
      if (!sent) {
        // Never written: no answer to learn from, and nothing of it to keep.
      } else if (saved.has(id)) {
        write.server = done;
      } else {
        if (write.writes > 0) stands.add(id);
        else if (write.server === done) stands.add(id);
      }
      if (write.writes === 0) inboxWritesOnTheWay.delete(id);
    }
    return stands;
  }

  /**
   * `#295 shift-e-read-collapse`. The Done of each message still on its way to the server, by message
   * id: settled, saved or not, once its write has settled and what a failure takes back is taken back.
   * `dismissMessages` marks a message Done here at once, before its write is even sent, so being Done
   * here does not say the Done is saved; what may follow only a saved Done waits on this (`doneSaved`).
   * `#327 done-races-remaining`: a put back on its way too (`restoreMessages`), since a Done looked at
   * while one is on its way may yet stand — a put back the server refuses takes it back to Done — or not.
   *
   * @type {Map<string, Promise<void>>}
   */
  const doneSaving = new Map();

  /**
   * Note `ids`' Done, or put back, as on its way (`doneSaving`), and answer what to call once it has
   * settled.
   *
   * It takes the place of a Done, or put back, of the same message still on its way, and is waited for
   * in its stead: so its wait ends only once each one it took the place of has settled as well, however
   * soon it settles itself (`#327 done-races-remaining`). As first written it ended at once, and an Undo
   * that slipped in the page before its write, pressed while the Done it undid was on its way, left
   * nothing here for that Done: a sent reply, Through here and Shift+E asked the chat service at once,
   * before that Done was answered, and so whatever the server then did with it. A Done naming a message
   * whose earlier Done is still on its way (Clear the backlog, a row half Done) that slipped did the same.
   *
   * @param {string[]} ids
   * @returns {() => void}
   */
  function noteDoneSaving(ids: Ids) {
    let settle: () => void = () => {};
    /** @type {Promise<void>} */
    const saving = new Promise<void>((resolve) => { settle = resolve as () => void; });
    // What this takes the place of: none, or acts made before it, so no wait here is ever on itself.
    const before = [...new Set(ids.map((id: string) => doneSaving.get(id)).filter(Boolean))];
    for (const id of ids) doneSaving.set(id, saving);
    const end = () => {
      for (const id of ids) if (doneSaving.get(id) === saving) doneSaving.delete(id);
      settle();
    };
    return () => {
      if (before.length === 0) end();
      else Promise.all(before).then(end);
    };
  }

  /**
   * Whether every one of `ids` is Done here with its Done saved: a Done of any of them still on its way
   * to the server waited for first (`doneSaving`), then the ids looked at again, since a Done that
   * failed has put them back, as has a put back (Undo, `e`) made meanwhile. `#295 shift-e-read-collapse`.
   * A put back still on its way is waited for too, and settles it either way (`#327 done-races-remaining`).
   * `done` is that look: by default the column worked on once the wait is over; a confirmed send's
   * (`markAnsweredDone`, `#311 sent-read-waits-for-done`) looks at what the page holds (`doneHeldHere`).
   *
   * @param {string[]} ids
   * @param {() => boolean} [done]
   */
  async function doneSaved(ids: Ids, done: () => boolean = () => ids.every((id: string) => doneState().archived.has(String(id)))) {
    for (;;) {
      const saving = [...new Set(ids.map((id: string) => doneSaving.get(String(id))).filter(Boolean))];
      if (saving.length === 0) return done();
      await Promise.all(saving);
    }
  }

  /**
   * Mark messages as dealt with, remember the exact set, and settle the list.
   *
   * `bump` is what the Done does to the bookmark (`#240 reading-bookmark`): by default, for messages
   * named one by one, the slide of a Done on the message right under the line (`singleDoneBump`),
   * decided before anything changes. It is drawn with the Done, written after it, taken back with it
   * when it fails, and remembered with it for Undo.
   */
  async function dismissMessages(body: DismissBody, bump: { channel: unknown; to: unknown } | null = body.messages ? deps.singleDoneBump(body.messages) : null) {
    const channel = deps.selectedChannelId();
    // In time order, as the server resolves `through`: gathered replies (`#215 reply-coalesce`) are
    // above rows older than they are, and are not "before" them.
    const visible = deps.listedMessages().map((message) => String(message.id));
    const boundary = body.through === undefined ? -1 : visible.indexOf(String(body.through));
    const requested = body.messages
      ? body.messages.map(String)
      : boundary >= 0
        ? visible.slice(0, boundary + 1)
        : [];
    if (requested.length === 0) return;
    // Exactly what this changes, to put back if the write fails: never a copy of the whole set, which
    // would undo what reads folded meanwhile (`#222 unread-replies`).
    const added = requested.filter((id: string) => !doneState().archived.has(id));
    const previousDismissal = deps.lastDismissal().current();
    // `#245 done-feedback`. Named now, while the row is still drawn; see `dismissalName`.
    const name = dismissalName(requested);
    // This Done's own offer of Undo, marked as its own (`act`), which a put back of some of its messages
    // from their rows keeps. `#326 refused-undo-restores-done`: once the write is answered the chip is
    // changed only while it still offers this Done — not once the reader has pressed Undo, or made another
    // Done, while it was being saved, when it came back offering to put back a message already back.
    const act = Symbol("done");
    const at = deps.nextUndoAt();
    const saidAtOnce = dealtWithWords(requested.length);
    deps.lastDismissal().set({ channel, messages: requested, bump, name, act, at });
    const offered = () => deps.lastDismissal().current() !== null && deps.lastDismissal().current()!.act === act;
    // Before anything here changes, so that what the server holds is known if it refuses (`#326`).
    noteInboxWrite(requested);
    for (const id of requested) {
      doneState().archived.add(id);
    }
    // Done here from now on, and saved only once the write below is (`doneSaved`).
    const saved = noteDoneSaving(requested);
    // `#229 desktop-two-column`. Done is the message's, so the other column shows it Done as well.
    const column = deps.columnWork().active();
    // What the lines in the `try` change, for a refusal to take back. `#326 refused-undo-restores-done`:
    // from the notes above on, everything is in the `try`, so a slip in drawing the Done before its write
    // is queued takes back exactly what the Done changed, the ledger told no write went (`sent`), and only
    // then is the wait on it ended. Left out, it left the message Done here with no write on its way, a
    // wait on it (`doneSaved`) that never ended, and the ledger counting a write still on its way, so every
    // later refusal of that message took nothing back. Nor is a plain `finally` enough: the Done still drawn
    // and in the store, a send looking again once the wait ends would find it Done, and ask the chat
    // service, whose marker cannot be moved back.
    /** @type {HeldDoneMark | null} */
    let held: unknown = null;
    /** @type {string[]} */
    let mirrored: string[] = [];
    /** The bookmark's move, staged with the Done. */
    /** @type {ReturnType<typeof stageBookmark> | null} */
    let move: unknown = null;
    /** Whether the Done's write was queued, and the move's behind it (`settleInboxWrite`). */
    let sent = false;
    // `#327 done-races-remaining`. Under Hide read without threads the rows this takes off the list are
    // held nowhere else: kept as they stood, for a refusal to put back (`putBackRowsOff`).
    /** @type {Element[] | null} */
    let listed: Element[] | null = null;
    let payload: DonePayload;
    /** The ids of the batches the server has taken, when there are several. */
    const landed: string[] = [];
    /** The write as kept on the device until answered (`inboxLedger`, `#892 voice-page-optimistic-done`). */
    let kept: unknown = null;
    try {
      held = deps.markHeldDone(requested, true, deps.doneSettlesHere());
      mirrored = mirrorArchived(requested, true);
      // ...and every view held out of sight, and the cards on screen. `#306 done-stale-across-views`.
      deps.followDoneEverywhere(requested, column);
      // Before the redraw, so the line moves with the row it was over; and in the other column, whose own
      // redraw (`mirrorArchived`) came before it, or never came, when it draws none of these messages.
      const staged = bump ? deps.stageBookmark(bump.channel, bump.to) : null;
      move = staged;
      if (staged) deps.inOtherColumn(deps.renderScrollTools);
      listed = deps.readModes().hiding() && !deps.providerFacts.threading() ? [...deps.el("discord-log").children] : null;
      // `#892 voice-page-optimistic-done`. Said now, as drawn now, before anything awaits: Done here does not
      // wait for the server, and a failure is said by the error line. It used to say "Saving" until answered.
      deps.setStatus(saidAtOnce);
      await refreshAfterInboxChange();
      renderTodoControls();
      kept = deps.inboxLedger().add("dismiss", channel, body.messages ? body.messages.map(String) : requested, inboxWriteOwner());
      const writing = queueInboxWrite(async () => {
        const route = `/api/v1/channels/${encodeURIComponent(channel)}/dismiss`;
        if (!body.messages || body.messages.length <= DISMISS_BATCH) {
          return inboxPost(route, body);
        }
        // `#202 read-through-here`: "through here" over a long list names more ids than one request
        // may carry, so it goes in batches, and the undo covers all of them.
        return { messages: await deps.writeInBatches(route, body.messages.map(String), landed) };
      });
      sent = true;
      // Queued straight behind the Done, and sent only once it is saved.
      if (staged) {
        deps.guardQuietly(() => deps.sendBookmark(staged, { conditional: true, after: writing.then(() => true, () => false) }))();
      }
      payload = (await writing) as DonePayload;
    } catch (error) {
      // `#892 voice-page-optimistic-done`. What was said at once is taken back, if nothing was said since.
      if (deps.statusSays(saidAtOnce)) deps.dismissStatus();
      // Only what the server did not take is taken back: the batches before the one that failed are
      // Done there, so they stay Done here, and are what Undo puts back. The bookmark's move, sent only
      // after the whole write, is taken back, which leaves the line above them all.
      const kept = new Set(landed);
      // `#326 refused-undo-restores-done`: nor what the server holds Done all the same, by an earlier
      // write of this page that it saved, nor what a later act of the reader's, still on its way, drew.
      const stands = settleInboxWrite(requested, true, kept, sent);
      const back = new Set(requested.filter((id: string) => !kept.has(id) && !stands.has(id)));
      const standing = requested.filter((id: string) => !back.has(id));
      if (held) {
        if (standing.length > 0) deps.settleHeldDone(deps.heldDonePart(held, standing), true);
        deps.settleHeldDone(standing.length > 0 ? deps.heldDonePart(held, [...back]) : held, false);
      }
      // A move staged before a slip that came ahead of the write: taken back as a refused Done's is.
      const unsent = sent ? null : move;
      if (unsent) deps.guardQuietly(() => deps.sendBookmark(unsent, { conditional: true, after: Promise.resolve(false) }))();
      if (offered()) deps.lastDismissal().set(landed.length > 0 ? { channel, messages: landed.slice(), bump: null, at } : previousDismissal);
      // Taken back in the column it was made in, and in the other. `#229 desktop-two-column`.
      deps.inColumn(column, () => {
        for (const id of added) if (back.has(id)) doneState().archived.delete(id);
        mirrorArchived(mirrored.filter((id: string) => back.has(id)), false);
        // ...and the rows it took off under Hide read without threads, while that list is still on screen.
        if (listed && deps.readModes().hiding() && !deps.providerFacts.threading() && String(deps.selectedChannelId()) === String(channel)) {
          putBackRowsOff(listed, back);
        }
        deps.guardQuietly(refreshAfterInboxChange)();
      });
      // ...and in every view held, from the store the lines above took it back in; in the left column
      // when it was made in the right one and that has stopped showing its thread since; and in the right
      // column when it was made in the left one and the right one shows its thread again since. The
      // other column's line above reaches neither. `#306 done-stale-across-views`.
      deps.followDoneEverywhere(requested, column);
      renderTodoControls();
      if (landed.length > 0) (error as { landed?: number }).landed = landed.length;
      throw error;
    } finally {
      // Answered, taken or refused: no longer kept on the device to send again.
      if (kept !== null) deps.inboxLedger().settle(kept);
      // Once a failure has been taken back, so what waits on it (`doneSaved`) finds the messages put
      // back. It runs after the lines below, which leave the Done as it is.
      saved();
    }
    settleInboxWrite(requested, true, new Set(requested));
    if (held) deps.settleHeldDone(held, true);
    const stored = (payload.messages || requested).map(String);
    // What the server stored, less what was put back from its row meanwhile.
    if (offered()) {
      const still = new Set(deps.lastDismissal().current()!.messages);
      deps.lastDismissal().set({ channel, messages: stored.filter((id: string) => still.has(id) || !requested.includes(id)), bump, name, act, at });
    }
    // Said again only where the server stored another count than was said, and nothing was said since.
    if (stored.length !== requested.length && deps.statusSays(saidAtOnce)) deps.setStatus(dealtWithWords(stored.length));
    renderTodoControls();
  }

  /** What the status line says of a Done of `count` messages. */
  function dealtWithWords(count: number) {
    return `${count} message${count === 1 ? "" : "s"} marked as dealt with here — not in the source chat service.`;
  }

  /** Move the source provider's monotone read cursor without changing local Done/archive state. */
  async function markReadUpstream(messageId: unknown) {
    const channel = deps.selectedChannelId();
    const payload = await deps.api(
      `/api/v1/channels/${encodeURIComponent(channel)}/upstream-read`,
      {
        method: "POST",
        body: { message_id: String(messageId),
          ...(deps.threadForMessageId(messageId) ? { thread_id: deps.threadForMessageId(messageId) } : {}) },
      }
    );
    deps.setStatus(
      String(payload.upstream_read_notice || "") ||
        "The source chat service marked messages read through this one."
    );
  }

  /**
   * Put named messages back, one row at a time.
   *
   * The undo chip is a different act and keeps its own path: it restores the exact set the LAST
   * dismissal cleared. This one is the reader changing their mind about a single row they can see,
   * so it also takes that row out of the pending undo — otherwise the chip would go on offering to
   * put back a message that is already back, and "exactly what the last dismissal cleared" would
   * stop being true of it.
   */
  async function restoreMessages(ids: Ids) {
    const channel = deps.selectedChannelId();
    const restored = ids.map(String);
    const removed = restored.filter((id: string) => doneState().archived.has(id));
    const previousDismissal = deps.lastDismissal().current();
    // Before anything here changes, so that what the server holds is known if it refuses (`#326
    // refused-undo-restores-done`).
    noteInboxWrite(restored);
    for (const id of restored) {
      doneState().archived.delete(id);
    }
    // On its way from now on, for what waits on a Done of these messages to be saved (`doneSaved`,
    // `#327 done-races-remaining`): settled once the write has been answered and a refusal taken back.
    const saving = noteDoneSaving(restored);
    const saidBack = `${restored.length} message${restored.length === 1 ? "" : "s"} back in the list.`;
    // `#229 desktop-two-column`. Put back in the other column as well.
    const column = deps.columnWork().active();
    // What the lines in the `try` change, for a refusal to take back: from the notes above on, a slip
    // before the write is queued takes back exactly what the put back changed, the ledger told no write
    // went (`sent`), as in `dismissMessages` (`#326 refused-undo-restores-done`).
    /** @type {HeldDoneMark | null} */
    let held: unknown = null;
    /** @type {string[]} */
    let mirrored: string[] = [];
    // The Undo chip's offer once this has taken its messages out of it. `#326 refused-undo-restores-done`:
    // a refusal changes the chip back only while it still offers that — not once a Done made while this
    // was on its way offers its own, which it used to take away, offering this one's again.
    let offer = deps.lastDismissal().current();
    /** Whether the write was queued: a slip before it takes back exactly what this changed. */
    let sent = false;
    let settled = false;
    /** The ids of the batches the server has taken. */
    const landed: string[] = [];
    /** The write as kept on the device until answered (`inboxLedger`, `#892 voice-page-optimistic-done`). */
    let kept: unknown = null;
    try {
      // `#245 done-feedback`. Unread again, so no longer being drawn as just marked Done: the Undo chip
      // and z (`undoDismissal`) and the row's own button (`toggleArchived`) all come through here.
      unflashIds(restored);
      held = deps.markHeldDone(restored, false);
      mirrored = mirrorArchived(restored, false);
      // ...and every view held out of sight, and the cards on screen. `#306 done-stale-across-views`.
      deps.followDoneEverywhere(restored, column);
      if (deps.lastDismissal().current() !== null) {
        const left = deps.lastDismissal().current()!.messages.filter((id: string) => !restored.includes(id));
        deps.lastDismissal().set(left.length === 0 ? null : { ...deps.lastDismissal().current(), messages: left });
      }
      offer = deps.lastDismissal().current();
      // `#892 voice-page-optimistic-done`. Said now, as drawn now, before anything awaits; a refusal is said by
      // the error line.
      deps.setStatus(saidBack);
      await refreshAfterInboxChange();
      renderTodoControls();
      kept = deps.inboxLedger().add("restore", channel, restored, inboxWriteOwner());
      // In batches the server accepts, as the dismissal that made them went: the undo of a long Done
      // through here names more ids than one request may carry. `#240 reading-bookmark`.
      const writing = queueInboxWrite(() => deps.writeInBatches(`/api/v1/channels/${encodeURIComponent(channel)}/restore`,
        restored, landed));
      sent = true;
      await writing;
      // Settled before the read below, which then speaks for these messages.
      settleInboxWrite(restored, false, new Set(restored));
      deps.settleHeldDone(held, true);
      settled = true;
      // Saved: what waits on it need not wait for the read below as well.
      saving();
      if (deps.readModes().hiding() && !deps.providerFacts.threading()) {
        await deps.loadTodo({ keepPosition: true, ownAct: true, readAs: "mutation" });
      }
    } catch (error) {
      // `#892 voice-page-optimistic-done`. What was said at once is taken back, if nothing was said since.
      if (deps.statusSays(saidBack)) deps.dismissStatus();
      // Only what the server did not put back is Done again: the batches before the one that failed
      // are back there, so they stay back here, and are no longer the last dismissal's. A read of the
      // to-do list that failed after every batch landed takes nothing back.
      const kept = new Set(landed);
      // `#326 refused-undo-restores-done`: nor what the server holds put back all the same — a Done this
      // put back, refused itself — nor what a later act of the reader's, still on its way, drew.
      const stands = settled ? new Set() : settleInboxWrite(restored, false, kept, sent);
      const back = new Set(restored.filter((id: string) => !kept.has(id) && !stands.has(id)));
      if (!settled && held) {
        const standing = restored.filter((id: string) => !back.has(id));
        if (standing.length > 0) deps.settleHeldDone(deps.heldDonePart(held, standing), true);
        deps.settleHeldDone(standing.length > 0 ? deps.heldDonePart(held, [...back]) : held, false);
      }
      // Unless the chip has changed since: a Done made meanwhile offers its own Undo, which stands.
      if (deps.lastDismissal().current() === offer) {
        const left = previousDismissal === null ? [] : previousDismissal.messages.filter((id: string) => !kept.has(id));
        deps.lastDismissal().set(previousDismissal === null || left.length === 0 ? null : { ...previousDismissal, messages: left });
      }
      deps.inColumn(column, () => {
        for (const id of removed) if (back.has(id)) doneState().archived.add(id);
        mirrorArchived(mirrored.filter((id: string) => back.has(id)), true);
        deps.guardQuietly(refreshAfterInboxChange)();
      });
      // ...and in every view held, from the store the lines above took it back in; in the left column
      // when it was made in the right one and that has stopped showing its thread since; and in the right
      // column when it was made in the left one and the right one shows its thread again since. The
      // other column's line above reaches neither. `#306 done-stale-across-views`.
      deps.followDoneEverywhere(restored, column);
      renderTodoControls();
      if (landed.length > 0) (error as { landed?: number }).landed = landed.length;
      throw error;
    } finally {
      // Answered, taken or refused: no longer kept on the device to send again.
      if (kept !== null) deps.inboxLedger().settle(kept);
      // Refused, once taken back, so what waits on it finds the messages as the server holds them.
      saving();
    }
    renderTodoControls();
  }

  /**
   * Done, or put back, in one column, shown in the other: Done is the message's, not a column's.
   * `#229 desktop-two-column`. Redrawn there only when the other column draws one of those messages;
   * otherwise, on Main, only the counts of unread replies its roots show. Answers the ids it changed
   * there, for a write that fails to take back.
   *
   * @param {string[]} ids
   * @param {boolean} done
   * @returns {string[]}
   */
  function mirrorArchived(ids: Ids, done: boolean) {
    /** @type {string[]} */
    let changed: string[] = [];
    deps.inOtherColumn(() => {
      changed = ids.filter((id: string) => doneState().archived.has(id) !== done);
      for (const id of changed) {
        if (done) doneState().archived.add(id);
        else doneState().archived.delete(id);
      }
      const drawn = [...deps.el("discord-log").children].some((row: Row) => deps.idsOf(row).some((id: string) => ids.includes(id)));
      if (drawn && changed.length) renderDoneChange();
      else deps.syncUnreadReplies();
    });
    return changed;
  }

  // --- one read state in every view the page holds -------------------------------------------------
  //
  // `#306 done-stale-across-views`. Found in review of #295 shift-e-read-collapse: a message marked

  return {
    freshDoneState, doneState, noteArchived, dismissalName, drawUndoLabel, renderTodoControls,
    backlogIsArmed, disarmBacklog, armBacklog, todoView, toggleArchived, markRowDone,
    forgetDoneFlash, drawDoneFlash, drawDoneFlashes, unflashIds, unflashDone, flashDone,
    refreshAfterInboxChange, renderDoneChange, putBackRowsOff, queueInboxWrite, inboxWriteOwner,
    inboxPost, sendLeftOverInboxWrites, noteInboxWrite, settleInboxWrite, noteDoneSaving,
    doneSaved, dismissMessages, dealtWithWords, markReadUpstream, restoreMessages, mirrorArchived,
    doneSaving, DONE_FLASH_HOLD_MS, DONE_FLASH_FADE_MS, INBOX_WAITING_WORDS,
  };
}

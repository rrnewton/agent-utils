// The read state's acts beside Done's own (`#624 voice-page-modularity`, moved from the legacy
// script): the read-run lines' construction, the dismissal offer and inbox ledger, every copy of
// the Done state kept in line with the store's, and the dismissal's Undo and the backlog's clear.
// What counts as a read mode, and putting the list in one, stay the page's (present/rows and the
// legacy script) — this owner only asks whether read messages are hidden, never which mode that
// is or how the reader cycles to it. Words, order and timing are as they were in the script.

import { DISCORD_PAGE_LIMIT } from "../../core/channels/index.ts";
import { createReadRuns } from "../../present/rows/index.ts";
import type { ReadRuns } from "../../present/rows/index.ts";
import { createReadRunLines } from "./readrunlines.ts";
import type { ReadRunLines } from "./readrunlines.ts";
import type { LeftOverWrite } from "./doneact.ts";

/** A dismissal remembered for Undo, as this owner reads and hands it on. */
export interface ReadDismissal {
  messages: string[];
  bump?: unknown;
  at?: number;
  [key: string]: unknown;
}

/** The dismissal offer, as this owner uses it (view/shell `createUndoOffer`). */
export interface ReadDismissalOffer {
  current(): ReadDismissal | null;
  set(offer: ReadDismissal | null): void;
}

/** The inbox write ledger, as this owner hands it on (core/outbox `createInboxWriteLedger`). */
export interface ReadInboxLedger {
  add(kind: string, channel: string, messages: readonly string[], owner: string, at?: number): unknown;
  settle(kept: unknown): void;
  beat(): void;
  takeLeftOver(owner: string): LeftOverWrite[];
  leave(): void;
  clear(): void;
  size(): number;
}

/** One held view's copy of which messages are Done. */
export interface HeldDoneCopy {
  dismissed: Set<string>;
}

/** A message, as the dismissal acts read it. */
export interface ListedMessage {
  readonly id: unknown;
}

/** What the read state's acts ask of the page. */
export interface ReadActsDependencies {
  // The read-run lines' page (as `createReadRunLines` asks for it).
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  inThisColumn<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void;
  selection(): { view: string; threadId: unknown; thread: { root?: { id: unknown } | null } | null };
  threads(): Array<{ id: unknown; root?: { id: unknown } | null }>;
  readHiding(): boolean;
  readRunsSuspended(): boolean;
  threading(): boolean;
  eachColumnRenderCached(): void;
  openReadRun(row: Element): void;
  readRunRows(row: Element): Element[];
  threadIdOf(message: unknown): string | null;
  selectionThread(): { root?: { id: unknown } | null } | null;
  // The dismissal offer and ledger, made by their own layers' factories, handed in as makers
  // because those layers are not this package's to import.
  makeDismissalOffer(): ReadDismissalOffer;
  makeInboxLedger(): ReadInboxLedger;
  // Done in the store and in every held copy.
  storeDismissed(): Set<string>;
  storeChannel(): string;
  selectedChannelId(): unknown;
  heldDoneCopies(): Iterable<HeldDoneCopy>;
  sideShowing(): boolean;
  activeColumn(): unknown;
  sideColumn(): unknown;
  mainColumn(): unknown;
  sideLeft(): boolean;
  inColumn(column: unknown, act: () => void): void;
  doneState(): { archived: Set<string> };
  cardsWhileDressedAfter(up: boolean, act: () => void): void;
  sideColumnUp(): boolean;
  renderCachedTimeline(): void;
  syncUnreadReplies(): void;
  renderSideColumn(): void;
  // The rows as drawn, and which of them the read filter keeps.
  el(id: string): HTMLElement;
  idsOf(row: unknown): string[];
  readFilterKeeps(): (message: ListedMessage) => boolean;
  timelineMessages(): readonly ListedMessage[];
  // The dismissal's Undo and the backlog's clear.
  renderTodoControls(): void;
  restoreMessages(messages: string[]): Promise<unknown>;
  undoBookmarkBump(bump: unknown, restoring: Promise<unknown>): Promise<unknown>;
  listedMessages(): ListedMessage[];
  backlogIsArmed(): boolean;
  armBacklog(): void;
  disarmBacklog(): void;
  setStatus(text: string): void;
  dismissMessages(body: { through: unknown; limit: number }, bump: unknown): Promise<unknown>;
  throughBump(row: unknown): unknown;
}

/** The read state's acts and the state they own. */
export interface ReadActs {
  readonly readRuns: ReadRuns;
  readonly readRunLines: ReadRunLines;
  /** The exact set the last dismissal cleared, so the undo restores that and nothing else. */
  readonly lastDismissal: ReadDismissalOffer;
  /** The Done and put-back writes not yet answered, kept on the device. */
  readonly inboxLedger: ReadInboxLedger;
  caughtUpDone(copy: Set<string>, ids: string[]): Set<string>;
  followDoneEverywhere(ids: string[], madeIn: unknown): void;
  rowsMissDone(): boolean;
  undoDismissal(): Promise<void>;
  clearBacklog(): Promise<void>;
}

/** Make the read state's acts, in the page's construction order. */
export function createReadActs(deps: ReadActsDependencies): ReadActs {
  const readRuns = createReadRuns();

  const readRunLines = createReadRunLines({
    byId: deps.byId,
    leftById: deps.leftById,
    inThisColumn: deps.inThisColumn,
    selection: deps.selection,
    threads: deps.threads,
    hiding: deps.readHiding,
    suspended: deps.readRunsSuspended,
    threading: deps.threading,
    eachColumnRenderCached: deps.eachColumnRenderCached,
    openReadRun: deps.openReadRun,
    readRunRows: deps.readRunRows,
    threadIdOf: deps.threadIdOf,
    selectionThread: deps.selectionThread,
  });

  /**
   * The exact set the last dismissal cleared, so the undo restores that and nothing else.
   *
   * Not a count and not "the last N": by the time the reader presses undo, N may name a different
   * set. Held as the server reported it, which is also what makes undoing a BULK clear exact (web/src/view/
   * shell `createUndoOffer`).
   */
  const lastDismissal = deps.makeDismissalOffer();

  /** The Done and put-back writes not yet answered, kept on the device. */
  const inboxLedger = deps.makeInboxLedger();

  // Done in Main still showed as not Done in All, loaded earlier in the session, for seconds, until
  // some later read redrew it. Each view keeps its own copy of which messages are Done (`doneState().archived`)
  // for when it is shown again — a view left in a column (`channelContexts`), and the right column's
  // thread while two columns are put away or show their cards — and only the list the Done was made in,
  // and the other column while it shows a thread (`mirrorArchived`), were told. Worse, a stale copy put
  // back was later written over the store (`syncCanonDismissed`), so the Done could go out of every view.
  //
  // So a Done or an Undo here, and a failed one taken back, brings every copy into line with the store
  // at once (`followDoneEverywhere`), which `markHeldDone` and `settleHeldDone` have already moved; the
  // cards on screen, which count unread replies from the store, are drawn again then; and a view whose
  // rows were drawn before is drawn again as it is shown, where they say otherwise (`rowsMissDone`).
  // No read is asked for any of it.
  //
  // A Done or an Undo made in the right column reaches the left one through `mirrorArchived`, which
  // works only while the right column shows a thread. Its thread closed, or two columns put away, while
  // the write was on its way, a refusal taken back in the right column left the left column showing it
  // as made, and the next write from the left saved that over the store. So the left column's own copy
  // is brought into line here too then, and its rows drawn again where they say otherwise.
  //
  // The mirror image, found in the fourth review round: a Done or an Undo made in the left column while
  // the right column's thread was put away reached the right column's copy (above), but nothing told the
  // `mirrorArchived` of the act; so when the right column showed the thread again (its thread opened
  // again, or two columns brought back) while the write was on its way, a refusal taken back in the left
  // column left the right one showing it as made. A reply from there then wrote no Done, its message
  // looking Done already, and asked the chat service to mark a message read whose Done had failed; and
  // the next write from the right saved the refused state over the store. So the right column showing a
  // thread is brought into line here too, after an act made in the left column.

  /**
   * `copy`, a copy of which messages are Done, with `ids` brought into line with the store's
   * (`channelStore.current().dismissed`): `copy` itself where none of them differs, else a new set, since `copy`
   * may be the very set a list on screen is drawn from.
   */
  function caughtUpDone(copy: Set<string>, ids: string[]): Set<string> {
    const moved = ids.filter((id) => copy.has(id) !== deps.storeDismissed().has(id));
    if (moved.length === 0) return copy;
    const next = new Set(copy);
    for (const id of moved) {
      if (deps.storeDismissed().has(id)) next.add(id);
      else next.delete(id);
    }
    return next;
  }

  /**
   * Every copy of the read state of `ids` that the page holds out of sight, into line with the store's
   * (`channelStore.current().dismissed`), after a Done, a put back, or a failed one taken back: the views held in
   * either column, and the right column's own while it does not show a thread. Each copy is replaced
   * rather than changed in place, since one may be the very set a list on screen is drawn from. Then
   * the cards on screen, drawn from the store, are drawn again. `#306 done-stale-across-views`.
   *
   * `madeIn` is the column the Done or the Undo was made in. Made in the right column, which no longer
   * shows a thread (its thread closed, or two columns put away, while the write was on its way), the
   * left column on screen is brought into line as well, and drawn again: `mirrorArchived`, which tells it
   * while the right column shows a thread, does nothing then. Made in the left column, the right column
   * showing a thread is brought into line, and drawn again where its rows say otherwise: a thread shown
   * again since the act (its thread opened again, or two columns brought back) holds the copy brought
   * into line while it was put away, which the `mirrorArchived` of the act never changed, so the
   * `mirrorArchived` taking it back changes nothing in it.
   */
  function followDoneEverywhere(ids: string[], madeIn: unknown): void {
    if (!deps.threading() || deps.storeChannel() !== String(deps.selectedChannelId())) return;
    const named = ids.map(String);
    const caughtUp = (copy: Set<string>): Set<string> => caughtUpDone(copy, named);
    for (const held of deps.heldDoneCopies()) held.dismissed = caughtUp(held.dismissed);
    if (!deps.sideShowing() && deps.activeColumn() !== deps.sideColumn() && deps.sideLeft()) {
      deps.inColumn(deps.sideColumn(), () => {
        deps.doneState().archived = caughtUp(deps.doneState().archived);
      });
    }
    if (madeIn === deps.sideColumn() && !deps.sideShowing()) {
      // The cards, drawn at the end of this, dress the rows of what changed among them.
      deps.cardsWhileDressedAfter(deps.sideColumnUp(), () => {
        deps.inColumn(deps.mainColumn(), () => {
          deps.doneState().archived = caughtUp(deps.doneState().archived);
          // Its rows, or under Hide read which rows it draws; else Main's roots' counts of unread replies.
          if (rowsMissDone()) deps.renderCachedTimeline();
          else deps.syncUnreadReplies();
        });
      });
    }
    if (madeIn !== deps.sideColumn() && deps.sideShowing()) {
      deps.inColumn(deps.sideColumn(), () => {
        deps.doneState().archived = caughtUp(deps.doneState().archived);
        // Its rows, or under Hide read which rows it draws. A thread draws no counts of unread replies.
        if (rowsMissDone()) deps.renderCachedTimeline();
      });
    }
    if (deps.sideColumnUp()) deps.renderSideColumn();
  }

  /**
   * Whether the rows drawn in the column being worked on show a message's read state other than its own
   * copy (`doneState().archived`) says: a row drawn Done, or not, against it, or under Hide read a message drawn,
   * or left out, against it. Rows held while a Done or an Undo was made in another view keep the look
   * they had; their copy was brought into line at once (`followDoneEverywhere`), and this says they are
   * to be drawn again as they are shown. `#306 done-stale-across-views`.
   */
  function rowsMissDone(): boolean {
    if (!deps.threading() || deps.selection().view === "threads") return false;
    const rows = [...deps.el("discord-log").children];
    if (rows.some((row) => {
      const ids = deps.idsOf(row);
      return ids.length > 0 && (row.getAttribute("data-archived") === "true") !== ids.every((id) => deps.doneState().archived.has(id));
    })) return true;
    if (!readRunLines.hidingRead()) return false;
    const keeps = deps.readFilterKeeps();
    const wanted = new Set(deps.timelineMessages().filter(keeps).map((message) => String(message.id)));
    const drawn = new Set(rows.flatMap((row) => deps.idsOf(row)));
    return wanted.size !== drawn.size || [...wanted].some((id) => !drawn.has(id));
  }

  /** Put back exactly what the last dismissal cleared, and the bookmark where that moved it from. */
  async function undoDismissal(): Promise<void> {
    if (lastDismissal.current() === null) {
      return;
    }
    const undoing = lastDismissal.current() as ReadDismissal;
    lastDismissal.set(null);
    deps.renderTodoControls();
    const restoring = deps.restoreMessages(undoing.messages);
    // `#240 reading-bookmark`. Back where it was, unless something moved it since.
    if (undoing.bump) await deps.undoBookmarkBump(undoing.bump, restoring);
    else await restoring;
  }

  /** Declare bankruptcy on everything currently in the list. Two taps, and it says the count. */
  async function clearBacklog(): Promise<void> {
    const rows = [...deps.el("discord-log").children];
    if (rows.length === 0) {
      return;
    }
    // MESSAGES, not rows: what the server is about to clear is Discord messages, and a combined row
    // is more than one of them. A count of rows would promise to clear fewer than it did. In time
    // order, so the newest is last even with replies gathered above older rows (`#215 reply-coalesce`).
    const messages = deps.listedMessages().map((message) => String(message.id));
    if (!deps.backlogIsArmed()) {
      deps.armBacklog();
      deps.setStatus(`Tap again to clear ${messages.length} — undo will be offered afterwards.`);
      return;
    }
    deps.disarmBacklog();
    // THROUGH the newest row on screen, so the server decides the boundary from its own ordering
    // rather than from a list of ids this page assembled. The boundary is included.
    //
    // ...and WITH the window this page read the list with. The server resolves `through` against a
    // window of its own, so a boundary sent without one is resolved against the server's default —
    // which, for a client reading a smaller page than the server's default, means clearing messages
    // that were never on screen. The limit is what makes "everything above this row" mean the rows
    // the reader was actually looking at.
    //
    // The boundary is the NEWEST message on screen, which on a combined bottom row is its LAST
    // constituent rather than the id the row is filed under. Sending the row's identity would stop
    // the sweep one message short and leave the trailing overflow in the queue.
    const rowsNow = [...deps.el("discord-log").children];
    await deps.dismissMessages({
      through: messages[messages.length - 1],
      limit: DISCORD_PAGE_LIMIT,
    }, deps.throughBump(rowsNow[rowsNow.length - 1]));
  }

  return {
    readRuns,
    readRunLines,
    lastDismissal,
    inboxLedger,
    caughtUpDone,
    followDoneEverywhere,
    rowsMissDone,
    undoDismissal,
    clearBacklog,
  };
}

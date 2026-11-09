// A reply sent from a column, reported in that column (`#277 reply-in-column`, `#467
// inline-send-in-place`; `#624 voice-page-modularity`, moved from the legacy script into
// view/channel for the plan's step 17): which column each send in the outbox was opened from,
// what the reader is told when one settles without going (in place, at the foot of its column,
// or where it was sent), and the one not yet been down to since — which the column's Newest chip
// reads. The outbox, its retry state and the columns are handed in; the words are the page's.

/** A send in the outbox, as this reporting reads it. */
export interface ColumnReplyEntry {
  readonly id: string;
  readonly state: string;
  readonly postedCount: number;
}

export interface ColumnRepliesDependencies<Column> {
  el(id: string): HTMLElement & { hidden: boolean };
  /** The column a send was opened from is remembered by the dispatch in this map. */
  outbox: { has(id: string): boolean; get(id: string): ColumnReplyEntry | undefined };
  retryOf(id: string): { offline: boolean } | null;
  retryPending(id: string): boolean;
  inPlaceDrawn(column: Column): { groups: { children: Iterable<{ outgoingEntry?: ColumnReplyEntry }> }[] } | undefined;
  layoutColumns(): boolean;
  isSide(column: Column): boolean;
  isMain(column: Column): boolean;
  sideShowing(): boolean;
  inColumn<T>(column: Column, act: () => T): T;
  columnKey(column: Column): string;
  activeColumn(): Column;
  setStatus(text: string): void;
  renderJumpNewest(): void;
}

/** The reply-in-column reporting, as the page and the dispatch call it. */
export interface ColumnReplies<Column> {
  /** By outbox id, the column a send was opened from (the dispatch writes it). */
  readonly columnReplies: Map<string, Column>;
  /** By column name, the outbox id of a reply not yet been down to since it was told of. */
  readonly unseenReplies: { main: string | null; side: string | null };
  reportColumnReply(entry: ColumnReplyEntry): void;
  outboxRow(id: string): HTMLElement | null;
  unseenColumnReply(): ColumnReplyEntry | null;
}

/** Make the reply-in-column reporting from the page's outbox and columns. */
export function createColumnReplies<Column>(deps: ColumnRepliesDependencies<Column>): ColumnReplies<Column> {
  const columnReplies = new Map<string, Column>();
  const unseenReplies: { main: string | null; side: string | null } = { main: null, side: null };
  const unseenColumnReplies = unseenReplies as Record<string, string | null>;

  function reportColumnReply(entry: ColumnReplyEntry): void {
    const column = columnReplies.get(entry.id);
    // Those dismissed, or taken back into a box by Edit, since: nothing more to tell of them.
    for (const id of [...columnReplies.keys()]) if (!deps.outbox.has(id)) columnReplies.delete(id);
    if (!column || entry.state === "sending") return;
    if (entry.state === "sent") {
      columnReplies.delete(entry.id);
      return;
    }
    const waiting = deps.retryOf(entry.id);
    if (waiting) {
      deps.setStatus(waiting.offline
        ? "Your reply is not sent yet: this device is offline. It goes when the connection is back."
        : "Your reply is not sent yet. The page tries again in a moment.");
      return;
    }
    // `#467 inline-send-in-place`. Drawn where it was written, under the message it answers: said so, and
    // no way down to a row the reader is already at.
    const placed = (deps.inPlaceDrawn(column) || { groups: [] }).groups
      .some((group) => [...group.children].some((row) => row.outgoingEntry === entry));
    if (placed) {
      deps.setStatus(entry.state === "failed"
        ? entry.postedCount > 0
          ? "Only part of your reply was sent. Retry the rest under the message it answers."
          : "Your reply was not sent. Retry is under the message it answers."
        : "Your reply may not have been posted. Check it under the message it answers.");
      return;
    }
    // Where the row is: in the column it was sent from while that column shows where it went.
    const named = deps.layoutColumns() ? (deps.isSide(column) ? "the right column" : "the left column") : "the list";
    const drawn = (deps.isMain(column) || deps.sideShowing()) && deps.inColumn(column, () => outboxRow(entry.id) !== null);
    const where = drawn ? `at the foot of ${named}` : "with it where it was sent";
    // A refusal after some parts went (`#444 send-refusal-reason`) is failed too, and part of it is in.
    deps.setStatus(entry.state === "failed"
      ? entry.postedCount > 0
        ? `Only part of your reply was sent. Retry the rest ${where}.`
        : `Your reply was not sent. Retry is ${where}.`
      : `Your reply may not have been posted. Check it ${where}.`);
    unseenColumnReplies[deps.columnKey(column)] = entry.id;
    if (drawn) deps.inColumn(column, deps.renderJumpNewest);
  }

  /** The outbox row of entry `id` in the column being worked on, while it is drawn there; else null. */
  function outboxRow(id: string): HTMLElement | null {
    const host = deps.el("outgoing-log");
    if (host.hidden) return null;
    return [...host.children].find((row) => (row as HTMLElement).getAttribute("data-outgoing-id") === id) as HTMLElement | undefined || null;
  }

  /**
   * The reply sent from the column being worked on that did not go, while its outbox row is drawn in
   * this column and the reader has not been down to it since they were told: the entry, or null. One
   * sent since, Retried, or dismissed is let go.
   */
  function unseenColumnReply(): ColumnReplyEntry | null {
    const key = deps.columnKey(deps.activeColumn());
    const id = unseenColumnReplies[key];
    if (id === null || id === undefined) return null;
    const entry = deps.outbox.get(id);
    if (!entry || (entry.state !== "failed" && entry.state !== "unconfirmed") || deps.retryPending(id)) {
      unseenColumnReplies[key] = null;
      return null;
    }
    return outboxRow(id) ? entry : null;
  }

  return { columnReplies, unseenReplies, reportColumnReply, outboxRow, unseenColumnReply };
}

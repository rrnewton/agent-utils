// A drawn row of messages, read back (`#624 voice-page-modularity`, moved from the legacy script into
// view/channel): whether it is a message row, which messages and ids it stands for and when they were
// sent, whether it is shown, the row standing for one folded into a run, a message's top within it,
// its buttons, its author's label, its place for the bookmark, and finding a message's row in a list.

import type { Message } from "../../contract/index.ts";
import { anchorOfMessage } from "../../core/annotations/index.ts";
import { childByClass } from "../dom/index.ts";

/** A row the legacy script drew, carrying the messages it stands for (set as it draws the row). */
type RowElement = Element & { messages?: Message[] };

/** Whether `row` is a message the filters judge: it declares what it is findable by (`searchable`). */
const isMessageRow = (row: Element): boolean => row.getAttribute("data-search") !== null;

/**
 * The messages a rendered row stands for, oldest first.
 *
 * Held ON the element, because the row already is this page's record of what it is showing and a
 * second map keyed by id would have to be kept in step with a list three different reads rebuild.
 */
const rowMessages = (row: Element | null | undefined): Message[] => (row && (row as RowElement).messages) || [];

/**
 * Every Discord id a row stands for, oldest first.
 *
 * Read from the ATTRIBUTE rather than from `rowMessages`, because that is the idiom every other
 * derivation in `renderChannelRows` follows and because it keeps the grouping visible in the
 * document rather than only in a property a debugger has to be told about. `discordNode` writes
 * both from the same array, so they cannot disagree.
 */
const idsOf = (row: Element | null | undefined): string[] =>
  String((row && row.getAttribute("data-ids")) || "")
    .split(" ")
    .filter(Boolean);

/** The time a row was sent, in milliseconds, from its first message; NaN when it cannot be read. */
const rowSentMs = (row: Element): number => Date.parse(String((rowMessages(row)[0] || ({} as Partial<Message>)).timestamp || ""));

/** The time a row's newest message was sent, in milliseconds; NaN when it cannot be read. */
const rowNewestMs = (row: Element): number => {
  const held = rowMessages(row);
  return Date.parse(String((held[held.length - 1] || ({} as Partial<Message>)).timestamp || ""));
};

/** Whether the reader can see `row` in the list: not hidden, and not filtered out by the search. */
const rowShown = (row: Element): boolean => !(row as HTMLElement).hidden && !String(row.className).split(/\s+/).includes("search-hidden");

/** The row on screen standing for `row`: itself, or the line of the collapsed run it is hidden in. */
function shownRowFor<T extends Element>(row: T | null | undefined): T | null | undefined {
  if (!row || row.getAttribute("data-read-run") !== "member" || !row.parentNode) return row;
  // A run's rows are the same kind of element as the row asked about.
  const rows: Element[] = [...row.parentNode.children];
  for (let at = rows.indexOf(row); at >= 0; at -= 1) {
    if (rows[at].getAttribute("data-read-run") === "head") return rows[at] as T;
  }
  return row;
}

/** The row standing for `row` on the screen — itself, or the line of its collapsed run — or null when none does. */
function standingRow<T extends Element>(row: T | null | undefined): T | null {
  const shown = row ? shownRowFor(row) : null;
  return shown && rowShown(shown) ? shown : null;
}

/**
 * Where message `id` stands in `row`, in the viewport, as the top of a row of its own would: the row's
 * top, less the parts of a combined row (`glom`) drawn above it. A combined row draws each of its
 * messages as a block of its `.body`, in order, so the second half of a long post split in two starts
 * as far below the row's first block as the first half is tall — and, as a row of its own in the
 * Pinned list, its text started right under the author line. The row's own top for its first message,
 * for a row that holds no such message (a run's line, the message standing in for one a filter hides),
 * and for a block its fold has cut off out of sight.
 *
 * From review: a pin found as the second part of a split post was put back by the row's top, and so
 * lower on the screen, by the height of the part above it, than it stood in the Pinned list.
 */
function messageTop(row: Element, id: string | null): number {
  const top = row.getBoundingClientRect().top;
  const ids = idsOf(row);
  const at = id === null ? -1 : ids.indexOf(id);
  if (at <= 0) return top;
  const body = childByClass(row, "body");
  if (!body || body.children.length !== ids.length) return top;
  // The body holds a child per id, checked just above, so both indexes are inside it.
  const start = (body.children[at] as Element).getBoundingClientRect().top;
  if (start >= body.getBoundingClientRect().bottom) return top;
  return top + start - (body.children[0] as Element).getBoundingClientRect().top;
}

const doneButtonOf = (row: Element): HTMLElement | null => childByClass(row, "done-button");

const notNoiseButtonOf = (row: Element): HTMLElement | null => childByClass(row, "not-noise-button");

/** How a row names its author: the display name, and `(bot)` when it is one. */
function authorLabel(row: Element): string {
  const name = row.getAttribute("data-author") || "";
  return row.getAttribute("data-author-bot") === "true" ? `${name} (bot)` : name;
}

/** The anchor for the line just above `row` (`above`), or just below it (`below`). */
function anchorOfRow(row: Element, place: "above" | "below"): ReturnType<typeof anchorOfMessage> | null {
  const held = rowMessages(row);
  const message = place === "below" ? held[held.length - 1] : held[0];
  return message ? anchorOfMessage(message, place) : null;
}

/** The row of `list` holding message `id`, or null. */
function rowIn(list: Element, id: unknown): HTMLElement | null {
  return ([...list.children].find((row) => idsOf(row).includes(String(id))) as HTMLElement | undefined) || null;
}

export {
  anchorOfRow,
  authorLabel,
  doneButtonOf,
  idsOf,
  isMessageRow,
  messageTop,
  notNoiseButtonOf,
  rowIn,
  rowMessages,
  rowNewestMs,
  rowSentMs,
  rowShown,
  shownRowFor,
  standingRow,
};
export type { RowElement };

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
    const sibling = rows[at];
    if (sibling && sibling.getAttribute("data-read-run") === "head") return sibling as T;
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

/** The view each row was built for, noted as it is built (`noteRowBuilt`). */
const rowBuiltFor = new WeakMap<Element, string>();

/**
 * What a build puts on a row, and what the dress paints on it, is the view's: a thread's tag
 * and gather button in Main and in All, neither in the thread's own list; its strip in All
 * alone; its unread count in Main alone. So a row is noted by the view it was built for, and
 * a draw for another view builds it again, as a fresh draw would.
 */
const builtForView = (view: string): string => view;

/** Note the view `row` is being built for, as its draw begins. */
const noteRowBuilt = (row: Element, view: string): void => {
  rowBuiltFor.set(row, builtForView(view));
};

/**
 * Put `groups` on `list`, patching in place (`#1229 keyed-row-patching`): a row whose messages
 * are the very objects it was drawn from keeps its node — its height, its fold, and any
 * selection in it — changed and new groups are built by `build`, gone ones removed, and the
 * rest moved to order. `kept` rows stand above the patched part, untouched, in their order.
 * What a row shows that lives outside its messages (a Done, a fold, a summary) is the dress
 * pass's, which runs after every redraw as it always did. `unchanged` says when a drawn
 * message and an incoming one are the same message: the very object, where the store holds
 * the copies a row was drawn from; word for word (`sameMessage`), where a redraw is handed
 * fresh copies of the same page and no store stands behind it. `view`, when given, is the
 * view being drawn: a row built for another view is built again, as a fresh draw would.
 */
const patchRows = (
  list: Element,
  groups: readonly (readonly Message[])[],
  build: (group: Message[]) => Element,
  kept: readonly Element[] = [],
  unchanged: (drawn: Message, incoming: Message) => boolean = (drawn, incoming) => drawn === incoming,
  view?: string,
): void => {
  const byKey = new Map<string, Element[]>();
  for (const row of [...list.children].slice(kept.length)) {
    const key = idsOf(row).join(" ");
    const held = byKey.get(key);
    if (held) held.push(row);
    else byKey.set(key, [row]);
  }
  const next: Element[] = [...kept];
  for (const group of groups) {
    const key = group.map((message) => String(message.id)).join(" ");
    let row = byKey.get(key)?.shift();
    if (row) {
      const drawn = rowMessages(row);
      // A build puts on a row what the view it is built for has (a thread's tag, in Main and
      // All; not in the thread's own list), so a row built for another view is built again.
      const builtFor = rowBuiltFor.get(row);
      if (drawn.length !== group.length || group.some((message, at) => !unchanged(drawn[at] as Message, message)) ||
        (view !== undefined && builtFor !== undefined && builtFor !== builtForView(view))) {
        row = undefined;
      }
    }
    next.push(row ?? build(group as Message[]));
  }
  const keep = new Set(next);
  for (const row of [...list.children]) if (!keep.has(row) && row.parentNode === list) list.removeChild(row);
  for (let at = 0; at < next.length; at += 1) {
    const wanted = next[at] as Element;
    if (list.children[at] !== wanted) list.insertBefore(wanted, list.children[at] ?? null);
  }
  listDraws.set(list, (listDraws.get(list) ?? 0) + 1);
};

/** How many times a list has been drawn (`patchRows`), by list. */
const listDraws = new WeakMap<Element, number>();

/** How many times `list` has been drawn (`patchRows`). */
const listDrawCount = (list: Element): number => listDraws.get(list) ?? 0;

export {
  anchorOfRow,
  authorLabel,
  doneButtonOf,
  idsOf,
  isMessageRow,
  listDrawCount,
  messageTop,
  notNoiseButtonOf,
  noteRowBuilt,
  patchRows,
  rowIn,
  rowMessages,
  rowNewestMs,
  rowSentMs,
  rowShown,
  shownRowFor,
  standingRow,
};
export type { RowElement };

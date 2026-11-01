// A thread card in the Threads list and on a desk's right column, as the DOM holds it (`#624
// voice-page-modularity`, moved from the legacy script into view/channel): whether a row is seen, the
// root a card stands for, its id, the groups a gap among the cards counts, what a card stands for in a
// read run, and its layout marks and words (`#528 thread-card-markdown-and-cap`).

import type { Message } from "../../contract/index.ts";
import type { ThreadChoice } from "../../present/rows/index.ts";
import { classWords } from "../dom/index.ts";
import { idsOf } from "./rows.ts";

/** A card row the legacy script drew, with what it set on it. */
type CardRow = HTMLElement & {
  cardChoice?: ThreadChoice;
  cardRoot?: Message | null;
  cardMessages?: number;
  cardMessagesExact?: boolean;
};

/** One group a gap among the cards counts (`cardGapGroups`). */
interface CardGapGroup {
  head: CardRow;
  card: boolean;
  other?: boolean;
}

/**
 * Whether `row` is drawn where the reader can see it: not inside a list hidden by `hidden`, `display` or
 * `visibility` — the left column's behind the thread list is `visibility: hidden`. True where the
 * browser cannot say.
 */
function rowSeen(row: Element): boolean {
  const ask = (row as Element & { checkVisibility?: unknown }).checkVisibility;
  return typeof ask === "function" ? Boolean((ask as (options: object) => unknown).call(row, { visibilityProperty: true })) : true;
}

/** The thread's first message a card stands for, as last drawn; null while it is not loaded. */
function cardRootOf(row: Element): Message | null {
  const choice = (row as CardRow).cardChoice;
  return (choice && choice.root) || (row as CardRow).cardRoot || null;
}

/** A thread card's id in the Threads list. */
const threadCardId = (row: Element): string => String(row.getAttribute("data-context-id"));

/**
 * `list`'s children as the groups a gap counts: a card with the messages unpacked under it, or one
 * of Main's messages among the cards. Anything else ends a run and is left alone.
 */
function cardGapGroups(list: Element): CardGapGroup[] {
  const groups: CardGapGroup[] = [];
  for (const row of [...list.children] as CardRow[]) {
    if (row.hasAttribute("data-unpack-thread")) continue;
    if (row.hasAttribute("data-card") || classWords(row).includes("thread-card")) groups.push({ head: row, card: true });
    else if (row.hasAttribute("data-main-row")) groups.push({ head: row, card: false });
    else groups.push({ head: row, card: false, other: true });
  }
  return groups;
}

/** What one hidden group adds to its gap: its messages, its thread if it is one, its exactness. */
function cardGapTally(group: CardGapGroup): { messages: number; threads: number; exact: boolean; onlyCards: boolean } {
  if (group.card) {
    const messages = typeof group.head.cardMessages === "number" ? group.head.cardMessages : 1;
    return { messages, threads: 1, exact: group.head.cardMessagesExact !== false, onlyCards: true };
  }
  return { messages: Math.max(1, idsOf(group.head).length), threads: 0, exact: true, onlyCards: false };
}

/** What an item of the cards stands for in a run: a card its thread, a Main row its messages. */
const cardRunIds = (row: Element): string[] => (row.hasAttribute("data-card") ? [`card:${row.getAttribute("data-id")}`] : idsOf(row));

/**
 * Mark card `row` for the stylesheet by its layout (`cardView`): split, clamped to K lines, or waiting
 * for its summary. `#528 thread-card-markdown-and-cap`. Written only where it changes.
 */
function markCardLayout(row: Element, mode: string, cut: { readonly whole?: unknown } | null = null): void {
  const marks: Array<[string, boolean]> = [["data-split-card", mode === "split"],
    ["data-card-clamp", mode === "clamped" || (mode === "pending" && Boolean(cut && cut.whole))],
    ["data-summary-pending", mode === "pending"]];
  for (const [attribute, on] of marks) {
    if (row.hasAttribute(attribute) !== on) row.toggleAttribute(attribute, on);
  }
}

/**
 * Put a card's words in `node`: its root's formatted body where the card says the root, else `text`.
 * `#528 thread-card-markdown-and-cap`.
 */
function fillCardText(node: Element, view: { body(): Node | null | undefined }, text: string): void {
  const body = view.body();
  if (body) {
    node.replaceChildren(body);
    return;
  }
  node.replaceChildren();
  node.textContent = text;
}

export { cardGapGroups, cardGapTally, cardRootOf, cardRunIds, fillCardText, markCardLayout, rowSeen, threadCardId };
export type { CardGapGroup, CardRow };

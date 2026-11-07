// The messages a thread draws unpacked under its card, chosen from the store (`#407
// unpack-row-parity`, `#405 root-under-summary`, `#410 inline-reply-editor`, `#532
// shift-o-latency`; `#624 voice-page-modularity`, moved from the legacy script into
// core/channels; mu-readcore C3). Building, dressing and folding the rows themselves
// (`unpackBlock`, `placeUnpackRuns`, `redrawUnpackBlock`) is the page's and stays there;
// this owner only says which held messages they are, and how their read state signs.

import type { Message } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";
import { inTimeOrder, threadOf } from "./canon.ts";

/** A thread as unpacking names it: its id, and its root where the page holds it. */
export interface UnpackChoice {
  readonly id: string;
  readonly root: Message | null | undefined;
}

/** The page's read-state facts unpacking filters by, each asked as it is needed. */
export interface UnpackFilter {
  /** Is `message` left out of the page: empty, while the reader hides those (`hiddenEmpty`)? */
  empty(message: Message): boolean;
  /** Is Hide read on (`hidingRead`)? */
  hiding(): boolean;
  /**
   * What the read filters keep of thread `choice` (`readFilterKeeps`): builds the predicate.
   * Called only where `hiding()` holds, as the legacy `unpackMessages` called it.
   */
  keeps(choice: UnpackChoice): (message: Message) => boolean;
  /** Is `message` still to do over read set `done` (`stillToDo`)? */
  toDo(message: Message, done: ReadonlySet<string>): boolean;
}

/**
 * The store's messages of thread `choice.id`, oldest first, its root among them where it is held.
 * `current` is whether store `canon` is the channel on screen: nothing held answers for another.
 */
export function unpackHeldMessages(canon: ChannelCanon, choice: UnpackChoice, current: boolean): Message[] {
  if (!current) return [];
  const held = canon.messages.filter((message) => threadOf(message) === choice.id);
  const root = choice.root;
  if (root && !canon.removed.has(String(root.id)) &&
      !held.some((message) => String(message.id) === String(root.id))) {
    return inTimeOrder([root, ...held], "timestamp");
  }
  return held;
}

/** Is `message` the root of thread `choice`, by its record or by being the summary's root? */
export function isThreadRoot(message: Message, choice: UnpackChoice): boolean {
  if (message.thread && message.thread.is_root === true) return true;
  const root = choice.root;
  return Boolean(root) && String(message.id) === String((root as Message).id);
}

/**
 * The messages thread `choice` draws unpacked, oldest first, from `held` (`unpackHeldMessages`):
 * what a thread draws of them — no empty message where those are hidden, and under Hide read what
 * a thread keeps (`filter.keeps`) — with its root only where `withRoot`. Shared by the cards'
 * blocks and by a Main root's replies gathered under it for Reply (`#410 inline-reply-editor`).
 */
export function unpackMessages(
  choice: UnpackChoice,
  held: readonly Message[],
  withRoot: boolean,
  filter: UnpackFilter,
): Message[] {
  let messages = held.filter((message) => !filter.empty(message));
  if (!withRoot) messages = messages.filter((message) => !isThreadRoot(message, choice));
  if (filter.hiding()) messages = messages.filter(filter.keeps(choice));
  return messages;
}

/**
 * Which of a block's messages `held` are still to do, as a signature. Apart from the block's own
 * (`unpackSignature` in the legacy script), but under Hide read, which takes a read message out
 * of the block: elsewhere a Done or a put back changes only how its row is dressed and where the
 * runs fall, which a block already drawn takes in place (`redrawUnpackBlock`, `#532`).
 */
export function unpackReadSignature(
  held: readonly Message[],
  dismissed: ReadonlySet<string>,
  filter: UnpackFilter,
): string {
  return held.map((message) => readLook(message, dismissed, filter)).join("");
}

/**
 * How a message's row draws its read state, as a signature: Done or not in the store, and still
 * to do or not — two answers, since a message read by a rule (noise, or the reader's own under
 * Mark own read) is not to do whether it is Done or not, and its row says which. `#532`.
 * The store arrives as its read set (`canon.dismissed`), the rule as `filter.toDo` — callers
 * pass the store's live set, so a pass over `held` reads the same set throughout, as today.
 */
export function readLook(message: Message, dismissed: ReadonlySet<string>, filter: UnpackFilter): string {
  return `${dismissed.has(String(message.id)) ? "d" : "-"}${filter.toDo(message, dismissed) ? "1" : "0"}`;
}

// One channel row's dress (`#407 unpack-row-parity`; `#624 voice-page-modularity`, moved from
// the legacy script into view/channel; mu-readcore): its thread chips, replied-to, speaker,
// Done, pinned, noise and read-aloud state, drawn from what the page holds now. Building the
// row and its body are channelrows.ts's and body.ts's; this owner only dresses a built row.

import type { Message } from "../../contract/index.ts";
import { authorLabel } from "./rows.ts";
import { doneButtonOf } from "./rows.ts";
import { childByClass, setAttributeIfChanged, setClassNameIfChanged, setHiddenIfChanged, setTextIfChanged } from "../dom/index.ts";
import type { ReplyTally } from "../../core/channels/index.ts";

/** What a pass over rows worked out once for all of them. */
export interface DressPass {
  tally: ReplyTally;
  answered: Set<string>;
  viewing: string;
  inMain: boolean;
  inAll: boolean;
}

/** What dressing a row asks of the page. */
export interface RowDressDependencies {
  outboxEntries(): Iterable<{ channel: string; replyTo: string | null; state: string; postedCount: number }>;
  idsOf(row: Element | null | undefined): string[];
  rowMessages(row: Element | null | undefined): Message[];
  drawThreadChips(row: Element, tally: ReplyTally, inMain: boolean, inAll: boolean): void;
  drawDoneFlash(row: Element): void;
  drawRowPin(row: Element): void;
  whoBucket(authorId: string | null, bot: boolean, viewing: string): string;
  archivedIds(): ReadonlySet<string>;
  isNoise(message: { noise?: unknown }): boolean;
  notNoiseButtonOf(row: Element): HTMLElement | null;
  readAloudNow(): { nowPlaying: { id: unknown } | null; pendingRead: unknown; readingLabel(): string; markReadingBlock(row: Element): void };
  inColumn<T>(column: unknown, fn: () => T): T;
  replyTally(): ReplyTally;
  unpackOnScreen(): boolean;
  columns: { main: unknown; side: unknown };
  leftEl(id: string): HTMLElement;
  selectedChannelId(): string;
  isFolded(entry: unknown): boolean;
  setFolded(entry: unknown, folded: boolean): void;
  selectedChannelId(): string;
}

/** Row dressing's commands: the legacy names, unchanged. */
export interface RowDress {
  answeredBy(rows: readonly Element[], viewing: string): Set<string>;
  dressChannelRow(row: HTMLElement, pass: DressPass): void;
  dressUnpackedRows(): void;
}

/** Row dressing over `deps`. */
export function createRowDress(deps: RowDressDependencies): RowDress {
  const outbox = { values: () => deps.outboxEntries() };
  const drawThreadChips = deps.drawThreadChips;
  const drawDoneFlash = deps.drawDoneFlash;
  const drawRowPin = deps.drawRowPin;
  const whoIsWho = { bucketFor: (authorId: string | null, bot: boolean, viewing: string) => deps.whoBucket(authorId, bot, viewing) };
  const doneState = () => ({ archived: deps.archivedIds() });
  const readAloudOwner = () => deps.readAloudNow();
  const idsOf = deps.idsOf;
  const rowMessages = deps.rowMessages;
  const isNoise = deps.isNoise;
  const notNoiseButtonOf = deps.notNoiseButtonOf;
  const unpackOnScreen = deps.unpackOnScreen;
  const columns = deps.columns;
  const leftEl = deps.leftEl;
  const selectedChannelId = deps.selectedChannelId;
  const isFolded = deps.isFolded;
  const setFolded = deps.setFolded;
  const inColumn = deps.inColumn;
  const replyTally = deps.replyTally;

function answeredBy(rows: readonly Element[], viewing: string): Set<string> {
  const answered = new Set<string>();
  for (const entry of outbox.values()) {
    if (entry.channel === viewing && entry.replyTo &&
        (entry.state === "sent" || entry.postedCount > 0)) answered.add(entry.replyTo);
  }
  for (const row of rows) {
    for (const parent of String(row.getAttribute("data-answers") || "").split(" ").filter(Boolean)) answered.add(parent);
  }
  return answered;
}

/**
 * Draw one channel row's state from what the page holds now: its thread chips, replied to, its speaker,
 * Done, pinned, noise, read automatically, being read aloud, and its author line and Done button's word.
 * What every pass over the rows does for every row it draws (`renderChannelRows`), and what an unpacked
 * block does for its rows (`unpackBlock`, `#407 unpack-row-parity`): one dress for a message row, in
 * whichever list it stands.
 *
 * `pass` is what the pass worked out once for all its rows: `tally` (`replyTally`), `answered`
 * (`answeredBy`), `viewing`, the channel, and whether the row is one of Main's own list (`inMain`) or
 * All's (`inAll`), which draw a root's unread replies and the thread strips.
 *
 * @param {Element} row
 * @param {{tally: ReplyTally, answered: Set<string>, viewing: string, inMain: boolean, inAll: boolean}} pass
 */

function dressChannelRow(row: HTMLElement, { tally, answered, viewing, inMain, inAll }: DressPass): void {
  const id = row.getAttribute("data-id") || "";
  drawThreadChips(row, tally, inMain, inAll);
  // Every message the row stands for. One of them on an ordinary row; see the combining note.
  const ids = idsOf(row);
  // DIMS, never hides. See the note at the head of this section: this is derived from what
  // happens to be loaded, and evidence that admits it is incomplete must not remove anything.
  setAttributeIfChanged(row, "data-replied", ids.some((one) => answered.has(one)) ? "true" : "false");
  // The speaker treatment, on the same pass and from the same census. `me` and `coder` are drawn
  // as the transcript's two speakers; see web/voice.css.
  setAttributeIfChanged(row, "data-who",
    whoIsWho.bucketFor(
      row.getAttribute("data-author-id"),
      row.getAttribute("data-author-bot") === "true",
      viewing
    )
  );
  // GREYS, never hides, and that is the whole distinction between this and the To do filter.
  // Being archived is DECLARED by the reader, so unlike `data-replied` it is not evidence that
  // might be incomplete — but a channel view that removed the row would leave the reader no way
  // to see what they had archived, and no way to change their mind about it.
  const who = row.getAttribute("data-who");
  // EVERY constituent, or not at all. A row half of whose messages have been archived shows as
  // UNREAD: the other reading files an unseen message away behind one the reader has dealt with,
  // and the whole point of the archive is that it says what is left.
  const isArchived = ids.length > 0 && ids.every((one) => doneState().archived.has(one));
  setAttributeIfChanged(row, "data-archived", isArchived ? "true" : "false");
  // `#245 done-feedback`. The moment of a Done, on whichever row now stands for the message.
  drawDoneFlash(row);
  drawRowPin(row);
  // `#196 auto-read-noise`. Read automatically, by the server's verdict on EVERY constituent —
  // `joinsGroup` keeps a placeholder out of a real message's row, so a mixed row is not one the
  // reader should be told is noise. Dimmed, never hidden, here: hiding is Hide read's job, and
  // a row that vanished could not be rescued from a rule that caught it by mistake.
  const held = rowMessages(row);
  const isNoiseRow = held.length > 0 && held.every(isNoise);
  setAttributeIfChanged(row, "data-noise", !isArchived && isNoiseRow ? "true" : "false");
  const rescue = notNoiseButtonOf(row);
  if (rescue) setHiddenIfChanged(rescue, !isNoiseRow);
  // `#549 own-read-stored`. The reader's own words are no case of their own here: "Mark my own
  // messages as read" decides where their read state STARTS, on the server, and the row is drawn from
  // that state like any other — archived, with Unarchive, or not, with Done.
  const isReading = readAloudOwner().nowPlaying !== null && (readAloudOwner().nowPlaying as { id: unknown }).id === id;
  // ASKED FOR, but not yet speaking. Distinct from `data-reading` on purpose: this one the
  // reader can still change their mind about, and it is the only thing on screen during a wait
  // that is entirely somebody else's network.
  setAttributeIfChanged(row, "data-pending", readAloudOwner().pendingRead === id ? "true" : "false");
  // IS THIS MESSAGE ITSELF AN ANSWER? The opposite direction from `data-replied`, and the more
  // reliable one: that is derived from whatever happens to be loaded and misses an answer
  // further back than the window, whereas the pointer for this is ON the message. Correct
  // whether or not the message it answers is anywhere on screen.
  setAttributeIfChanged(row, "data-is-reply", row.getAttribute("data-reply-to") ? "true" : "false");
  // WHICH ROW IS SPEAKING. Without it the reader taps, waits, and has nothing but the sound to
  // tell them which message they hit — on a list where the next act archives it.
  setAttributeIfChanged(row, "data-reading", isReading ? "true" : "false");
  // THE ROW BEING READ STAYS OPEN.
  //
  // The owner watched a message he was listening to collapse mid-read and assumed a stray tap.
  // It was not: the channel re-reads itself every DISCORD_POLL_MS, `applyNewestPage` rebuilds
  // every row, and a freshly built row starts folded. So a long message being read aloud folded
  // itself on the next poll, every time, while its own audio was still playing. Hearing the
  // whole message and being shown a clamped third of it is the wrong pair.
  // The row's own fold, on a row drawn again from a held view as on one just built. `#301`.
  const entry = (row as HTMLElement & { foldEntry?: unknown }).foldEntry;
  if (isReading && entry && isFolded(entry)) {
    setFolded(entry, false);
  }

  // The row's own way back, labelled for what it will DO rather than for what the row is. In the
  // To do filter an archived row is never on screen, so this only ever reads "Done" there.
  // WHO, printed only where the colour cannot say it. `me` and `coder` are drawn as the
  // transcript's two speakers, so their names are a line of chrome restating what the row's own
  // colour already said. A third party is one of many and has to be named.
  // THE AUTHOR LINE, decided ONCE. It used to be set twice — the name here and `reading` in a
  // later block — and the later write is the one that lost, so the row being read never said so.
  //
  // While the audio runs, `reading` replaces the name: that is the fact the reader wants at a
  // glance on a list where the next thing that happens is the row archiving itself. It is shown
  // even for a principal whose colour already names them, because hiding it would leave the row
  // saying nothing at all.
  const named = childByClass(row, "msg-author");
  if (named) {
    setClassNameIfChanged(named, isReading ? "msg-author reading-mark" : "msg-author");
    // `#376 read-aloud-blocks`: and how far through, "reading 2 of 5", in a read of several parts.
    setTextIfChanged(named, isReading ? readAloudOwner().readingLabel() : authorLabel(row));
    setHiddenIfChanged(named, !isReading && (who === "me" || who === "coder"));
    // `#377 compact-collapsed-rows`. Where the header line has no room for the whole name it shows a
    // prefix and an ellipsis (web/voice.css); the whole name is here, for a pointer resting on it.
    const whole = isReading ? "" : authorLabel(row);
    if ((named.getAttribute("title") || "") !== whole) {
      if (whole) named.setAttribute("title", whole);
      else named.removeAttribute("title");
    }
  }
  // The block being read, lit again on a row the poll has just rebuilt. `#376`.
  readAloudOwner().markReadingBlock(row);
  const done = doneButtonOf(row);
  if (done) {
    // Written every time, unlike the rest of this dress (`#532 shift-o-latency`): with this text too left
    // alone where unchanged, a reply's Send moved its row down the list by the box it closed
    // (tests/reply_continuity_browser.py). The place the Done keeps (`markRowDone`) depends on it.
    done.textContent = isArchived ? "Unarchive" : "Done";
    setAttributeIfChanged(
      done,
      "title",
      isArchived
        ? "Put this back in the list. This does not change the source chat service."
        : "Mark as dealt with here. This does not change the source chat service."
    );
  }
}

function dressUnpackedRows(): void {
  if (!unpackOnScreen()) return;
  inColumn(columns.side, () => {
    const rows = ([...leftEl("side-cards").children] as HTMLElement[])
      .filter((row) => row.hasAttribute("data-unpack-thread") && row.hasAttribute("data-id"));
    if (rows.length === 0) return;
    const viewing = String(selectedChannelId());
    const pass = { tally: replyTally(), answered: answeredBy(rows, viewing), viewing, inMain: false, inAll: false };
    for (const row of rows) dressChannelRow(row, pass);
  });
}

/**
 * Swipe a row away, on a device that has swipes.
 *
 * TOUCH AND PEN ONLY. A horizontal mouse drag across a message is how a person SELECTS TEXT, and
 * this list exists so that a specific real message can be quoted and checked — taking that gesture
 * away to save a pointer user one click would be a bad trade. The button in the row's meta line is
 * the way in on a desktop, and it is present on every device, so nothing is reachable only by
 * gesture.
 */
/**
 * What the row stopped printing, shown on the row that was asked about.
 *
 * Inline rather than a dialog: the reader is holding a finger on one row of a list, and a modal
 * would take the list away to answer a question about it. Toggled, so the same gesture closes it.
 *
 * `textContent` throughout — an author name and a channel's own timestamp are third-party text,
 * and this is the one place they are shown in full.
 */
// --- the reading bookmark -----------------------------------------------------------------------
//
// `#240 reading-bookmark`. The owner, catching up on a long backlog: "I want to be able to mark my
// place, reply to messages, jump back to where I was reading in the transcript before my reply,
// mark as done when I'm done, and advance the bookmark." So each channel has ONE bookmark: a line
// in the accent colour drawn across the list just above one message. Shift+B puts it above the
// selected message (and takes it away when pressed on the message right under it); b, or m, the key
// of the place marker it replaces, goes back to it, and so does the Bookmark chip beside Newest.
//
// KEPT ON THE SERVER, so it follows the owner between his phone and his desk. Every channel read
// carries it, and this page keeps a copy per channel that it changes at once and writes behind the
// Done writes (`queueInboxWrite`). A write that fails is taken back; a conditional one that lost to a
// move made on the other device takes where the server says the bookmark is now.
//
// IT MARKS NOTHING, AND ONLY THE OWNER MOVES IT. Setting it never marks a message Done. Besides his
// Shift+B, the one thing that moves it is a Done of his own: on the message right under the line,
// which slides it down to the next message still to read (`singleDoneBump`), or a Done through here
// at or below it (`throughBump`); in a thread, never past a message of Main's still to read
// (`heldBackByMain`). Those moves are conditional (`if_revision`), so they can never
// undo a move made on the other device meanwhile, and Undo takes them back too. A message arriving
// never moves it, and nor does putting one back.
//
// It replaces the place marker this browser kept in localStorage under `MARKER_KEY`, which moved
// itself past read rows on every redraw. One left over is moved to the server once, the first time
// its message is drawn in a channel with no bookmark (`adoptLegacyMarker`).

/**
 * Each channel's bookmark as this page holds it, and the bookkeeping of this page's writes of it
 * (web/src/core/annotations `createBookmarks`, `HeldBookmark`), private to its owner; the epoch both
 * the bookmark's and Done's writes move is this page's (`readStateEpoch`). Here where the maps stood.
 *
 * @typedef {import("./src/core/annotations/index.ts").HeldBookmark} HeldBookmark
 */

  return { answeredBy, dressChannelRow, dressUnpackedRows };
}

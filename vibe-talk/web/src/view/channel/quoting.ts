// Quoting part of a message in the reply to it, the Quoted reply chip under a selection, and the quick
// reply that chip opens (`#267 quote-reply`, `#726 vt-selection-chip`, `#727 vt-selection-quick-reply`;
// `#624 voice-page-modularity`, moved from the legacy script into view/channel). The one owner of the
// selection Quote reply would quote, the chip's place, the open quick reply, its drafts and the list it
// holds still. Where a reply posts, the outbox, and the row's Reply label are the page's, passed in.
//
// `#267 quote-reply`. The owner, 2026-10-13: "when I highlight part of a message, the Reply button
// turns into 'Quote Reply' ... Clicking that button replies ... but it prepends `> SELECTED TEXT FOR
// QUOTATION` and a blank line to the text input box where I type in the rest. This is a more
// targeted quote reply than google chat's notion of using the whole message."
//
//   * WHAT IS QUOTED: text selected inside ONE message's drawn body. Not a selection across two rows,
//     nor one reaching into the row's header line or its buttons, a message box, or the page's own
//     controls: those leave every Reply as it was. A range that runs past the body without taking in
//     a word drawn outside it is the body's, as a triple-click leaves one (`rowQuotedBy`).
//   * THE BUTTON SAYS WHAT A PRESS DOES. While there is such a selection, that row's Reply reads
//     "Quote reply" ("Quote reply (2)" where it counts replies), in the accent. The label and the press
//     read one record, `quoteHeld`, so they cannot disagree. It never moves the message's words: where
//     the header line has no room for the longer label it reads "Quote" at Reply's width instead
//     (`drawReplyLabel`).
//   * NO NEW WAY TO POST. The press opens the reply Reply opens, routed as Reply routes on that
//     provider (`replyRoute`): a reply by pointer where the provider has one, and on Slack, which has
//     none, the message's thread. Only the box's first lines differ.
//   * A FINGER'S TAP CAN TAKE THE SELECTION FIRST. A phone's browser may clear the selection when the
//     Reply under it is tapped, before the tap arrives as a click. So after a touch, a selection that
//     goes away is kept for `QUOTE_HOLD_MS`, the label with it, and a press in that time still quotes.
//     After the mouse or the keys it goes at once: Chromium keeps a selection through a click on a
//     button (measured, tests/reply_continuity_browser.py), so there the label says only what is true.
//   * THE QUOTE is the selection's text as the browser draws it: each run of spaces made one, each
//     line trimmed, no more than one blank line together, the ends trimmed, cut at `QUOTE_MAX_CHARS`
//     ending "…". Every line is led by "> ", a blank line inside it by ">", and one empty line follows,
//     with the caret after it in the box, focused to write the rest. A draft already written to the
//     message stays, under the quote; a quote left untouched is not a draft, so Cancel keeps the
//     draft that was there before and nothing else.
//   * NOT A GESTURE ON THE ROW. Dragging over a message's words neither folds it nor reads it aloud
//     (`tapRow` passes over a click that ends a selection), and the click a drag ends with selects the
//     row it was in, as any click in a row does (`#239 single-selection`): so `r` then quotes too. Text
//     selected in another message than the selected one is not `r`'s: `r` answers the message it acts
//     on, and plainly.
//
// `#726 vt-selection-chip`. The owner, 2026-11-01: words selected in a message offer the page's own
// "Quoted reply" chip right under them, where the thumb that selected them already is, as well as the
// row's Reply turned Quote reply (above). Under, because a phone's own action bar (Copy, Look Up, ...)
// stands over a selection. The chip is the row's Quote reply: it reads the one record the label reads
// (`quoteHeld`), so it shows exactly while that Reply would quote, a touch's held moment included, and
// a press of it is a press of that Reply. Placed from the selection's own box
// (`Range.getBoundingClientRect`), centred under it and kept inside the window, in the layout
// viewport's coordinates, as the document never scrolls; a list scrolled under it moves it with the
// words, and a selection a touch let go of leaves it where it was until the hold ends.
//
// `#727 vt-selection-quick-reply`. The owner, 2026-11-01: the chip opens a FLOATING box under the
// selection, not the reply box in the stream. Several replies to different sentences of one long
// message in a row is the use: select a sentence, tap Quoted reply, write, Send, and the next.
//
//   * NOTHING MOVES. The box floats over the list, under the words, kept inside what the window shows
//     above an on-screen keyboard (the visual viewport). The list under it is not scrolled to open it,
//     to send it, nor when the reply or anything answering it arrives: while the box is open, and
//     after a send until the reader moves the list themselves, the list does not follow the newest
//     message (`followsNewest`) nor keep its foot as the window resizes. The focus goes into the box
//     with `preventScroll`.
//   * WHAT IT SENDS is a quoted reply, the words as Quote reply quotes them (`quoteBlock`) and then
//     what was written, in that message's thread: Reply's routing with the thread bound
//     (`replyRoute(message, true)`), so it joins the thread the message heads or starts it, and on a
//     provider with no threads answers by pointer. It goes through the same outbox as every send, and
//     marks the message read as Reply does, unless its quote covers too little of it (`replyMarksRead`).
//   * ESC closes it, keeping what was written for the same words of the same message: the chip on that
//     selection again brings it back. Send clears it.
//   * A TAP OUTSIDE NEVER HIDES WHAT WAS WRITTEN. The owner, 2026-11-02: "if I accidentally click anywhere
//     else on the screen, my draft is lost. ... if it's hidden from the UI, I have no way to discover it.
//     So I think if I click anywhere else in the screen my drafted reply should go back to the normal
//     position under the current message and should be open and visible." With words in it, the box's
//     quote and words become the message's reply draft and its reply opens in the stream under it, as
//     Reply opens it (`putDownQuickReply`); with none, the box just closes.
//   * NEVER LOST. `#819 voice-page-quoted-reply-and-draft-chip`, after the owner lost a dictated reply: what
//     is written in the box is kept on the device as it is written, not only as the box closes, so a reload
//     or a crashed tab with the box open still has it; each write is told to the page (`noteDraft`), whose
//     Draft chip finds it again and opens the box on its message (`openQuickDraft`).

import type { Message } from "../../contract/index.ts";
import { quoteBlock } from "../../present/rows/index.ts";
import { nodeWithin, setHiddenIfChanged, setTextIfChanged } from "../dom/index.ts";
import type { ElementById } from "../dom/index.ts";
import { rowQuotedBy } from "./quotes.ts";
import { rowMessages } from "./rows.ts";

/** How long after a touch a selection that went away is still quoted, in milliseconds. */
const QUOTE_HOLD_MS = 2000;

/** Reply's tooltip while it is Quote reply. No channel data. */
const QUOTE_REPLY_TITLE = "Reply to this message, starting with the words you selected in it quoted";

/** How far under the selection the chip stands after the mouse or the keys, in CSS pixels. */
const SELECTION_CHIP_GAP_PX = 14;

/**
 * `#819 voice-page-quoted-reply-and-draft-chip`. How far under the selection the chip stands after a touch,
 * in CSS pixels: clear of the selection handles the phone's browser hangs under the words' two ends, which
 * take a touch on them for themselves, over anything the page draws.
 */
const SELECTION_CHIP_HANDLES_PX = 36;

/**
 * `#819 voice-page-quoted-reply-and-draft-chip`. The room the phone's own bar for a selection (Copy, Share,
 * Select all) takes over the words, in CSS pixels. With less than this above them, the browser puts the bar
 * under them instead, past the handles, where the chip would stand; so the chip stands under the bar.
 */
const SELECTION_BAR_ROOM_PX = 72;

/** How close to the window's edges the chip may stand, in CSS pixels. */
const SELECTION_CHIP_EDGE_PX = 8;

/** How far under the selection the box stands, in CSS pixels, as the chip did. */
const QUICK_REPLY_GAP_PX = 10;

/** The widest the box is drawn, in CSS pixels: a comfortable line at a desk, the window on a phone. */
const QUICK_REPLY_MAX_WIDTH_PX = 560;

/** A row Quote reply may quote, with the Reply it draws. */
export type QuotedRow = HTMLElement & { replyButton?: HTMLElement };

/**
 * The selection a press of its row's Reply quotes: the row, its Reply, the words as drawn where they
 * have been read (after a touch; null after the mouse or the keys, which read them when Reply is
 * pressed), and the timer that lets it go when a touch has taken the selection away.
 */
export interface HeldQuote {
  row: QuotedRow;
  button: HTMLElement | undefined;
  text: string | null;
  timer: ReturnType<typeof setTimeout> | null;
}

/** A box in the viewport's pixels. */
export interface Box {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

/** Where a reply to a message posts, as the page's `replyRoute` says. */
export interface QuoteRoute {
  readonly thread: string | null;
  readonly branch: string | null;
  readonly into: string | null;
  readonly unknown: boolean;
  readonly removed: boolean;
}

/** A send handed to the outbox, as the page's `queueOutgoingMessage` takes it. */
export interface QuickReplySend {
  readonly channel: string;
  readonly threadId: string | null;
  readonly replyTo: string;
  readonly text: string;
  readonly marksRead: string | null;
  readonly placement?: "unknown";
}

/** Where the quick replies' drafts are kept, by key, as the page's draft stores keep theirs. */
export interface QuickReplyDraftStore {
  get(key: string): string | undefined;
  set(key: string, text: string): void;
  delete(key: string): void;
  /** Write them to the device; whether it kept them. */
  save(): boolean;
}

/** What quoting asks of the page. */
export interface QuotingDependencies {
  /** The element with this id, as the page's own lookup finds it. */
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** How the reader last moved: "touch", "mouse", "keys" or "". */
  inputKind(): string;
  /** Draw the Reply `button` again: Reply, or Quote reply while its row's words are held. */
  drawReplyLabel(button: HTMLElement): void;
  /** The channel on screen, or "". */
  selectedChannelId(): string;
  /** Where a reply to `message` posts, with its thread bound where `bound`. */
  replyRoute(message: Message, bound: boolean): QuoteRoute;
  /** Whether channel `channel` is known read-only. */
  readOnly(channel: string): boolean;
  /** What a reply to `message` with text `text` marks read once sent, or null. */
  replyMarksRead(message: Message, text: string): string | null;
  /** Hand a send to the outbox; resolves when it is done with. */
  queueOutgoingMessage(send: QuickReplySend): Promise<unknown>;
  /**
   * Put a quick reply's words back under the message they answer: `text`, the quote and what was written,
   * as `message`'s reply draft, its reply open and shown in the stream under `row`, the row the box was
   * opened on (or the row drawing that message now).
   */
  toStream(row: QuotedRow, message: Message, text: string): void;
  /**
   * `#819 voice-page-quoted-reply-and-draft-chip`. Where quick replies closed unsent are kept, by message and
   * quoted words, on the device.
   */
  readonly drafts: QuickReplyDraftStore;
  /** A quick reply's draft `key`, in channel `channel`, was written, and holds words or not now. */
  noteDraft(key: string, channel: string, written: boolean): void;
  /** `#821 voice-page-read-aloud-react`. The box has closed, however it did: whatever listens into it stops. */
  closed(): void;
}

/** The quoting owner's commands. */
export interface Quoting {
  /** The selection held for Quote reply, or null. */
  held(): HeldQuote | null;
  /** Hold `next` as the selection to quote, or nothing, and redraw the Reply of each row it changes. */
  holdQuote(next: HeldQuote | null): void;
  /** `selectionchange`, and every press of a Reply: what Quote reply would quote now. */
  noteQuoteSelection(): void;
  /** What a press of `button` quotes, or "" for a plain Reply; taken, and cleared from the page. */
  takeQuote(button: unknown): string;
  /** Place the chip at the next frame. */
  placeSelectionChip(): void;
  /** A press of the chip: the quick reply to the words it stands under. */
  pressSelectionChip(): void;
  /** Whether `area` is held still for a quick reply. */
  quickReplyHolds(area: unknown): boolean;
  /** The held list moved (`target` scrolled): an open box goes with its words, a closed one lets go. */
  quickReplyListMoved(target: unknown): void;
  /** Whether a quick reply is open. */
  quickReplyOpen(): boolean;
  /** Place the open box under the words it quotes, inside what the window shows. */
  placeQuickReply(): void;
  /** Close the quick reply, keeping what was written for the same words. */
  closeQuickReply(): void;
  /** A tap outside the box: what was written goes back under its message, open and shown; nothing written, it closes. */
  putDownQuickReply(): void;
  /** Send the quick reply, then close it. */
  sendQuickReply(): Promise<void>;
  /** `#819`. Something was written in the open box: kept now, on the device. */
  noteQuickReplyInput(): void;
  /** `#819`. Whether quick reply draft `key` still holds words. */
  quickDraftLive(key: string): boolean;
  /** `#819`. The message quick reply draft `key` answers, by id. */
  quickDraftMessage(key: string): string;
  /** `#819`. Open the quick reply kept under `key` on `row`, the row drawing its message: under the row's top. */
  openQuickDraft(key: string, row: QuotedRow): void;
  /**
   * `#821 voice-page-read-aloud-react`. React: the quick reply to message `id`, drawn by `row`, quoting `quote`,
   * the words a paused read held, under `box`, where they stood (or the row's top). Its draft is kept and
   * found as any quick reply's.
   */
  openSentenceReply(row: QuotedRow, id: string, quote: string, box: Box | null): void;
}

/** Nothing held, no box open. */
export function createQuoting(deps: QuotingDependencies): Quoting {
  let quoteHeld: HeldQuote | null = null;
  /** The animation frame the chip's next placing waits for, or null. */
  let selectionChipFrame: number | null = null;
  /** How far under the words the chip was last placed (`drawSelectionChip`). */
  let selectionChipGap = SELECTION_CHIP_GAP_PX;
  /**
   * The quick reply open now, or null: the row and message it answers, its channel, the words it quotes,
   * the key its draft is kept under, and the selection's box it stands under.
   */
  let quickReply: { row: QuotedRow; message: Message; channel: string; quote: string; key: string; box: Box | null } | null = null;
  /** What was written in quick replies closed unsent, by message and quoted words (`quickReplyKey`). */
  const quickReplyDrafts = deps.drafts;
  /**
   * The list a quick reply holds still, and where it was: from the box's opening until the reader moves
   * it, the list neither follows the newest message nor keeps its foot. `rows` is the list of rows kept
   * from growing shorter meanwhile, or null. Null when nothing is held.
   */
  let quickReplyHold: { area: HTMLElement; top: number; rows: HTMLElement | null } | null = null;

  /**
   * What is selected on the page, for Quote reply: `found`, the row where every range of the selection
   * is that one row's (`rowQuotedBy`) and holds more than spaces, with its words as drawn after a touch;
   * `stray`, whether something else is selected. Neither, where nothing is.
   *
   * Every `selectionchange` asks, which is every move of the mouse in a drag. So the words are told from
   * spaces by each range's own text, which reads no layout, and the words as drawn, which bring the
   * layout up to date first, are read here only after a touch: a phone may take the selection away
   * before its tap on Reply arrives. After the mouse or the keys they are read once, when Reply is
   * pressed (`takeQuote`).
   */
  function quotableSelection(): { found: { row: QuotedRow; text: string | null } | null; stray: boolean } {
    const selection = typeof getSelection === "function" ? getSelection() : null;
    if (!selection || selection.isCollapsed || !(selection.rangeCount > 0)) return { found: null, stray: false };
    let row: QuotedRow | null = null;
    let words = false;
    for (let at = 0; at < selection.rangeCount; at += 1) {
      const range = selection.getRangeAt(at);
      const own = rowQuotedBy(range) as QuotedRow | null;
      if (!own || (row !== null && own !== row)) return { found: null, stray: true };
      row = own;
      words = words || /\S/.test(String(range));
    }
    if (!words || row === null) return { found: null, stray: true };
    return { found: { row, text: deps.inputKind() === "touch" ? String(selection) : null }, stray: false };
  }

  function holdQuote(next: HeldQuote | null): void {
    const was = quoteHeld;
    if (was && was.timer !== null) clearTimeout(was.timer);
    quoteHeld = next;
    const button = next ? next.button : null;
    if (was && was.button && was.button !== button) deps.drawReplyLabel(was.button);
    if (button && (!was || was.button !== button)) deps.drawReplyLabel(button);
    placeSelectionChip();
  }

  function noteQuoteSelection(): void {
    const { found, stray } = quotableSelection();
    if (found) {
      if (!quoteHeld || quoteHeld.row !== found.row || quoteHeld.timer !== null) {
        holdQuote({ row: found.row, button: found.row.replyButton, text: found.text, timer: null });
      } else {
        quoteHeld.text = found.text;
      }
    } else if (quoteHeld && (stray || deps.inputKind() !== "touch")) {
      holdQuote(null);
    } else if (quoteHeld && quoteHeld.timer === null) {
      quoteHeld.timer = setTimeout(() => holdQuote(null), QUOTE_HOLD_MS);
    }
    placeSelectionChip();
  }

  /**
   * What a press of `button` quotes, or "" for a plain Reply: the words as drawn of the selection held
   * for its row, read now where they are still selected, else as a touch last read them; taken, and
   * cleared from the page.
   */
  function takeQuote(button: unknown): string {
    noteQuoteSelection();
    // `noteQuoteSelection` may have let it go; read it again.
    const held = quoteHeld as HeldQuote | null;
    if (!held || held.button !== button) return "";
    const selection = typeof getSelection === "function" ? getSelection() : null;
    const text = held.timer === null && selection ? String(selection) : held.text || "";
    holdQuote(null);
    if (selection && typeof selection.removeAllRanges === "function") selection.removeAllRanges();
    return text;
  }

  /** The box of what is selected now, in the viewport's pixels, or null where nothing is or it has no box. */
  function selectionBox(): Box | null {
    const selection = typeof getSelection === "function" ? getSelection() : null;
    if (!selection || selection.isCollapsed || !(selection.rangeCount > 0)) return null;
    let box = null as Box | null;
    for (let at = 0; at < selection.rangeCount; at += 1) {
      const range = selection.getRangeAt(at);
      if (!range || typeof range.getBoundingClientRect !== "function") continue;
      const rect = range.getBoundingClientRect();
      if (!(rect.width > 0 || rect.height > 0)) continue;
      const was: Box | null = box;
      box = was === null ? { top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right } : {
        top: Math.min(was.top, rect.top), bottom: Math.max(was.bottom, rect.bottom),
        left: Math.min(was.left, rect.left), right: Math.max(was.right, rect.right),
      };
    }
    return box;
  }

  /** Place the chip at the next frame: every `selectionchange` of a drag asks, and one placing a frame is enough. */
  function placeSelectionChip(): void {
    if (typeof requestAnimationFrame !== "function") {
      drawSelectionChip();
      return;
    }
    if (selectionChipFrame !== null) return;
    selectionChipFrame = requestAnimationFrame(() => {
      selectionChipFrame = null;
      drawSelectionChip();
    });
  }

  /** Show the chip under the selection Quote reply would quote, or hide it where there is none. */
  function drawSelectionChip(): void {
    const chip = deps.byId("selection-chip");
    const box = quoteHeld ? selectionBox() : null;
    // A touch's held selection, gone from the page, keeps the chip where it was until the hold ends.
    // `#819 voice-page-quoted-reply-and-draft-chip`. And its twin in the read's strip, which nothing covers.
    const twin = deps.byId("read-steer-quote");
    if (!quoteHeld || (box === null && chip.hidden)) {
      setHiddenIfChanged(chip, true);
      setHiddenIfChanged(twin, true);
      return;
    }
    setHiddenIfChanged(chip, false);
    setHiddenIfChanged(twin, false);
    if (box === null) return;
    const size = chip.getBoundingClientRect();
    const width = Number(size.width) || 0;
    const height = Number(size.height) || 0;
    const viewWidth = (document.documentElement && document.documentElement.clientWidth) || 0;
    const viewHeight = window.innerHeight || 0;
    const left = Math.max(SELECTION_CHIP_EDGE_PX,
      Math.min((box.left + box.right) / 2 - width / 2, viewWidth - SELECTION_CHIP_EDGE_PX - width));
    // `#819 voice-page-quoted-reply-and-draft-chip`. After a touch, what the browser draws for the selection
    // takes touches on it over every layer of the page: the handles under the words, and the bar of Copy
    // and Share, which goes under them where there is no room over them. The chip stands clear of both.
    const touch = deps.inputKind() === "touch";
    const visual = window.visualViewport;
    const shownTop = visual ? Number(visual.offsetTop) || 0 : 0;
    const barUnder = touch && box.top - shownTop < SELECTION_BAR_ROOM_PX;
    selectionChipGap = !touch ? SELECTION_CHIP_GAP_PX : SELECTION_CHIP_HANDLES_PX + (barUnder ? SELECTION_BAR_ROOM_PX : 0);
    let top = Math.max(SELECTION_CHIP_EDGE_PX,
      Math.min(box.bottom + selectionChipGap, viewHeight - SELECTION_CHIP_EDGE_PX - height));
    // No room under the words for that: over them, above where the bar stands there.
    if (touch && top < box.bottom + selectionChipGap) {
      top = Math.max(SELECTION_CHIP_EDGE_PX, box.top - SELECTION_BAR_ROOM_PX - height);
      selectionChipGap = top - box.bottom;
    }
    chip.style.left = `${Math.round(left)}px`;
    chip.style.top = `${Math.round(top)}px`;
  }

  function pressSelectionChip(): void {
    const held = quoteHeld;
    if (held && held.button && held.button.isConnected !== false) openQuickReply(held);
    else drawSelectionChip();
  }

  /**
   * Hold `area` still for a quick reply. Its rows are kept from growing shorter than they are now
   * (`min-height`): rows drawn again shorter under a reader at the list's foot — two replies of the
   * reader's combined into one row, a long one folded — would otherwise take the foot up with them, and
   * the browser's clamp to the new foot would drop every word on screen by what they lost.
   */
  function holdQuickReplyList(area: HTMLElement): void {
    if (quickReplyHold && quickReplyHold.area === area) {
      quickReplyHold.top = area.scrollTop;
      return;
    }
    releaseQuickReplyHold();
    const rows = area === deps.byId("scroll-area") ? deps.byId("discord-log") : null;
    if (rows) rows.style.minHeight = `${Math.ceil(Number(rows.getBoundingClientRect().height) || 0)}px`;
    quickReplyHold = { area, top: area.scrollTop, rows };
  }

  /** Let go of the list a quick reply held, and of the height its rows were kept at. */
  function releaseQuickReplyHold(): void {
    if (quickReplyHold && quickReplyHold.rows) quickReplyHold.rows.style.minHeight = "";
    quickReplyHold = null;
  }

  /** The key a quick reply's draft is kept under: its channel, the message, and the words it quotes. */
  const quickReplyKey = (channel: string, id: string, quote: string): string => `${channel}\n${id}\n${quote}`;

  /** A key's parts, as `quickReplyKey` joined them: the quote may itself hold line breaks. */
  const keyParts = (key: string): { channel: string; id: string; quote: string } => {
    const [channel = "", id = "", ...quote] = key.split("\n");
    return { channel, id, quote: quote.join("\n") };
  };

  /** Keep what is written under `key` now, on the device, and tell the page where it changed. */
  function keepQuickDraft(key: string, channel: string, written: string): void {
    // Opened and put away again unchanged, a draft is no newer than it was.
    if ((quickReplyDrafts.get(key) || "") === (written.trim() ? written : "")) return;
    if (written.trim()) quickReplyDrafts.set(key, written);
    else quickReplyDrafts.delete(key);
    quickReplyDrafts.save();
    deps.noteDraft(key, channel, written.trim() !== "");
  }

  /** Whether `area` is held still for a quick reply (`quickReplyHold`). */
  const quickReplyHolds = (area: unknown): boolean => quickReplyHold !== null && quickReplyHold.area === area;

  /** The list a row is drawn in: the right column's or the left's. */
  const listOf = (row: Node): HTMLElement => (nodeWithin(deps.byId("side-scroll"), row) ? deps.byId("side-scroll") : deps.byId("scroll-area"));

  /**
   * The held list moved, which only the reader does while it is held: an open box goes with the words it
   * stands under, and once the box is closed the list is the reader's again.
   */
  function quickReplyListMoved(target: unknown): void {
    const hold = quickReplyHold;
    if (hold === null || target !== hold.area || hold.area.scrollTop === hold.top) return;
    if (!quickReply) {
      releaseQuickReplyHold();
      return;
    }
    const moved = hold.area.scrollTop - hold.top;
    hold.top = hold.area.scrollTop;
    const box = quickReply.box;
    if (box) quickReply.box = { ...box, top: box.top - moved, bottom: box.bottom - moved };
    placeQuickReply();
  }

  /**
   * Open the quick reply to `held`, the selection the chip stands for: the words it quotes read now, the
   * selection let go, and the box drawn under where it was, with any draft kept for the same words.
   */
  function openQuickReply(held: HeldQuote): void {
    const box = selectionBox() || (quickReply ? quickReply.box : null) || chipBox();
    const quote = takeQuote(held.button).trim();
    const message = rowMessages(held.row)[0];
    if (!message || quote === "") return;
    openQuickReplyFor(held.row, message, quote, box);
  }

  /**
   * Open the quick reply to `message`, drawn by `row`, quoting `quote`, under `box`: with any draft kept for
   * the same words, focused, the caret at its end. The chip's press and the Draft chip both come here.
   */
  function openQuickReplyFor(row: QuotedRow, message: Message, quote: string, box: Box | null): void {
    if (quickReply) closeQuickReply();
    const channel = String(deps.selectedChannelId() || "");
    const key = quickReplyKey(channel, String(message.id), quote);
    quickReply = { row, message, channel, quote, key, box };
    holdQuickReplyList(listOf(row));
    const form = deps.byId("quick-reply");
    setTextIfChanged(deps.byId("quick-reply-quote"), quote);
    deps.byId("quick-reply-text").value = quickReplyDrafts.get(key) || "";
    setTextIfChanged(deps.byId("quick-reply-state"), "");
    setHiddenIfChanged(form, false);
    placeQuickReply();
    const text = deps.byId("quick-reply-text");
    text.focus({ preventScroll: true });
    const end = text.value.length;
    if (typeof text.setSelectionRange === "function") text.setSelectionRange(end, end);
  }

  /** The chip's own box, where a touch let the selection go before the press: the box opens under it. */
  function chipBox(): Box | null {
    const chip = deps.byId("selection-chip");
    if (chip.hidden) return null;
    const rect = chip.getBoundingClientRect();
    const top = rect.top - selectionChipGap;
    return { top, bottom: top, left: rect.left, right: rect.right };
  }

  /**
   * Place the open box under the words it quotes, as wide as the window allows up to
   * `QUICK_REPLY_MAX_WIDTH_PX`, and inside what the window shows: above an on-screen keyboard, over the
   * words rather than under the keyboard where there is no room below them.
   */
  function placeQuickReply(): void {
    if (!quickReply) return;
    const form = deps.byId("quick-reply");
    const viewWidth = (document.documentElement && document.documentElement.clientWidth) || 0;
    const visual = window.visualViewport;
    const shownTop = visual ? Number(visual.offsetTop) || 0 : 0;
    const shownBottom = visual ? shownTop + (Number(visual.height) || 0) : window.innerHeight || 0;
    const width = viewWidth > 0 ? Math.min(QUICK_REPLY_MAX_WIDTH_PX, viewWidth - 2 * SELECTION_CHIP_EDGE_PX) : QUICK_REPLY_MAX_WIDTH_PX;
    form.style.width = `${Math.round(width)}px`;
    const height = Number(form.getBoundingClientRect().height) || 0;
    const box = quickReply.box || { top: shownTop, bottom: shownTop, left: 0, right: viewWidth };
    const left = Math.max(SELECTION_CHIP_EDGE_PX,
      Math.min((box.left + box.right) / 2 - width / 2, viewWidth - SELECTION_CHIP_EDGE_PX - width));
    const top = Math.max(shownTop + SELECTION_CHIP_EDGE_PX,
      Math.min(box.bottom + QUICK_REPLY_GAP_PX, shownBottom - SELECTION_CHIP_EDGE_PX - height));
    form.style.left = `${Math.round(left)}px`;
    form.style.top = `${Math.round(top)}px`;
  }

  /** Close the quick reply, keeping what was written for the same words (`quickReplyDrafts`). */
  function closeQuickReply(): void {
    if (!quickReply) return;
    keepQuickDraft(quickReply.key, quickReply.channel, deps.byId("quick-reply-text").value);
    quickReply = null;
    deps.closed();
    setHiddenIfChanged(deps.byId("quick-reply"), true);
    deps.byId("quick-reply-text").blur();
  }

  /**
   * A tap outside the open box. Nothing written: it closes. Words written: they go under their quote into
   * the message's reply in the stream, open and shown under it (`toStream`), and the box closes holding
   * nothing for those words, the draft being the stream reply's now, where it can be seen.
   */
  function putDownQuickReply(): void {
    const open = quickReply;
    if (!open) return;
    const written = deps.byId("quick-reply-text").value;
    if (!written.trim()) {
      closeQuickReply();
      return;
    }
    // Into the stream's draft first, and only then let go of its own (`#819`): never in neither.
    deps.toStream(open.row, open.message, `${quoteBlock(open.quote)}${written}`);
    deps.byId("quick-reply-text").value = "";
    closeQuickReply();
  }

  /** Send the quick reply: the quoted words and what was written, in the message's thread; then close it. */
  async function sendQuickReply(): Promise<void> {
    const open = quickReply;
    if (!open) return;
    const written = deps.byId("quick-reply-text").value;
    if (!written.trim()) {
      setTextIfChanged(deps.byId("quick-reply-state"), "Nothing to send yet — write something first.");
      return;
    }
    if (deps.readOnly(open.channel)) {
      setTextIfChanged(deps.byId("quick-reply-state"), "This channel is read-only.");
      return;
    }
    const route = deps.replyRoute(open.message, true);
    if (route.removed) {
      setTextIfChanged(deps.byId("quick-reply-state"), "Not sent: the message you are answering was removed from the channel.");
      return;
    }
    const text = `${quoteBlock(open.quote)}${written.trim()}`;
    // The list stays held from here until the reader moves it: the reply, and what answers it, land
    // without moving the words they read (see the note at the head of this file).
    if (!quickReplyHold) holdQuickReplyList(listOf(open.row));
    const completion = deps.queueOutgoingMessage({
      channel: open.channel, threadId: route.branch || route.into || route.thread, replyTo: String(open.message.id), text,
      marksRead: deps.replyMarksRead(open.message, text),
      ...(route.unknown ? { placement: "unknown" as const } : {}),
    });
    deps.byId("quick-reply-text").value = "";
    closeQuickReply();
    await completion;
  }

  function noteQuickReplyInput(): void {
    if (quickReply) keepQuickDraft(quickReply.key, quickReply.channel, deps.byId("quick-reply-text").value);
  }

  function openSentenceReply(row: QuotedRow, id: string, quote: string, box: Box | null): void {
    const message = rowMessages(row).find((one) => String(one.id) === id) || rowMessages(row)[0];
    if (!message || !quote.trim()) return;
    const rect = row.getBoundingClientRect();
    const top = Number(rect.top) || 0;
    openQuickReplyFor(row, message, quote.trim(), box || { top, bottom: top + QUICK_REPLY_GAP_PX * 3, left: rect.left, right: rect.right });
  }

  function openQuickDraft(key: string, row: QuotedRow): void {
    const { id, quote } = keyParts(key);
    const message = rowMessages(row).find((one) => String(one.id) === id) || rowMessages(row)[0];
    if (!message || !quote) return;
    const rect = row.getBoundingClientRect();
    // Under the row's first line, its header: the words it quoted were somewhere in the message below.
    const top = Number(rect.top) || 0;
    openQuickReplyFor(row, message, quote, { top, bottom: top + QUICK_REPLY_GAP_PX * 3, left: rect.left, right: rect.right });
  }

  return {
    held: () => quoteHeld, holdQuote, noteQuoteSelection, takeQuote, placeSelectionChip, pressSelectionChip,
    quickReplyHolds, quickReplyListMoved, quickReplyOpen: () => quickReply !== null, placeQuickReply,
    closeQuickReply, putDownQuickReply, sendQuickReply, noteQuickReplyInput, openQuickDraft, openSentenceReply,
    quickDraftLive: (key) => Boolean((quickReplyDrafts.get(key) || "").trim()),
    quickDraftMessage: (key) => keyParts(key).id,
  };
}

// What the page and the tests use of it.
export {
  QUICK_REPLY_GAP_PX, QUICK_REPLY_MAX_WIDTH_PX, QUOTE_HOLD_MS, QUOTE_REPLY_TITLE, SELECTION_BAR_ROOM_PX, SELECTION_CHIP_EDGE_PX,
  SELECTION_CHIP_GAP_PX, SELECTION_CHIP_HANDLES_PX,
};

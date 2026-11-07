// Copying the highlighted message (`#296 copy-highlighted-message`, `#538 c-key-copy`; `#624
// voice-page-modularity`, moved from the legacy script into view/shell for the plan's step 17): Ctrl+C (Cmd+C)
// with nothing selected, and c or y, put the message the reader highlighted on the clipboard, in a plain and a
// rich flavour, and say so. Which row is lit, what its messages say and how they render, the status pill and
// the plain-text clipboard fallback are the page's.
//
// The owner, 2026-10-14: a keystroke that copies the highlighted message, with no text selected first.
// The design agreed in chat: Cmd+C (Ctrl+C) keeps the browser's own copy whenever the browser has
// something to copy, meaning a non-empty text selection on the page or the focus in a text field (whose
// selection the page cannot see and must not overrule). Otherwise, with a message highlighted, it
// copies that whole message: everything it says as written, also when its row shows a summary or is
// folded. A bare y does the same outside the text fields. Cmd+Shift+C was set aside because Chrome keeps
// it for the DevTools element picker.
//
// THE BROWSER'S OWN COPY DOES THE WORK. Chromium fires `copy` for Ctrl+C with nothing selected, and on a
// Mac for Cmd+C, through its Edit menu. A `copy` handler that sets `clipboardData` and refuses the
// default writes the clipboard at once, in a plain and a rich flavour, with no permission to ask for
// (`onPageCopy`). Safari, like older Chromium, enables its Copy command with nothing selected only where
// `beforecopy` is refused, so `onPageBeforeCopy` refuses it whenever there is a message to copy. The key
// itself only notes the message (`copyChordFromKeyboard`) and leaves its default alone, since refusing
// that would stop the very `copy` event that does the work. A browser that still fires no `copy`, and a
// chord no menu answers (Cmd+C anywhere but on a Mac), is caught a task later, once the default has run:
// the message goes by `execCommand("copy")`, which fires the same event, else by the asynchronous
// Clipboard API, else through a hidden textarea (`copyNow`). y, which no browser copies with, goes that
// way at once.
//
// WHAT IS COPIED is `content`, the raw text as written, of every message the row stands for, in order,
// as the ⋯ menu's Copy text copies it (`#198 copy-message-text`). It is never the summary of a summarised
// row nor the clamped preview of a folded one: those are how the row is drawn, and the row holds its
// messages whole. A thread card copies its thread's first message. The rich flavour is each message's
// body as the page renders it: its sanitized `content_html`, or else its text as plain paragraphs. The
// page's messages carry no attachments of their own (a message that is only an attachment arrives with
// no content), and every link a message has is in its text, so the copy holds every address it shows.
//
// WHAT IS HIGHLIGHTED is the one lit row (`data-selected`) of the pane in use (`drawSelection`): the
// selection the keys or the mouse made in Main, All, a thread, the right column's thread or the thread
// list's cards. A silent selection, made by a tap on a touch screen, draws nothing and is not one: the
// reader could not see what would go. Neither is the line of a collapsed run of read messages, which
// stands for many. With no message highlighted, each of these keys is left to the browser.
//
// A COPY IS NOT HELD TO THE RULE THAT A KEY NEVER ACTS ON A MESSAGE SCROLLED OUT OF SIGHT (`rowForAct`).
// That rule keeps a key from changing a message the reader cannot see. A copy changes nothing, and the
// message it copies is still the one the reader chose. It says what it did in the status pill, which
// floats over the list and moves nothing under the reader (`#280 scroll-stability`), and a screen
// reader reads that out politely.

import { createPendingCopy } from "../dom/index.ts";

/** What the status pill says once a message is on the clipboard. */
export const COPIED_MESSAGE = "Copied message.";

/** What the status pill says for c or y with nothing to copy (`#538 c-key-copy`). */
export const NOTHING_TO_COPY = "Nothing to copy: select a message, or some text, first.";

/** What it says once text the reader selected is on the clipboard by c or y. */
export const COPIED_SELECTION = "Copied the selected text.";

/** A copy of the highlighted message. */
export interface MessageCopy {
  /** The plain flavour: the raw text as written. */
  readonly text: string;
  /** The rich flavour: the body as the page renders it. */
  readonly html: string;
  /** Whether a `copy` event has put it on the clipboard (`onPageCopy`). */
  served: boolean;
}

/**
 * `text` as the text of an HTML document: the characters markup gives meaning to, escaped. The quotes
 * are named by their code points: the page suite reads its sources with their strings taken out, and a
 * quote mark standing alone in a pattern would start a string there that runs on through the code below.
 */
export const htmlText = (text: unknown): string => String(text).replace(/[&<>\u0022\u0027]/g, (char) => `&#${char.charCodeAt(0)};`);

/**
 * `text` as plain paragraphs of HTML, as `plainParagraphs` draws it: each run of lines that are not blank
 * a paragraph, its lines separated by line breaks.
 */
export function plainParagraphsHtml(text: unknown): string {
  const paragraphs: string[][] = [];
  let lines: string[] = [];
  for (const line of String(text).split("\n")) {
    if (/^[ \t]*$/.test(line)) {
      if (lines.length) paragraphs.push(lines);
      lines = [];
    } else {
      lines.push(htmlText(line));
    }
  }
  if (lines.length) paragraphs.push(lines);
  return paragraphs.map((one) => `<p>${one.join("<br>")}</p>`).join("");
}

/** A list the keys move through, as far as the copy reads it. */
export interface CopiedPane {
  /** The row lit in it, or null. */
  readonly lit: HTMLElement | null;
  /** Its list element. */
  list(): unknown;
}

/** A `copy` event, as far as the copy answers it. */
export interface CopyEvent {
  readonly clipboardData?: { setData(type: string, data: string): void } | null;
  preventDefault(): void;
}

export interface HighlightCopyDependencies<Message> {
  /** Whether `node` is a field that types. */
  isTextField(node: unknown): boolean;
  /** The text the reader has selected on the page, or "". */
  selectedText(): string;
  /** The pane in use, or null. */
  activePane(): CopiedPane | null;
  /** Whether `row` is shown. */
  rowShown(row: HTMLElement): boolean;
  /** The messages `row` stands for, oldest first. */
  rowMessages(row: HTMLElement): Message[];
  /** The text of `messages` together, as written. */
  combinedContent(messages: readonly Message[]): string;
  /** `message`'s body as the page renders it, or "". */
  renderedHtmlOf(message: Message): string;
  /** `message`'s raw text. */
  rawTextOf(message: Message): string;
  /** Say `text` on the status pill. */
  setStatus(text: string): void;
  /** What the status pill says when no way of copying worked. */
  readonly copyRefused: string;
  /** Put `text` on the clipboard as plain text; whether it went. */
  writeClipboardText(text: string): Promise<boolean>;
  /** `run`, wrapped so a failure is reported without ending a call. */
  guardQuietly(run: () => unknown): () => unknown;
  setTimeout(run: () => void, ms: number): unknown;
}

/** The copy's listeners and keys. */
export interface HighlightCopy {
  /**
   * The `copy` event: with nothing selected and no text field focused, the highlighted message, both
   * flavours written to `clipboardData` and the default refused. Anything else is the browser's.
   */
  onPageCopy(event: CopyEvent | null | undefined): void;
  /**
   * `beforecopy`, refused whenever `onPageCopy` would copy: the signal Safari (and Chromium before it)
   * takes to enable Copy with nothing selected. Chromium fires `copy` either way, and once.
   */
  onPageBeforeCopy(event: { preventDefault(): void } | null | undefined): void;
  /**
   * Ctrl+C (Cmd+C), with nothing selected and no text field focused, on a highlighted message: noted for
   * the `copy` event that the browser's own default fires next, and a task later, once that default has
   * run, copied by `copyNow` if no `copy` came. FALSE EITHER WAY, on purpose: the key's default is the
   * browser's and must stay so, since it is what fires the `copy` event.
   */
  copyChordFromKeyboard(): boolean;
  /**
   * c and y: the highlighted message copied, as Ctrl+C (Cmd+C) copies it with nothing selected; with text
   * selected on the page, that text, as the chord would copy it (`#538 c-key-copy`); with neither, the
   * status pill says so and nothing is copied. Never in a text field, where both are typed.
   */
  copyFromKeyboard(): boolean;
}

export function createHighlightCopy<Message>(deps: HighlightCopyDependencies<Message>): HighlightCopy {
  /** The copy a key in the page has noted, while the `copy` event it expects may still come. */
  const pendingCopy = createPendingCopy<MessageCopy>();

  /** Whether a copy is the browser's own: a text field holding the focus, or text selected on the page. */
  const browserCopies = (): boolean => deps.isTextField(document.activeElement) || deps.selectedText() !== "";

  /**
   * The messages of the highlighted row, oldest first: the row lit in the pane in use, or for a thread
   * card its thread's first message. [] with nothing lit, on the line of a collapsed run of read messages,
   * and on a card whose first message the page does not hold.
   */
  function highlightedMessages(): Message[] {
    const pane = deps.activePane();
    const row = pane ? pane.lit : null;
    if (!pane || !row || row.parentNode !== pane.list() || !deps.rowShown(row)) return [];
    if (row.getAttribute("data-read-run") === "head") return [];
    if (row.hasAttribute("data-card")) {
      const root = (row as { cardRoot?: Message | null }).cardRoot;
      return root ? [root] : [];
    }
    return deps.rowMessages(row);
  }

  /** The rich flavour of a copy of `messages`: one block for each, its body as the page renders it. */
  const copyHtml = (messages: readonly Message[]): string =>
    messages.map((message) => `<div>${deps.renderedHtmlOf(message) || plainParagraphsHtml(deps.rawTextOf(message))}</div>`).join("");

  /**
   * The highlighted message as a copy, or null: where the copy is the browser's own (`browserCopies`),
   * where nothing is highlighted, and where what is highlighted says nothing, which a copy would only
   * empty the clipboard with.
   */
  function messageCopy(): MessageCopy | null {
    if (browserCopies()) return null;
    const messages = highlightedMessages();
    const text = deps.combinedContent(messages);
    return text.trim() === "" ? null : { text, html: copyHtml(messages), served: false };
  }

  /**
   * The text selected on the page, `selection`, on the clipboard from a key the browser does not copy with:
   * by `execCommand("copy")`, the browser's own copy of the selection (no `copy` handler of the page takes
   * it, the selection being the browser's), else as plain text by `writeClipboardText`.
   */
  async function copySelectionNow(selection: string): Promise<boolean> {
    let copied = false;
    try {
      copied = typeof document.execCommand === "function" && document.execCommand("copy") === true;
    } catch (_error) {
      copied = false;
    }
    if (!copied) copied = await deps.writeClipboardText(selection);
    deps.setStatus(copied ? COPIED_SELECTION : deps.copyRefused);
    return copied;
  }

  /**
   * Put `copy` on the clipboard from a key the browser does not copy with: by `execCommand("copy")`, whose
   * `copy` event `onPageCopy` answers in both flavours; where no event came, as plain text by
   * `writeClipboardText`. The status pill says how it went.
   */
  async function copyNow(copy: MessageCopy): Promise<boolean> {
    pendingCopy.hold(copy);
    try {
      if (typeof document.execCommand === "function") document.execCommand("copy");
    } catch (_error) {
      // Refused: the ways below are tried.
    } finally {
      pendingCopy.release(copy);
    }
    if (copy.served) return true;
    const copied = await deps.writeClipboardText(copy.text);
    deps.setStatus(copied ? COPIED_MESSAGE : deps.copyRefused);
    return copied;
  }

  return {
    onPageCopy(event) {
      if (!event || !event.clipboardData || browserCopies()) return;
      const copy = pendingCopy.held() || messageCopy();
      if (!copy) return;
      event.clipboardData.setData("text/plain", copy.text);
      event.clipboardData.setData("text/html", copy.html);
      event.preventDefault();
      copy.served = true;
      deps.setStatus(COPIED_MESSAGE);
    },
    onPageBeforeCopy(event) {
      if (event && !browserCopies() && (pendingCopy.held() || messageCopy())) event.preventDefault();
    },
    copyChordFromKeyboard() {
      const copy = messageCopy();
      if (!copy) return false;
      pendingCopy.hold(copy);
      deps.setTimeout(() => {
        pendingCopy.release(copy);
        if (!copy.served) deps.guardQuietly(() => copyNow(copy))();
      }, 0);
      return false;
    },
    copyFromKeyboard() {
      const selection = deps.selectedText();
      if (selection !== "") {
        deps.guardQuietly(() => copySelectionNow(selection))();
        return true;
      }
      const copy = messageCopy();
      if (!copy) {
        deps.setStatus(NOTHING_TO_COPY);
        return true;
      }
      deps.guardQuietly(() => copyNow(copy))();
      return true;
    },
  };
}

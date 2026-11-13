// A quoted stretch as the head of a reply (`#624 voice-page-modularity`, moved from the legacy script
// into present/rows; `#267 quote-reply`).

/** The longest quote, in characters, the "…" that ends a longer one included. Help says so. */
const QUOTE_MAX_CHARS = 1000;

/** `text` as the head of a reply: every line led by "> ", then one empty line. "" for nothing. */
function quoteBlock(text: unknown): string {
  let said = String(text).replace(/\r\n?/g, "\n").split("\n")
    .map((line) => line.replace(/[ \t\f\v\u00a0]+/g, " ").trim())
    .join("\n").replace(/\n{3,}/g, "\n\n").trim();
  if (said === "") return "";
  if (said.length > QUOTE_MAX_CHARS) {
    let cut = said.slice(0, QUOTE_MAX_CHARS - 1);
    // At the end of a word, where one ends in the second half, rather than in the middle of one.
    // `cut` is shorter than `said` here, so this reads a character of it.
    const end = /\s/.test(said[cut.length] as string) ? cut.length : cut.search(/\s\S*$/);
    if (end > QUOTE_MAX_CHARS / 2) cut = cut.slice(0, end);
    // Never half of a character written as two code units.
    if (/[\ud800-\udbff]$/.test(cut)) cut = cut.slice(0, -1);
    said = `${cut.trimEnd()}…`;
  }
  return `${said.split("\n").map((line) => (line === "" ? ">" : `> ${line}`)).join("\n")}\n\n`;
}

/** `text`'s words as one line: each run of white space made one space, the ends trimmed. */
const wordsLine = (text: string): string => text.replace(/\s+/g, " ").trim();

/**
 * The words of the quote a reply's text opens with (`quoteBlock`'s lines, led by ">"), unquoted and
 * joined; "" where it opens with none. A cut quote's closing "…" is not the message's.
 */
function quotedHead(text: unknown): string {
  const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
  const quoted: string[] = [];
  for (const line of lines) {
    if (!/^\s*>/.test(line)) break;
    quoted.push(line.replace(/^\s*> ?/, ""));
  }
  return wordsLine(quoted.join(" ")).replace(/…$/, "").trimEnd();
}

/**
 * `#728 vt-quote-read-threshold`. How much of `whole`, a message's text, the quote `replyText` opens
 * with covers, from 0 to 1, counted in characters of their words; null where the reply quotes nothing
 * or the message has no words to cover.
 */
function quoteShare(replyText: unknown, whole: unknown): number | null {
  const quoted = quotedHead(replyText);
  const all = wordsLine(String(whole ?? ""));
  if (quoted === "" || all === "") return null;
  return Math.min(1, quoted.length / all.length);
}

/**
 * Whether a reply marks the message it answers read (`#300 reply-marks-read`): always, unless it
 * quotes less of it than `percent` of its words (`quoteShare`). A reply to one sentence of a long
 * message leaves the rest of it to be read.
 */
const quoteMarksRead = (share: number | null, percent: number): boolean => share === null || share * 100 >= percent;

/** Where a reply's quote sits in the message it answers, counted in words. */
export interface QuotePosition {
  /** Words from the message's start to the quote's end: the quote covers words up to here. */
  readonly endWord: number;
  /** The message's words in all. */
  readonly totalWords: number;
  /** The quote ends at or before the final paragraph begins. */
  readonly beforeFinalParagraph: boolean;
}

/** A text's words: each run of white space is a break, as `wordsLine` joins them. */
const wordList = (text: string): string[] => wordsLine(text).split(" ").filter((word) => word !== "");

/**
 * A message as the blocks it is drawn in, each as its words: a paragraph, every bullet or
 * numbered item, a heading, a code block and a quoted block each count as one. A one-block
 * message is all final block, so a quote in it is never before the final paragraph.
 */
function renderedBlocks(text: string): string[][] {
  const blocks: string[][] = [];
  let current: string[] = [];
  let kind = "";
  let inCode = false;
  const flush = (): void => { if (current.length > 0) blocks.push(current); current = []; kind = ""; };
  const startBlock = (next: string): void => { flush(); kind = next; };
  for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
    const trimmed = line.trim();
    if (trimmed.startsWith("```")) {
      if (!inCode) { startBlock("code"); inCode = true; current.push(...wordList(line)); }
      else { current.push(...wordList(line)); inCode = false; flush(); }
      continue;
    }
    if (inCode) { current.push(...wordList(line)); continue; }
    if (trimmed === "") { flush(); continue; }
    if (/^#{1,6}\s/.test(trimmed)) startBlock("heading");
    else if (/^\s*>/.test(line)) { if (kind !== "quote") startBlock("quote"); }
    else if (/^\s*(?:[-*+]\s|\d+[.)]\s)/.test(line)) startBlock("item");
    else if (kind === "heading" || kind === "quote") startBlock("para");
    current.push(...wordList(line));
  }
  flush();
  return blocks.filter((words) => words.length > 0);
}

/**
 * Where the quote `replyText` opens with sits in `whole`: the first place its words appear in
 * the message's, in order. Null where the reply quotes nothing, the message has no words, or
 * the quoted words are not found in it (a quote cut or reworded past finding fails open: the
 * position guards below then hold nothing back).
 */
function quotePosition(replyText: unknown, whole: unknown): QuotePosition | null {
  const quoted = wordList(quotedHead(replyText));
  const all = wordList(String(whole ?? ""));
  if (quoted.length === 0 || all.length === 0) return null;
  let at = -1;
  for (let start = 0; start + quoted.length <= all.length; start += 1) {
    if (quoted.every((word, offset) => all[start + offset] === word)) { at = start; break; }
  }
  if (at < 0) return null;
  const blocks = renderedBlocks(String(whole ?? ""));
  const lastWords = blocks.length > 0 ? (blocks[blocks.length - 1] as string[]).length : 0;
  return { endWord: at + quoted.length, totalWords: all.length, beforeFinalParagraph: at + quoted.length <= all.length - lastWords };
}

/** The reader's Mark read on reply choices, as a reply reads them at send time. */
export interface ReplyMarkChoices {
  /** Mark read on reply itself: No holds every reply back. */
  readonly enabled: boolean;
  /** Hold a reply back when its quote covers under the share. */
  readonly quoteGuard: boolean;
  /** Hold a reply back sent while the list is in Fold read. */
  readonly foldGuard: boolean;
  /** Whether the list is in Fold read as the reply is sent. */
  readonly inFold: boolean;
  /** Whether a reply that quotes nothing marks what it answers. */
  readonly markNonQuote: boolean;
  /** Hold a quoted reply back when its quote ends before the final paragraph. */
  readonly finalGuard: boolean;
  /** Hold a quoted reply back when its quote sits in the first share of a long message. */
  readonly firstGuard: boolean;
  /** The first share, in percent, and how many words make a message long. */
  readonly firstPercent: number;
  readonly longWords: number;
}

/**
 * Whether a reply marks the message it answers read, by the reader's choices: No never does;
 * in Fold read it does not while that guard is on, because Fold read folds what was just
 * answered out of sight. A reply that quotes nothing marks only while non-quote replies do.
 * A quoted reply is held back by any quoted-reply guard it trips: too small a quote, a quote
 * ending before the final paragraph, or a quote sitting in the first share of a long message.
 * With every guard off, Yes marks as `#300 reply-marks-read` always did.
 */
function replyShouldMarkRead(choices: ReplyMarkChoices, share: number | null, percent: number, position: QuotePosition | null): boolean {
  if (!choices.enabled) return false;
  if (choices.foldGuard && choices.inFold) return false;
  if (share === null) return choices.markNonQuote;
  if (choices.quoteGuard && !quoteMarksRead(share, percent)) return false;
  if (position !== null) {
    if (choices.finalGuard && position.beforeFinalParagraph) return false;
    if (choices.firstGuard && position.totalWords > choices.longWords
      && position.endWord * 100 <= position.totalWords * choices.firstPercent) return false;
  }
  return true;
}

export { QUOTE_MAX_CHARS, quoteBlock, quoteMarksRead, quotePosition, quoteShare, quotedHead, replyShouldMarkRead };

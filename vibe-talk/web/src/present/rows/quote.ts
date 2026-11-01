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

export { QUOTE_MAX_CHARS, quoteBlock, quoteMarksRead, quoteShare, quotedHead };

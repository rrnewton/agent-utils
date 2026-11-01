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

export { QUOTE_MAX_CHARS, quoteBlock };

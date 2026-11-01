// The links one message's text holds as written (`#624 voice-page-modularity`, moved from the legacy
// script into foundation, beside `linkKind`; `#211 link-filter`): Markdown links, the angle-bracket
// forms chat services send, and bare addresses, each bounded so a hostile message cannot make the
// scan slow.

/** A link found where it starts: its address, the name it is given, and where it ends. */
interface FoundLink {
  href: string;
  name: string;
  end: number;
}

/**
 * An address a link may be written to: http or https, and a host with a letter or digit in it. The
 * test is on the text itself, which is exactly what the href is set to — never a parsed and
 * re-serialised copy, which could point somewhere the words on the screen do not say.
 */
const HTTP_ADDRESS = /^https?:\/\/[^\s/?#\\]*[\p{L}\p{N}]/iu;

/**
 * The same test where an address would START, asked before it is read any further. It changes no
 * answer — whatever passes `HTTP_ADDRESS` passed this at its start — but a bare address and a
 * Markdown target are both read to their ends before that test, and one that was never going to
 * pass was read that far for nothing: a message of `https:///` repeated was read to its end once
 * per repeat (1.9s for 40,000 characters, its brackets counted each time), and one of `[a](`
 * repeated read LINK_TARGET_MAX characters for every bracket (0.6s). Asked first, each costs only
 * its own host.
 */
const HTTP_ADDRESS_AT = /https?:\/\/[^\s/?#\\]*[\p{L}\p{N}]/iuy;

/**
 * A bare address, from its scheme to the first character no address contains as written: a space,
 * an angle bracket, a double quote, a backtick or a bar (which ends one inside `<…|name>`).
 */
const BARE_ADDRESS = /https?:\/\/[^\s<>"`|]+/iuy;

/** `<https://…>` and `<https://…|name>`, the forms chat services send a link in. */
const ANGLE_LINK = /<(https?:\/\/[^\s<>|]+)(?:\|([^<>\n]*))?>/iuy;

/** What may not come straight before a bare address: `xhttps://…` is not an address anyone wrote. */
const WORD_BEFORE = /[\p{L}\p{N}]/u;

/**
 * Characters that end a sentence around an address rather than the address itself, as GitHub's
 * own autolinking reads them, plus the quotes and the ellipsis a sentence closes with.
 */
const TRAILING = new Set<string | undefined>([".", ",", ";", ":", "!", "?", "'", "\"", "*", "_", "~", "’", "”", "…"]);

/**
 * The longest Markdown name, and the longest target, read before deciding a bracket opens no link.
 * Bounded because every `[` asks for its closing `]`, and a long line of brackets that never close
 * would otherwise be read to its end once per bracket.
 */
const LINK_NAME_MAX = 500;

const LINK_TARGET_MAX = 4096;

/** Whether `text` holds an address that could pass `HTTP_ADDRESS` starting at `at`. */
function addressStartsAt(text: string, at: number): boolean {
  HTTP_ADDRESS_AT.lastIndex = at;
  return HTTP_ADDRESS_AT.test(text);
}

/**
 * A bare address with what follows it in the sentence taken off: trailing punctuation, and a
 * closing parenthesis or bracket that has no partner inside the address — `(see https://x.org)` —
 * while one that does is kept, as in `https://en.wikipedia.org/wiki/Mercury_(planet)`.
 *
 * THE BRACKETS ARE COUNTED ONCE, and the counts kept as characters come off the end. Counting the
 * whole address again for every character taken off read a run of closers once per closer, and a
 * bare address runs to the end of its message: one address followed by 40,000 `)` — the longest
 * message Slack allows — took 15s in Node, on the page's one thread, and again on every redraw of
 * the row while Links is on. Counted once, it takes a few milliseconds.
 */
function trimAddress(candidate: string): string {
  let [opens, closes, squareOpens, squareCloses] = [0, 0, 0, 0];
  for (let i = 0; i < candidate.length; i += 1) {
    const character = candidate[i];
    if (character === "(") opens += 1;
    else if (character === ")") closes += 1;
    else if (character === "[") squareOpens += 1;
    else if (character === "]") squareCloses += 1;
  }
  let end = candidate.length;
  for (;;) {
    const last = candidate[end - 1];
    if (TRAILING.has(last)) {
      end -= 1;
    } else if (last === ")" && opens < closes) {
      closes -= 1;
      end -= 1;
    } else if (last === "]" && squareOpens < squareCloses) {
      squareCloses -= 1;
      end -= 1;
    } else {
      return candidate.slice(0, end);
    }
  }
}

/**
 * The Markdown link whose `[` is at `start`, complete and well formed, as `{href, name, end}`, or
 * null. The name may hold brackets that pair, as CommonMark's may, and is on one line; the target
 * may hold parentheses that pair and be followed by a quoted title. An empty name is returned as
 * one, and the caller shows the address instead. A target that is not http or https is no link.
 */
function markdownLinkAt(text: string, start: number): FoundLink | null {
  let depth = 0;
  let close = start;
  for (; close < text.length && close - start <= LINK_NAME_MAX; close += 1) {
    const character = text[close];
    if (character === "\n") return null;
    if (character === "\\") {
      close += 1;
    } else if (character === "[") {
      depth += 1;
    } else if (character === "]") {
      depth -= 1;
      if (depth === 0) break;
    }
  }
  if (depth !== 0 || text[close] !== "]" || text[close + 1] !== "(") return null;
  const name = text.slice(start + 1, close).replace(/\s+/g, " ").trim();
  let at = close + 2;
  const skipBlanks = (): void => {
    while (text[at] === " " || text[at] === "\t") at += 1;
  };
  skipBlanks();
  if (!addressStartsAt(text, text[at] === "<" ? at + 1 : at)) return null;
  let href = "";
  if (text[at] === "<") {
    const shut = text.indexOf(">", at);
    if (shut < 0 || shut - at > LINK_TARGET_MAX) return null;
    href = text.slice(at + 1, shut);
    at = shut + 1;
  } else {
    const from = at;
    let parens = 0;
    for (; at < text.length && at - from <= LINK_TARGET_MAX; at += 1) {
      // A space by `\s`, the ASCII ones answered without the pattern: a message of
      // `[a](https://a/(<` repeated reads LINK_TARGET_MAX characters for every bracket, and asking
      // the pattern of every one of them made 40,000 characters take 180ms rather than tens.
      const code = text.charCodeAt(at);
      if (code === 32 || (code >= 9 && code <= 13) || (code > 127 && /\s/.test(text[at] as string))) break;
      const character = text[at];
      if (character === "(") {
        parens += 1;
      } else if (character === ")") {
        if (parens === 0) break;
        parens -= 1;
      }
    }
    href = text.slice(from, at);
  }
  skipBlanks();
  const titleEnds = ({ "\"": "\"", "'": "'", "(": ")" } as Record<string, string>)[text[at] as string];
  if (titleEnds) {
    const shut = text.indexOf(titleEnds, at + 1);
    if (shut < 0 || text.slice(at, shut).includes("\n")) return null;
    at = shut + 1;
    skipBlanks();
  }
  if (text[at] !== ")" || /\s/.test(href) || !HTTP_ADDRESS.test(href)) return null;
  return { href, name, end: at + 1 };
}

/** The `<https://…>` or `<https://…|name>` link at `start`, as `{href, name, end}`, or null. */
function angleLinkAt(text: string, start: number): FoundLink | null {
  ANGLE_LINK.lastIndex = start;
  const match = ANGLE_LINK.exec(text);
  if (!match || !HTTP_ADDRESS.test(match[1] as string)) return null;
  return { href: match[1] as string, name: (match[2] || "").trim(), end: start + match[0].length };
}

/** The bare address starting at `start`, as `{href, name, end}` with no name, or null. */
function bareLinkAt(text: string, start: number): FoundLink | null {
  if (start > 0 && WORD_BEFORE.test(text[start - 1] as string)) return null;
  if (!addressStartsAt(text, start)) return null;
  BARE_ADDRESS.lastIndex = start;
  const match = BARE_ADDRESS.exec(text);
  if (!match) return null;
  const href = trimAddress(match[0]);
  if (!HTTP_ADDRESS.test(href)) return null;
  return { href, name: "", end: start + href.length };
}

/**
 * Every link in one message's text, in the order it is written, as `{href, name}`: the name a
 * Markdown or angle-bracket link gives it, and otherwise the address itself.
 *
 * Scanned rather than matched by one pattern: which form a `[` or a `<` opens is decided where it
 * stands, and only where it opens none is the text inside it read for a bare address.
 */
function linksIn(text: unknown): Array<{ href: string; name: string }> {
  const source = String(text === null || text === undefined ? "" : text);
  const links: Array<{ href: string; name: string }> = [];
  let at = 0;
  while (at < source.length) {
    const character = source[at];
    const found = character === "[" ? markdownLinkAt(source, at)
      : character === "<" ? angleLinkAt(source, at)
        : character === "h" || character === "H" ? bareLinkAt(source, at)
          : null;
    if (found) {
      links.push({ href: found.href, name: found.name || found.href });
      at = found.end;
    } else {
      at += 1;
    }
  }
  return links;
}

export {
  ANGLE_LINK,
  BARE_ADDRESS,
  HTTP_ADDRESS,
  HTTP_ADDRESS_AT,
  LINK_NAME_MAX,
  LINK_TARGET_MAX,
  TRAILING,
  WORD_BEFORE,
  addressStartsAt,
  angleLinkAt,
  bareLinkAt,
  linksIn,
  markdownLinkAt,
  trimAddress,
};
export type { FoundLink };

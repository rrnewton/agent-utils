// The Documents reader's Markdown renderer: untrusted document text in, safe HTML out.
//
// The source is fetched document content, so it is untrusted exactly as agent and
// channel text are (see the voice page's header rules): nothing in it may reach the
// page as markup the source itself wrote. The whole source is HTML-escaped first,
// and only then is Markdown formatting applied to the escaped text, so the only
// tags in the output are the ones this module emits, and the only attributes are
// the ones it writes itself. Links and images are held to an allowlist of URL
// forms; anything else renders as its text alone.

/** Escape the characters that could start markup or break out of an attribute. */
function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/**
 * The `id` a heading with this text is rendered under, `doc-` prefixed: the plain
 * text lowercased, with each run of non-alphanumerics as one hyphen. The reader
 * derives the same id from the outline the server sends, so an outline tap lands
 * on exactly the heading this renderer anchored. Escaped and raw spellings of the
 * same heading agree, because the escaping entities are folded back first.
 */
export function documentHeadingId(text: string): string {
  const plain = text
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/[*_`~]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `doc-${plain === "" ? "section" : plain}`;
}

/** Characters that may not appear in a URL this renderer emits as an `href`. */
const UNSAFE_URL_CHARS = /[\s"<>\u0000-\u001f\u007f]/;

/**
 * The `href` for a link to `url`, or null where the URL is not one the page will
 * follow: only `http://`, `https://`, and the in-app `#/` and `docs:` forms are
 * links at all. Every other scheme (`javascript:`, `data:`, `vbscript:`, and the
 * rest), protocol-relative and relative addresses alike, renders as plain text.
 * The URL comes from already-escaped text, so it holds no quote that could close
 * the attribute; the character check fails closed regardless.
 */
function allowedLinkHref(url: string): string | null {
  const trimmed = url.trim();
  if (trimmed === "" || UNSAFE_URL_CHARS.test(trimmed)) return null;
  const lower = trimmed.toLowerCase();
  if (lower.startsWith("http://") || lower.startsWith("https://")) return trimmed;
  if (trimmed.startsWith("#/") || lower.startsWith("docs:")) return trimmed;
  return null;
}

/** The `href` for an image's alt-text link: `http(s)` only, never an in-app form. */
function allowedImageHref(url: string): string | null {
  const trimmed = url.trim();
  if (trimmed === "" || UNSAFE_URL_CHARS.test(trimmed)) return null;
  const lower = trimmed.toLowerCase();
  if (lower.startsWith("http://") || lower.startsWith("https://")) return trimmed;
  return null;
}

/** One `[label](url)` span: its label, its address, and where the span ends. */
interface BracketSpan {
  label: string;
  url: string;
  end: number;
}

/**
 * The bracket span opening at `open` (the index of `[`), or null where the text
 * there is not one. The address scan balances parentheses, so an address holding
 * a pair of them is taken whole; an optional title after the address is dropped.
 */
function parseBracket(text: string, open: number): BracketSpan | null {
  const close = text.indexOf("]", open + 1);
  if (close === -1 || text.charAt(close + 1) !== "(") return null;
  let depth = 1;
  let at = close + 2;
  while (at < text.length && depth > 0) {
    const ch = text.charAt(at);
    if (ch === "(") depth += 1;
    else if (ch === ")") depth -= 1;
    at += 1;
  }
  if (depth !== 0) return null;
  const label = text.slice(open + 1, close);
  let url = (text.slice(close + 2, at - 1).trim().split(/\s+/)[0] ?? "").trim();
  if (url.startsWith("<") && url.endsWith(">")) url = url.slice(1, -1);
  return { label, url, end: at };
}

/** The index of the `*` closing an italic span opened before `from`, or -1. */
function italicClose(text: string, from: number): number {
  for (let at = from; at < text.length; at += 1) {
    if (text.charAt(at) === "*" && text.charAt(at + 1) !== "*" && text.charAt(at - 1) !== "*") return at;
  }
  return -1;
}

/** How deep inline formatting may nest before the rest is left as plain text. */
const MAX_INLINE_DEPTH = 8;

/**
 * Inline formatting over already-escaped text: code spans verbatim, bold and
 * italic, and links and images held to the URL allowlists above. Anything this
 * does not recognise is copied through as the (escaped) text it is.
 */
function renderInline(text: string, depth: number): string {
  if (depth > MAX_INLINE_DEPTH) return text;
  let out = "";
  let at = 0;
  while (at < text.length) {
    const ch = text.charAt(at);
    if (ch === "`") {
      const end = text.indexOf("`", at + 1);
      if (end === -1) {
        out += ch;
        at += 1;
        continue;
      }
      out += `<code>${text.slice(at + 1, end)}</code>`;
      at = end + 1;
      continue;
    }
    if (ch === "!" && text.charAt(at + 1) === "[") {
      const span = parseBracket(text, at + 1);
      if (span !== null) {
        const href = allowedImageHref(span.url);
        const label = renderInline(span.label, depth + 1);
        out += href === null ? label : `<a href="${href}" rel="noopener noreferrer">${label}</a>`;
        at = span.end;
        continue;
      }
      out += ch;
      at += 1;
      continue;
    }
    if (ch === "[") {
      const span = parseBracket(text, at);
      if (span !== null) {
        const href = allowedLinkHref(span.url);
        const label = renderInline(span.label, depth + 1);
        out += href === null ? label : `<a href="${href}" rel="noopener noreferrer">${label}</a>`;
        at = span.end;
        continue;
      }
      out += ch;
      at += 1;
      continue;
    }
    if (ch === "*" && text.charAt(at + 1) === "*") {
      const end = text.indexOf("**", at + 2);
      if (end > at + 2) {
        out += `<strong>${renderInline(text.slice(at + 2, end), depth + 1)}</strong>`;
        at = end + 2;
        continue;
      }
      out += ch;
      at += 1;
      continue;
    }
    if (ch === "*") {
      const end = italicClose(text, at + 1);
      if (end > at + 1) {
        out += `<em>${renderInline(text.slice(at + 1, end), depth + 1)}</em>`;
        at = end + 1;
        continue;
      }
      out += ch;
      at += 1;
      continue;
    }
    out += ch;
    at += 1;
  }
  return out;
}

/** The cells of one table row, the framing pipes dropped and each cell trimmed. */
function tableCells(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|")) text = text.slice(0, -1);
  return text.split("|").map((cell) => cell.trim());
}

/** Whether `line` is a table's header separator: nothing but dashes, colons, pipes. */
function isSeparatorRow(line: string): boolean {
  const cells = tableCells(line);
  return cells.length > 0 && cells.every((cell) => /^:?-{1,}:?$/.test(cell));
}

/** Whether a table starts at `at`: a piped line over a separator row. */
function isTableStart(lines: string[], at: number): boolean {
  const line = lines[at] ?? "";
  const next = lines[at + 1];
  return line.includes("|") && next !== undefined && isSeparatorRow(next);
}

/** Whether the line at `at` opens a block of its own, ending any paragraph. */
function startsBlock(lines: string[], at: number): boolean {
  const line = (lines[at] ?? "").trim();
  if (line.startsWith("```")) return true;
  if (/^#{1,6}\s+/.test(line)) return true;
  if (/^(-{3,}|\*{3,}|_{3,})$/.test(line)) return true;
  if (line.startsWith("&gt;")) return true;
  if (/^[-*+]\s+/.test(line) || /^\d{1,9}[.)]\s+/.test(line)) return true;
  return isTableStart(lines, at);
}

/**
 * Render a document's raw Markdown as a safe HTML string: the source escaped
 * whole, then formatted. Headings, paragraphs, bold and italic, inline and
 * fenced code, lists, quotes, rules, tables, and allowlisted links; no raw HTML,
 * no script-bearing element, and no event handler can appear in the result,
 * because the source's own angle brackets never survive the first step.
 */
export function renderDocumentMarkdown(markdown: string): string {
  const escaped = escapeHtml(markdown.replace(/\u0000/g, "").replace(/\r\n?/g, "\n"));
  const lines = escaped.split("\n");
  const blocks: string[] = [];
  const headingCounts = new Map<string, number>();
  let at = 0;
  while (at < lines.length) {
    const line = (lines[at] ?? "").trim();
    if (line === "") {
      at += 1;
      continue;
    }
    if (line.startsWith("```")) {
      const code: string[] = [];
      at += 1;
      while (at < lines.length && !(lines[at] ?? "").trim().startsWith("```")) {
        code.push(lines[at] ?? "");
        at += 1;
      }
      if (at < lines.length) at += 1;
      blocks.push(`<pre><code>${code.join("\n")}</code></pre>`);
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading !== null) {
      const marks = heading[1] ?? "";
      const text = (heading[2] ?? "").replace(/\s+#+\s*$/, "");
      const base = documentHeadingId(text);
      const seen = headingCounts.get(base) ?? 0;
      headingCounts.set(base, seen + 1);
      const id = seen === 0 ? base : `${base}-${seen + 1}`;
      blocks.push(`<h${marks.length} id="${id}">${renderInline(text, 0)}</h${marks.length}>`);
      at += 1;
      continue;
    }
    if (/^(-{3,}|\*{3,}|_{3,})$/.test(line)) {
      blocks.push("<hr>");
      at += 1;
      continue;
    }
    if (isTableStart(lines, at)) {
      const header = tableCells(line);
      at += 2;
      const rows: string[][] = [];
      while (at < lines.length) {
        const row = (lines[at] ?? "").trim();
        if (row === "" || !row.includes("|")) break;
        rows.push(tableCells(row));
        at += 1;
      }
      const head = header.map((cell) => `<th>${renderInline(cell, 0)}</th>`).join("");
      const body = rows
        .map((row) => `<tr>${row.map((cell) => `<td>${renderInline(cell, 0)}</td>`).join("")}</tr>`)
        .join("");
      blocks.push(`<table><thead><tr>${head}</tr></thead>${body === "" ? "" : `<tbody>${body}</tbody>`}</table>`);
      continue;
    }
    if (line.startsWith("&gt;")) {
      const quoted: string[] = [];
      while (at < lines.length) {
        const quoteLine = (lines[at] ?? "").trim();
        if (!quoteLine.startsWith("&gt;")) break;
        quoted.push(quoteLine.replace(/^&gt;\s?/, ""));
        at += 1;
      }
      blocks.push(`<blockquote><p>${renderInline(quoted.join(" "), 0)}</p></blockquote>`);
      continue;
    }
    const unordered = /^[-*+]\s+(.*)$/.exec(line);
    const ordered = /^\d{1,9}[.)]\s+(.*)$/.exec(line);
    if (unordered !== null || ordered !== null) {
      const isOrdered = unordered === null;
      const items: string[] = [];
      while (at < lines.length) {
        const itemLine = (lines[at] ?? "").trim();
        const itemUnordered = /^[-*+]\s+(.*)$/.exec(itemLine);
        const itemOrdered = /^\d{1,9}[.)]\s+(.*)$/.exec(itemLine);
        if (isOrdered ? itemOrdered === null : itemUnordered === null) break;
        items.push((isOrdered ? itemOrdered?.[1] : itemUnordered?.[1]) ?? "");
        at += 1;
      }
      const tag = isOrdered ? "ol" : "ul";
      blocks.push(`<${tag}>${items.map((item) => `<li>${renderInline(item, 0)}</li>`).join("")}</${tag}>`);
      continue;
    }
    const paragraph: string[] = [];
    while (at < lines.length) {
      const text = (lines[at] ?? "").trim();
      if (text === "") break;
      if (paragraph.length > 0 && startsBlock(lines, at)) break;
      paragraph.push(text);
      at += 1;
    }
    blocks.push(`<p>${renderInline(paragraph.join("\n"), 0)}</p>`);
  }
  return blocks.join("\n");
}

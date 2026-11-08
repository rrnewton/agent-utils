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
 * Where a document link goes (`#1081 documents-relative-links`): a GitHub document
 * the reader can open in-app (its canonical server key, its external GitHub
 * address, and the `#fragment` to scroll to after opening, when given), an
 * ordinary external address, or nowhere (the label renders as plain text).
 */
export type ResolvedDocumentHref =
  | { kind: "document"; key: string; url: string; anchor: string | null }
  | { kind: "external"; url: string }
  | { kind: "text" };

/** The longest address the server recognises (`documents.rs MAX_ADDRESS`). */
const MAX_ADDRESS = 2048;

/** `text` with the escaping entities folded back, as `documentHeadingId` folds them. */
function foldEntities(text: string): string {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");
}

/** An owner/repository name as the server's `valid_name` accepts it. */
function validName(part: string): boolean {
  return part.length >= 1 && part.length <= 100 && /^[A-Za-z0-9._+\-]+$/.test(part);
}

/**
 * How many dots the segment is made of (`.` and `%2e` spellings), or -1 where it
 * is anything else: the server's `dot_segment` suspicion, counted, so `.`/`..`
 * navigation and a refused `...` read off the same walk.
 */
function dotCount(segment: string): number {
  let rest = segment;
  let dots = 0;
  while (rest !== "") {
    if (rest.startsWith(".")) {
      rest = rest.slice(1);
      dots += 1;
    } else if (rest.slice(0, 3).toLowerCase() === "%2e") {
      rest = rest.slice(3);
      dots += 1;
    } else {
      return -1;
    }
  }
  return segment === "" ? -1 : dots;
}

/**
 * The canonical document key for an absolute GitHub address, or null where the
 * server would not recognise it as a document. A step-for-step mirror of the
 * blob and raw arms of `recognize_url` in `src/documents.rs` (strict scheme and
 * host, no decoding, dot segments refused), so a key minted here is byte-for-byte
 * the key the server mints from the same address: `github:owner/repo@ref:path`.
 */
function githubKeyForUrl(address: string): string | null {
  if (address.length > MAX_ADDRESS || /[\\ \u0000-\u001f\u007f]/.test(address)) return null;
  const lower = address.toLowerCase();
  let rest: string;
  if (lower.startsWith("https://")) rest = address.slice("https://".length);
  else if (lower.startsWith("http://")) rest = address.slice("http://".length);
  else return null;
  const hostEnd = rest.search(/[/?#]/);
  const host = (hostEnd === -1 ? rest : rest.slice(0, hostEnd)).toLowerCase();
  let tail = hostEnd === -1 ? "" : rest.slice(hostEnd);
  tail = tail.split("#")[0] ?? "";
  let path = tail.split("?")[0] ?? "";
  if (path.startsWith("/")) path = path.slice(1);
  if (path.endsWith("/")) path = path.slice(0, -1);
  const segments = path === "" ? [] : path.split("/");
  if (segments.some((segment) => segment === "" || dotCount(segment) !== -1)) return null;
  if (host === "github.com" || host === "www.github.com") {
    const [owner, repo, blob, ref, ...file] = segments;
    if (owner === undefined || repo === undefined || ref === undefined || blob !== "blob") return null;
    if (!validName(owner) || !validName(repo) || file.length === 0) return null;
    return `github:${owner}/${repo}@${ref}:${file.join("/")}`;
  }
  if (host === "raw.githubusercontent.com") {
    const [owner, repo, ref, ...file] = segments;
    if (owner === undefined || repo === undefined || ref === undefined) return null;
    if (!validName(owner) || !validName(repo) || file.length === 0) return null;
    return `github:${owner}/${repo}@${ref}:${file.join("/")}`;
  }
  return null;
}

/** The parts of a `github:owner/repo@ref:path` key (delimiters read as the server minted them). */
interface GithubKeyParts {
  owner: string;
  repo: string;
  ref: string;
  path: string;
}

/** Parse a canonical GitHub document key, or null where `key` is not one. */
function parseGithubKey(key: string): GithubKeyParts | null {
  if (!key.startsWith("github:")) return null;
  const rest = key.slice("github:".length);
  const slash = rest.indexOf("/");
  const at = slash === -1 ? -1 : rest.indexOf("@", slash + 1);
  const colon = at === -1 ? -1 : rest.indexOf(":", at + 1);
  if (slash <= 0 || at <= slash + 1 || colon <= at + 1 || colon === rest.length - 1) return null;
  const parts = {
    owner: rest.slice(0, slash),
    repo: rest.slice(slash + 1, at),
    ref: rest.slice(at + 1, colon),
    path: rest.slice(colon + 1),
  };
  if (parts.ref.includes("/")) return null;
  if (!validName(parts.owner) || !validName(parts.repo)) return null;
  return parts;
}

/**
 * The external address of the document at `resolvedPath`, derived from the open
 * document's own address as the server gave it: swap the open document's
 * in-repository path at the address's tail for the resolved one, which works
 * for the blob and raw forms alike. The page names no vendor destination of
 * its own (destinations arrive as backend data), so where the address is
 * missing or does not end with the open document's path, there is nothing
 * honest to point a modifier-click at and the caller leaves the reference as
 * text rather than fabricate one.
 */
function externalUrlFor(
  currentUrl: string | null,
  currentPath: string,
  resolvedPath: string,
  anchor: string | null,
): string | null {
  if (currentUrl === null) return null;
  const trimmed = currentUrl.trim();
  const lower = trimmed.toLowerCase();
  if (!lower.startsWith("http://") && !lower.startsWith("https://")) return null;
  if (UNSAFE_URL_CHARS.test(trimmed) || trimmed.includes("\\")) return null;
  const hash = trimmed.indexOf("#");
  const noFragment = hash === -1 ? trimmed : trimmed.slice(0, hash);
  const query = noFragment.indexOf("?");
  let base = query === -1 ? noFragment : noFragment.slice(0, query);
  if (base.endsWith("/")) base = base.slice(0, -1);
  if (!base.endsWith(`/${currentPath}`)) return null;
  const url = `${base.slice(0, base.length - currentPath.length)}${resolvedPath}`;
  return anchor === null ? url : `${url}#${anchor}`;
}

/**
 * Resolve a link written in the document `currentKey` (null where no document
 * context is known). Pure over strings, so the resolution table is testable
 * without a DOM.
 *
 * Absolute GitHub blob/raw addresses name documents whatever the open document
 * is; relative and leading-`/` references resolve against the open GitHub
 * document's directory (a leading `/` is the repository root, as on GitHub),
 * with `.`/`..` normalised and any climb above the repository root refused.
 * Percent-escapes are carried verbatim, never decoded, matching the server.
 * Non-GitHub documents (paste, google_doc) have no directory to resolve
 * against, so their relative references stay text, as they did before.
 * `http(s)` non-document addresses and the in-app `#/`/`docs:` forms stay
 * ordinary links; every other scheme, protocol-relative addresses included,
 * stays text. A resolved document's external address is derived from
 * `currentUrl` — the open document's address as the server gave it — and a
 * relative reference whose external address cannot be derived stays text.
 */
export function resolveDocumentHref(
  currentKey: string | null,
  href: string,
  currentUrl: string | null = null,
): ResolvedDocumentHref {
  const trimmed = href.trim();
  if (trimmed === "" || UNSAFE_URL_CHARS.test(trimmed) || trimmed.includes("\\")) return { kind: "text" };
  if (trimmed.startsWith("#/")) return { kind: "external", url: trimmed };
  const lower = trimmed.toLowerCase();
  if (lower.startsWith("docs:")) return { kind: "external", url: trimmed };
  if (trimmed.startsWith("//")) return { kind: "text" };
  const isHttp = lower.startsWith("http://") || lower.startsWith("https://");
  if (!isHttp && /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(trimmed)) return { kind: "text" };
  const hash = trimmed.indexOf("#");
  const noFragment = hash === -1 ? trimmed : trimmed.slice(0, hash);
  const rawAnchor = hash === -1 ? null : trimmed.slice(hash + 1);
  const anchor = rawAnchor === "" ? null : rawAnchor;
  const query = noFragment.indexOf("?");
  const pathPart = query === -1 ? noFragment : noFragment.slice(0, query);
  if (isHttp) {
    const key = githubKeyForUrl(noFragment);
    return key === null ? { kind: "external", url: trimmed } : { kind: "document", key, url: trimmed, anchor };
  }
  const base = currentKey === null ? null : parseGithubKey(currentKey);
  if (base === null || currentKey === null) return { kind: "text" };
  if (pathPart === "") {
    // A bare `#heading` (or query-only reference): the open document itself.
    if (anchor === null) return { kind: "text" };
    const own = externalUrlFor(currentUrl, base.path, base.path, anchor);
    return own === null ? { kind: "text" } : { kind: "document", key: currentKey, url: own, anchor };
  }
  const segments: string[] = pathPart.startsWith("/") ? [] : base.path.split("/").slice(0, -1);
  for (const segment of pathPart.split("/")) {
    if (segment === "") continue;
    const dots = dotCount(segment);
    if (dots === 1) continue;
    if (dots === 2) {
      if (segments.length === 0) return { kind: "text" };
      segments.pop();
      continue;
    }
    if (dots > 2) return { kind: "text" };
    segments.push(segment);
  }
  if (segments.length === 0) return { kind: "text" };
  const parts: GithubKeyParts = { owner: base.owner, repo: base.repo, ref: base.ref, path: segments.join("/") };
  const key = `github:${parts.owner}/${parts.repo}@${parts.ref}:${parts.path}`;
  // The minted key rides in an attribute the reader trusts: fail closed on
  // anything that could break out of it or that the server would never mint.
  if (key.length > MAX_ADDRESS || UNSAFE_URL_CHARS.test(key) || key.includes("\\")) return { kind: "text" };
  const url = externalUrlFor(currentUrl, base.path, parts.path, anchor);
  return url === null ? { kind: "text" } : { kind: "document", key, url, anchor };
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
 * italic, and links and images held to the URL disciplines above. Anything this
 * does not recognise is copied through as the (escaped) text it is. `currentKey`
 * is the open document's key, the base its relative links resolve against;
 * a resolved document link is marked `data-doc` with this module's own minted
 * key (never raw source text), its external GitHub address as the `href`, so a
 * modifier-click, middle-click, or copy-link still goes to GitHub.
 */
function renderInline(
  text: string,
  depth: number,
  currentKey: string | null,
  currentUrl: string | null,
): string {
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
        const label = renderInline(span.label, depth + 1, currentKey, currentUrl);
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
        const resolved = resolveDocumentHref(currentKey, foldEntities(span.url), currentUrl);
        const label = renderInline(span.label, depth + 1, currentKey, currentUrl);
        if (resolved.kind === "text") {
          out += label;
        } else if (resolved.kind === "document") {
          const anchorAttr =
            resolved.anchor === null ? "" : ` data-doc-anchor="${escapeHtml(resolved.anchor)}"`;
          out +=
            `<a href="${escapeHtml(resolved.url)}" data-doc="${escapeHtml(resolved.key)}"${anchorAttr} ` +
            `title="Opens here — ${escapeHtml(resolved.url)}" rel="noopener noreferrer">${label}</a>`;
        } else {
          out += `<a href="${escapeHtml(resolved.url)}" rel="noopener noreferrer">${label}</a>`;
        }
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
        out += `<strong>${renderInline(text.slice(at + 2, end), depth + 1, currentKey, currentUrl)}</strong>`;
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
        out += `<em>${renderInline(text.slice(at + 1, end), depth + 1, currentKey, currentUrl)}</em>`;
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
 * fenced code, lists, quotes, rules, tables, and resolved links; no raw HTML,
 * no script-bearing element, and no event handler can appear in the result,
 * because the source's own angle brackets never survive the first step.
 * `currentKey` is the key of the document being rendered and `currentUrl` its
 * address as the server gave it: its relative links resolve against them, and
 * its GitHub-document links are marked for the reader to open in-app
 * (`resolveDocumentHref`). Without them, only absolute document links are
 * marked and relative references stay text.
 */
export function renderDocumentMarkdown(markdown: string, currentKey?: string, currentUrl?: string): string {
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
      blocks.push(`<h${marks.length} id="${id}">${renderInline(text, 0, currentKey ?? null, currentUrl ?? null)}</h${marks.length}>`);
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
      const head = header.map((cell) => `<th>${renderInline(cell, 0, currentKey ?? null, currentUrl ?? null)}</th>`).join("");
      const body = rows
        .map((row) => `<tr>${row.map((cell) => `<td>${renderInline(cell, 0, currentKey ?? null, currentUrl ?? null)}</td>`).join("")}</tr>`)
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
      blocks.push(`<blockquote><p>${renderInline(quoted.join(" "), 0, currentKey ?? null, currentUrl ?? null)}</p></blockquote>`);
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
      blocks.push(`<${tag}>${items.map((item) => `<li>${renderInline(item, 0, currentKey ?? null, currentUrl ?? null)}</li>`).join("")}</${tag}>`);
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
    blocks.push(`<p>${renderInline(paragraph.join("\n"), 0, currentKey ?? null, currentUrl ?? null)}</p>`);
  }
  return blocks.join("\n");
}

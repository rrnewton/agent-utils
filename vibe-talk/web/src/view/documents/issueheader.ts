// The issue header (`#1176 doc-kind-buckets`): a minibeads issue document opens
// with YAML front matter (title, status, and friends). Rendered raw, the reader
// shows that YAML as body text; split off here, the reader draws it as a small
// header — `#<n>`, the title, the status — over the body that follows it.
// Pure string work, no DOM and no markup out: the reader sets the values as
// text, so nothing here can become a sink.

/** What an issue header names: the row's fields where the server gave them. */
export interface IssueHeaderMeta {
  number?: number | null;
  status?: string | null;
  title?: string | null;
}

/** An issue document split into its header fields and the remaining body. */
export interface IssueHeader {
  /** The heading line: `#<n>` plus the title, either part optional. */
  heading: string;
  /** The issue's status (open, in_progress, closed), when named. */
  status: string | null;
  /** The Markdown body after the front matter. */
  body: string;
}

/** One front-matter field's value: trimmed, one layer of quotes removed. */
function field(lines: readonly string[], name: string): string | null {
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith(`${name}:`)) {
      const value = trimmed
        .slice(name.length + 1)
        .trim()
        .replace(/^"(.*)"$/, "$1")
        .replace(/^'(.*)'$/, "$1");
      return value === "" ? null : value;
    }
  }
  return null;
}

/**
 * Split `markdown` into an issue header plus body, or null when it does not
 * open with a complete `---` front-matter block. `meta` (the document row's
 * issue fields) wins over the front matter where both name a value.
 */
export function splitIssueHeader(markdown: string, meta: IssueHeaderMeta = {}): IssueHeader | null {
  const lines = markdown.split("\n");
  if ((lines[0] ?? "").trim() !== "---") return null;
  let end = -1;
  for (let i = 1; i < lines.length; i += 1) {
    if ((lines[i] ?? "").trim() === "---") {
      end = i;
      break;
    }
  }
  if (end === -1) return null;
  const front = lines.slice(1, end);
  const title = meta.title ?? field(front, "title");
  const status = meta.status ?? field(front, "status");
  const parts: string[] = [];
  if (meta.number !== null && meta.number !== undefined) parts.push(`#${meta.number}`);
  if (title !== null) parts.push(title);
  return { heading: parts.join(" "), status, body: lines.slice(end + 1).join("\n") };
}

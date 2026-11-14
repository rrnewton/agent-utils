// The Documents reader's ONE block model (`#1505
// documents-read-controls-highlight-tap`): how a document is cut into the
// blocks it is read aloud in. The reader speaks these blocks, lights
// these blocks and starts reading at a tapped one of these blocks, so
// block index N means the same drawn block on every path.
//
// The cut mirrors `renderDocumentMarkdown`'s block splitting step for
// step — a heading, a paragraph, a block quote, a code fence and a table
// are one block each; EVERY list-item line is a block of its own (a whole
// list is not one chunk), with an indented continuation joining its item
// as the renderer joins it; a rule is no block. The drawn body holds the
// same blocks in the same order (`documentBlocks` in reader.ts walks it
// by the same rule), which the module tests hold this file to.

/** One block of a document read: its owner's kind and the words said of it. */
export interface DocumentReadBlock {
  kind: "paragraph" | "heading" | "item" | "quote" | "code" | "table";
  text: string;
  /** A heading block's source level (its `#` count, 1-6); absent otherwise. */
  level?: number;
}

/** Inline dressing off one line of prose: links and images say their words. */
function speakable(text: string): string {
  return text
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\*\*([^*]*)\*\*/g, "$1")
    .replace(/\*([^*]*)\*/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/** The cells of one table row, the framing pipes dropped (render.ts's rule). */
function tableCells(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|")) text = text.slice(0, -1);
  return text.split("|").map((cell) => cell.trim());
}

/** Whether `line` is a table's header separator: dashes, colons, pipes only. */
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
  if (line.startsWith(">")) return true;
  if (/^[-*+]\s+/.test(line) || /^\d{1,9}[.)]\s+/.test(line)) return true;
  return isTableStart(lines, at);
}

/**
 * The blocks `markdown` is read in, in drawn order. Pure over the source
 * text, so the cut is testable without a reader; the reader hands these
 * texts (and kinds) to the shared read-aloud owner and lights the drawn
 * block of the same index.
 */
export function documentReadBlocks(markdown: string): DocumentReadBlock[] {
  const lines = markdown.replace(/\u0000/g, "").replace(/\r\n?/g, "\n").split("\n");
  const blocks: DocumentReadBlock[] = [];
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
      blocks.push({ kind: "code", text: code.join(" ").replace(/\s+/g, " ").trim() });
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading !== null) {
      blocks.push({
        kind: "heading",
        text: speakable((heading[2] ?? "").replace(/\s+#+\s*$/, "")),
        level: (heading[1] ?? "#").length,
      });
      at += 1;
      continue;
    }
    if (/^(-{3,}|\*{3,}|_{3,})$/.test(line)) {
      at += 1;
      continue;
    }
    if (isTableStart(lines, at)) {
      const said: string[] = [tableCells(line).join(" ")];
      at += 2;
      while (at < lines.length) {
        const row = (lines[at] ?? "").trim();
        if (row === "" || !row.includes("|")) break;
        said.push(tableCells(row).join(" "));
        at += 1;
      }
      blocks.push({ kind: "table", text: speakable(said.join(" ")) });
      continue;
    }
    if (line.startsWith(">")) {
      const quoted: string[] = [];
      while (at < lines.length) {
        const quoteLine = (lines[at] ?? "").trim();
        if (!quoteLine.startsWith(">")) break;
        quoted.push(quoteLine.replace(/^>\s?/, ""));
        at += 1;
      }
      blocks.push({ kind: "quote", text: speakable(quoted.join(" ")) });
      continue;
    }
    const unordered = /^[-*+]\s+(.*)$/.exec(line);
    const ordered = /^\d{1,9}[.)]\s+(.*)$/.exec(line);
    if (unordered !== null || ordered !== null) {
      const isOrdered = unordered === null;
      const items: string[] = [];
      while (at < lines.length) {
        const raw = lines[at] ?? "";
        const itemLine = raw.trim();
        if (itemLine === "") break;
        const item = isOrdered
          ? /^\d{1,9}[.)]\s+(.*)$/.exec(itemLine)
          : /^[-*+]\s+(.*)$/.exec(itemLine);
        if (item !== null) {
          items.push(item[1] ?? "");
          at += 1;
          continue;
        }
        // As the renderer: a block of its own, an item of the other kind,
        // or prose at the left margin ends the list; an indented line is
        // the current item wrapped over source lines and joins it.
        if (startsBlock(lines, at) || !/^[ \t]/.test(raw) || items.length === 0) break;
        items[items.length - 1] = `${items[items.length - 1]} ${itemLine}`;
        at += 1;
      }
      for (const item of items) blocks.push({ kind: "item", text: speakable(item) });
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
    blocks.push({ kind: "paragraph", text: speakable(paragraph.join(" ")) });
  }
  return blocks;
}

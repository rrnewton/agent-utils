// The Documents reader: one fetched document, drawn safe and read aloud on request.
//
// The document body is the one place in this view set from markup, and it is fed
// only by `renderDocumentMarkdown`, whose output carries no markup the document
// itself wrote. Everything else the reader shows (the title, the outline, any
// status) is set as text. Construction does nothing: no element is looked up, no
// listener is added, and nothing is fetched until `open` is called.

import { createListening } from "../dom/index.ts";
import { documentHeadingId, renderDocumentMarkdown } from "./render.ts";

/** What the reader asks of the page. */
export interface DocumentsReaderDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  speak(text: string): unknown;
  stopSpeaking?(): unknown;
  onBack(): unknown;
  onStatus?(text: string): unknown;
}

/** The reader's commands. */
export interface DocumentsReader {
  /** Fetch the document `key` (fetching its content first if uncached) and draw it. */
  open(key: string): Promise<void>;
  /** Stop any reading in progress. */
  close(): void;
  /** Take down every listener the reader added. */
  unmount(): void;
}

/** What `GET /api/v1/documents/{key}` answers, as far as the reader uses it. */
interface DocumentAnswer {
  document: { title?: unknown; moved_to?: unknown } | null;
  content: unknown;
  outline: unknown;
}

/** Narrow an `api` answer to the fields the reader reads, or an empty answer. */
function asAnswer(value: unknown): DocumentAnswer {
  if (typeof value !== "object" || value === null) return { document: null, content: null, outline: null };
  const record = value as { document?: unknown; content?: unknown; outline?: unknown };
  const doc = record.document;
  return {
    document: typeof doc === "object" && doc !== null ? (doc as { title?: unknown; moved_to?: unknown }) : null,
    content: record.content,
    outline: record.outline,
  };
}

/** The strings among `value`, in order; none where it is not a list of them. */
function stringsOf(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string");
}

/**
 * The plain text of a Markdown document, for reading aloud: fence markers, heading
 * and quote and list marks, emphasis, code ticks, links and tables reduced to the
 * words they carry, split into chunks on blank lines.
 */
function plainChunks(markdown: string): string[] {
  const kept: string[] = [];
  for (const rawLine of markdown.replace(/\r\n?/g, "\n").split("\n")) {
    if (rawLine.trim().startsWith("```")) continue;
    let text = rawLine
      .replace(/^\s{0,3}#{1,6}\s+/, "")
      .replace(/^\s*>\s?/, "")
      .replace(/^\s*[-*+]\s+/, "")
      .replace(/^\s*\d{1,9}[.)]\s+/, "");
    if (text.includes("-") && /^\s*\|?[\s:|-]+\|?\s*$/.test(text)) continue;
    text = text
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/`([^`]*)`/g, "$1")
      .replace(/\*\*([^*]*)\*\*/g, "$1")
      .replace(/\*([^*]*)\*/g, "$1")
      .replace(/\|/g, " ");
    kept.push(text);
  }
  return kept
    .join("\n")
    .split(/\n\s*\n/)
    .map((chunk) => chunk.replace(/\s+/g, " ").trim())
    .filter((chunk) => chunk !== "");
}

/** A reader over the shell's document elements, idle until `open`. */
export function createDocumentsReader(deps: DocumentsReaderDependencies): DocumentsReader {
  const listening = createListening();
  let wired = false;
  let markdown: string | null = null;
  let speaking = false;

  /** Scroll the rendered heading `id` into view, where it was drawn. */
  function scrollToHeading(id: string): void {
    const body = deps.byId("doc-body");
    const found = Array.from(body.querySelectorAll("h1, h2, h3, h4, h5, h6")).find((el) => el.id === id);
    const target = found === undefined ? null : (found as HTMLElement);
    if (target !== null && typeof target.scrollIntoView === "function") {
      target.scrollIntoView({ block: "start" });
    }
  }

  /** Draw the outline as a list of buttons, each landing on its heading's anchor. */
  function drawOutline(entries: string[]): void {
    const list = deps.byId("doc-outline");
    list.textContent = "";
    const seen = new Map<string, number>();
    for (const entry of entries) {
      const base = documentHeadingId(entry);
      const count = seen.get(base) ?? 0;
      seen.set(base, count + 1);
      const id = count === 0 ? base : `${base}-${count + 1}`;
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = entry;
      listening.listen(button, "click", () => scrollToHeading(id));
      item.appendChild(button);
      list.appendChild(item);
    }
  }

  /** Speak the current document's plain text, one chunk per blank-line break. */
  function readAloud(): void {
    if (markdown === null) {
      deps.onStatus?.("Nothing to read yet.");
      return;
    }
    const chunks = plainChunks(markdown);
    if (chunks.length === 0) {
      deps.onStatus?.("Nothing to read yet.");
      return;
    }
    speaking = true;
    for (const chunk of chunks) deps.speak(chunk);
  }

  /** Stop this reader's own reading, and only its own. */
  function stopOwnReading(): void {
    if (!speaking) return;
    speaking = false;
    deps.stopSpeaking?.();
  }

  /** Add the Back and Read listeners, the first time the reader is opened. */
  function wire(): void {
    if (wired) return;
    wired = true;
    listening.listen(deps.byId("doc-back"), "click", () => {
      stopOwnReading();
      deps.onBack();
    });
    listening.listen(deps.byId("doc-read"), "click", () => {
      readAloud();
    });
  }

  return {
    async open(key: string): Promise<void> {
      wire();
      const path = `/api/v1/documents/${encodeURIComponent(key)}`;
      let answer: DocumentAnswer;
      try {
        answer = asAnswer(await deps.api(path));
        if (typeof answer.content !== "string") {
          await deps.api("/api/v1/documents/fetch", { method: "POST", body: { key } });
          answer = asAnswer(await deps.api(path));
        }
      } catch (error) {
        deps.onStatus?.("Could not load the document.");
        throw error;
      }
      markdown = typeof answer.content === "string" ? answer.content : null;
      const outline = stringsOf(answer.outline);
      const rawTitle = answer.document?.title;
      const docTitle = typeof rawTitle === "string" ? rawTitle : "";
      deps.byId("doc-title").textContent = docTitle !== "" ? docTitle : (outline[0] ?? key);
      const movedTo = answer.document?.moved_to;
      if (typeof movedTo === "string" && movedTo !== "") deps.onStatus?.(`Moved to ${movedTo}`);
      const body = deps.byId("doc-body");
      if (markdown !== null) {
        // The documents sink: fed solely by the sanitizer above, never raw text.
        body.innerHTML = renderDocumentMarkdown(markdown);
      } else {
        body.textContent = "This document is not available yet.";
        deps.onStatus?.("This document is not available yet.");
      }
      drawOutline(outline);
    },
    close(): void {
      stopOwnReading();
    },
    unmount(): void {
      wired = false;
      listening.unmount();
    },
  };
}

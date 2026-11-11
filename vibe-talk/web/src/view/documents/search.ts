// The Documents search (`#1266 documents-search`): a Search button in the
// Documents list that searches only FETCHED documents, through the server's
// documents scope (`GET /api/v1/documents/search`), which runs the message
// search's language and matching over fetched titles plus bodies. The bar,
// result list and highlight reuse the message search's shape: a field, a
// status line saying how far the search reached ("N fetched documents
// searched", so the bounded cache's reach is never silent), and results that
// open the reader at the match. All server words are untrusted: text only.

import { createListening } from "../dom/index.ts";

/** One hit of `GET /api/v1/documents/search`, as far as the list reads it. */
export interface DocumentSearchHit {
  key: string;
  title: string;
  kind: string;
  fetched_ms: number;
  title_match: boolean;
  snippet: string;
  snippet_start: number;
}

export interface DocumentsSearchDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  /** Open document `key` in the reader at its match of `terms`. */
  onOpen(key: string, terms: readonly string[]): unknown;
  onStatus?(text: string): unknown;
}

/** The Documents search's commands. */
export interface DocumentsSearch {
  /** Run a search for `query` now (the bar's Search and Enter). */
  search(query: string): Promise<void>;
  unmount(): void;
}

function asHits(value: unknown): { hits: DocumentSearchHit[]; searched: number; terms: string[] } {
  if (typeof value !== "object" || value === null) return { hits: [], searched: 0, terms: [] };
  const record = value as { hits?: unknown; searched?: unknown; terms?: unknown };
  const hits = Array.isArray(record.hits)
    ? record.hits.filter(
        (hit): hit is DocumentSearchHit =>
          typeof hit === "object" &&
          hit !== null &&
          typeof (hit as DocumentSearchHit).key === "string" &&
          typeof (hit as DocumentSearchHit).title === "string",
      )
    : [];
  const terms = Array.isArray(record.terms)
    ? record.terms.filter((term): term is string => typeof term === "string")
    : [];
  return { hits, searched: typeof record.searched === "number" ? record.searched : 0, terms };
}

/** Create the Documents search over the list's search elements. */
export function createDocumentsSearch(deps: DocumentsSearchDependencies): DocumentsSearch {
  const listening = createListening();
  let wired = false;
  let ticket = 0;

  function el(id: string): HTMLElement | null {
    try {
      return deps.byId(id) ?? null;
    } catch {
      return null;
    }
  }

  function render(hits: DocumentSearchHit[], searched: number, terms: readonly string[]): void {
    const note = el("docs-search-note");
    if (note) {
      note.hidden = false;
      note.textContent =
        hits.length === 0
          ? `No fetched document matches that search · ${searched} fetched document${searched === 1 ? "" : "s"} searched`
          : `${hits.length} match${hits.length === 1 ? "" : "es"} · ${searched} fetched document${searched === 1 ? "" : "s"} searched`;
    }
    const list = el("docs-search-results");
    if (!list) return;
    const rows = hits.map((hit) => {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.setAttribute("type", "button");
      button.className = "doc-search-hit";
      button.setAttribute("data-key", hit.key);
      const title = document.createElement("span");
      title.className = "doc-search-hit-title";
      title.textContent = hit.title || hit.key;
      const snippet = document.createElement("span");
      snippet.className = "doc-search-hit-snippet";
      snippet.textContent = hit.snippet.replace(/\s+/g, " ").trim();
      button.append(title, snippet);
      listening.listen(button, "click", () => deps.onOpen(hit.key, terms));
      item.appendChild(button);
      return item;
    });
    list.replaceChildren(...rows);
  }

  async function search(query: string): Promise<void> {
    const text = query.trim();
    if (text === "") return;
    const mine = ++ticket;
    try {
      const answer = await deps.api(
        `/api/v1/documents/search?q=${encodeURIComponent(text)}&limit=20`,
      );
      if (mine !== ticket) return;
      const { hits, searched, terms } = asHits(answer);
      render(hits, searched, terms);
    } catch (error) {
      if (mine !== ticket) return;
      deps.onStatus?.(
        `Could not search the fetched documents. ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  function wire(): void {
    if (wired) return;
    wired = true;
    const toggle = el("docs-search-toggle");
    const bar = el("docs-search-bar");
    const field = el("docs-search-field") as HTMLInputElement | null;
    const go = el("docs-search-go");
    if (toggle && bar) {
      listening.listen(toggle, "click", () => {
        const open = bar.hidden;
        bar.hidden = !open;
        toggle.setAttribute("aria-expanded", String(open));
        if (open && field && typeof field.focus === "function") field.focus();
      });
    }
    if (go && field) {
      listening.listen(go, "click", () => void search(field.value ?? ""));
    }
    if (field) {
      listening.listen(field, "keydown", (event: KeyboardEvent) => {
        if (event.key === "Enter" && !event.isComposing) {
          event.preventDefault();
          void search(field.value ?? "");
        }
      });
    }
  }

  wire();
  return {
    search,
    unmount(): void {
      wired = false;
      listening.unmount();
    },
  };
}

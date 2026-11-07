// The Documents list (`#629 documents-view-client`): one small card per document the fold
// knows, in the order the server gives them — mention order, newest last — with unfetched
// documents dimmed until a tap fetches them. All words from the server are untrusted, so
// every visible string goes through `textContent`, never markup. Construction does no
// fetch and no DOM writes; the first effect is an explicit `refresh()`.

import { createListening } from "../dom/index.ts";
import type { DocumentRow, DocumentsListAnswer } from "./types.ts";

export interface DocumentsListDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  onOpen(key: string): unknown;
  onStatus?(text: string): unknown;
}

/** The Documents list's commands. */
export interface DocumentsList {
  /** Fetch the list (`limit=100`) and draw it again, in the server's order. */
  refresh(): Promise<void>;
  /** Fetch the most recent unfetched documents, then draw the list again. */
  fetchRecent(): Promise<void>;
  /** Take down every listener the list added. */
  unmount(): void;
}

/** The element the integrator's shell gives the list. */
const LIST_ID = "documents-list";
const LIST_PATH = "/api/v1/documents?limit=100";
const FETCH_PATH = "/api/v1/documents/fetch";
const FETCH_RECENT_PATH = "/api/v1/documents/fetch-recent";
const FETCH_RECENT_LIMIT = 10;

export function createDocumentsList(deps: DocumentsListDependencies): DocumentsList {
  const listening = createListening();

  function cardFor(row: DocumentRow): HTMLElement {
    const card = document.createElement("button");
    card.setAttribute("type", "button");
    card.className = row.fetched ? "doc-card" : "doc-card doc-unfetched";
    card.setAttribute("data-key", row.key);
    const title = document.createElement("span");
    title.className = "doc-title";
    title.textContent = row.title;
    const subtitle = document.createElement("span");
    subtitle.className = "doc-subtitle";
    subtitle.textContent = `${row.file_name} · ${row.mention_count} ${row.mention_count === 1 ? "mention" : "mentions"} · ${row.kind}`;
    card.append(title, subtitle);
    listening.listen(card, "click", () => {
      void open(row);
    });
    return card;
  }

  async function open(row: DocumentRow): Promise<void> {
    if (row.fetched) {
      deps.onOpen(row.key);
      return;
    }
    try {
      await deps.api(FETCH_PATH, { method: "POST", body: { key: row.key } });
    } catch (error) {
      deps.onStatus?.(error instanceof Error ? error.message : String(error));
      return;
    }
    deps.onOpen(row.key);
  }

  function render(rows: readonly DocumentRow[]): void {
    // Cards about to be replaced take their listeners down with them.
    listening.unmount();
    const list = deps.byId(LIST_ID);
    if (rows.length === 0) {
      const note = document.createElement("p");
      note.className = "doc-empty";
      note.textContent = "No documents yet.";
      list.replaceChildren(note);
      return;
    }
    list.replaceChildren(...rows.map(cardFor));
  }

  async function refresh(): Promise<void> {
    const answer = (await deps.api(LIST_PATH)) as DocumentsListAnswer;
    render(Array.isArray(answer?.documents) ? answer.documents : []);
  }

  return {
    refresh,
    async fetchRecent(): Promise<void> {
      await deps.api(FETCH_RECENT_PATH, { method: "POST", body: { limit: FETCH_RECENT_LIMIT } });
      await refresh();
    },
    unmount(): void {
      listening.unmount();
    },
  };
}

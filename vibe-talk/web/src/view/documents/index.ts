// view/documents (`documents-view-client`): the Documents view — the list of
// documents linked in chat, and the full-screen reader for one of them. Its
// public interface; the page mounts it once and shows it as the `docs` view.

export { createDocumentsList } from "./list.ts";
export type { DocumentsList, DocumentsListDependencies } from "./list.ts";
export { createDocumentsReader } from "./reader.ts";
export type { DocumentsReader, DocumentsReaderDependencies } from "./reader.ts";
export { renderDocumentMarkdown, documentHeadingId } from "./render.ts";
export { createViewSelector, nextView, otherViews, VIEW_LABELS, VIEW_ORDER } from "./selector.ts";
export type { DocViewName, ViewSelector, ViewSelectorDependencies } from "./selector.ts";
export type { DocumentMention, DocumentRow, DocumentsListAnswer } from "./types.ts";

import { createListening } from "../dom/index.ts";
import { createDocumentsList } from "./list.ts";
import { createDocumentsReader } from "./reader.ts";

/** What the Documents view needs from the page. */
export interface DocumentsViewDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  speak(text: string): unknown;
  stopSpeaking?(): unknown;
  onStatus?(text: string): unknown;
}

/** The Documents view, once mounted. */
export interface DocumentsView {
  /** Show the list (refreshing it) or, with `key`, open that document. */
  show(key?: string): Promise<void>;
  /** Back from the reader to the list; stops any reading. */
  showList(): void;
  /** Whether the reader (rather than the list) is up. */
  reading(): boolean;
  /** Take down every listener the view added. */
  unmount(): void;
}

/** Mount the Documents view over `#pane-docs`. Construction does nothing. */
export function createDocumentsView(deps: DocumentsViewDependencies): DocumentsView {
  const listening = createListening();
  let readingNow = false;
  let wired = false;

  const reader = createDocumentsReader({
    api: deps.api,
    byId: deps.byId,
    speak: deps.speak,
    stopSpeaking: deps.stopSpeaking,
    onBack: () => showList(),
    onStatus: deps.onStatus,
  });
  const list = createDocumentsList({
    api: deps.api,
    byId: deps.byId,
    onOpen: (key) => {
      void openDocument(key);
    },
    onStatus: deps.onStatus,
  });

  function showList(): void {
    readingNow = false;
    reader.close();
    deps.byId("docs-reader").hidden = true;
    deps.byId("docs-list-view").hidden = false;
  }

  async function openDocument(key: string): Promise<void> {
    deps.byId("docs-list-view").hidden = true;
    deps.byId("docs-reader").hidden = false;
    readingNow = true;
    try {
      await reader.open(key);
    } catch {
      // The reader already said what happened; stay on the reader so Back works.
    }
  }

  function wire(): void {
    if (wired) return;
    wired = true;
    listening.listen(deps.byId("docs-fetch-recent"), "click", () => {
      void list.fetchRecent().catch((error: unknown) => {
        deps.onStatus?.(error instanceof Error ? error.message : String(error));
      });
    });
  }

  return {
    async show(key?: string): Promise<void> {
      wire();
      if (key) {
        await openDocument(key);
        return;
      }
      showList();
      try {
        await list.refresh();
      } catch (error) {
        deps.onStatus?.(error instanceof Error ? error.message : "Could not load the documents.");
      }
    },
    showList,
    reading: () => readingNow,
    unmount(): void {
      wired = false;
      listening.unmount();
      list.unmount();
      reader.unmount();
    },
  };
}

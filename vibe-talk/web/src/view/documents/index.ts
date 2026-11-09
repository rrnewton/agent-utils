// view/documents (`documents-view-client`): the Documents view — the list of
// documents linked in chat, and the full-screen reader for one of them. Its
// public interface; the page mounts it once and shows it as the `docs` view.

export { createDocumentsList } from "./list.ts";
export type { DocumentsList, DocumentsListDependencies } from "./list.ts";
export { createDocumentsReader, currentOutlineIndex, openDocumentNotice } from "./reader.ts";
export type { DocumentsReader, DocumentsReaderDependencies, OpenDocument } from "./reader.ts";
export { renderDocumentMarkdown, documentHeadingId } from "./render.ts";
export { createViewSelector, nextView, otherViews, VIEW_CARDS_SETTLE_MS, VIEW_LABELS, VIEW_ORDER } from "./selector.ts";
export type { DocViewName, ViewSelector, ViewSelectorDependencies } from "./selector.ts";
export type { DocumentMention, DocumentRow, DocumentsListAnswer } from "./types.ts";

import { createListening } from "../dom/index.ts";
import { createDocumentsList } from "./list.ts";
import { createDocumentsReader, openDocumentNotice } from "./reader.ts";
import type { OpenDocument } from "./reader.ts";

/** What the Documents view needs from the page. */
export interface DocumentsViewDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  /** Read one document aloud through the page's shared read-aloud owner (`#1039`). */
  readDocument(doc: { key: string; title?: string; texts: readonly string[] }): unknown;
  /** Stop the reading of document `key`, where that document's is the one playing; no other. */
  stopSpeaking?(key: string): unknown;
  onStatus?(text: string): unknown;
  /** The open levels changed (a document opened or closed): sync the history. */
  onLevelChange?(): unknown;
  /**
   * The reader's Back button was pressed (`#1082 documents-back-chain`): the
   * page's navigation pops its top back level, the same pop the back gesture
   * takes, so a document opened from a channel returns there. Where the page
   * wires none, the view pops its own chain one step.
   */
  onBack?(): unknown;
  /** Jump to the message that mentioned a document. */
  onJumpToMention?(mention: import("./types.ts").DocumentMention): unknown;
}

/** The Documents view, once mounted. */
export interface DocumentsView {
  /** Show the list (refreshing it) or, with `key`, open that document. */
  show(key?: string): Promise<void>;
  /**
   * Back from the reader one step (`#1082 documents-back-chain`): to the
   * previous document in the open chain, or to the list at the chain's start;
   * stops any reading where it reaches the list. The history's close of a
   * `document` level and the view's own way back both take this one step.
   */
  showList(): void;
  /** The same one step back as `showList`, under its Back name. */
  back(): void;
  /** How many documents are open in the chain (0 on the list): its back levels. */
  depth(): number;
  /** Whether the reader (rather than the list) is up. */
  reading(): boolean;
  /** The document open in the reader while it is up, else null (`#1040`). */
  current(): OpenDocument | null;
  /** The voice notice for the open document, or null where none is open. */
  documentNotice(): string | null;
  /** Take down every listener the view added. */
  unmount(): void;
}

/** Mount the Documents view over `#pane-docs`. Construction does nothing. */
export function createDocumentsView(deps: DocumentsViewDependencies): DocumentsView {
  const listening = createListening();
  let readingNow = false;
  let wired = false;
  let liveTimer = 0;
  /**
   * The open chain (`#1082 documents-back-chain`): every document opened
   * through this view, the reader's current one last, each with the shared
   * scroller's place when the next document opened over it. Back pops one.
   * Only the chain's depth leaves the view (as back levels, through the
   * page's wiring); where the chain's first document was opened from — a
   * channel, the list — is the page navigation's jump, not the view's.
   */
  let chain: { key: string; scrollTop: number }[] = [];

  /** The shared scroller's place now, or 0 where it cannot be read. */
  function scrollTopNow(): number {
    try {
      const top = deps.byId("scroll-area").scrollTop;
      return typeof top === "number" && Number.isFinite(top) ? top : 0;
    } catch {
      return 0;
    }
  }

  /** Put the shared scroller back at `top`, where it can be written. */
  function restoreScrollTop(top: number): void {
    try {
      deps.byId("scroll-area").scrollTop = top;
    } catch {
      // A host without the scroller keeps the reader where it opened.
    }
  }

  const reader = createDocumentsReader({
    api: deps.api,
    byId: deps.byId,
    readDocument: deps.readDocument,
    stopSpeaking: deps.stopSpeaking,
    onBack: () => {
      // The same pop as the gesture, where the page wired its navigation;
      // the view's own one step where it did not.
      if (deps.onBack) deps.onBack();
      else back();
    },
    onStatus: deps.onStatus,
    // A document opened from a document (`#1081 documents-relative-links`)
    // goes through this same `openDocument` entry a list tap uses — the one
    // open path `#1082 documents-back-chain` builds its chain on.
    onOpenDocument: (key) => openDocument(key),
  });
  const list = createDocumentsList({
    api: deps.api,
    byId: deps.byId,
    onOpen: (key) => {
      void openDocument(key);
    },
    onMentions: (row) => showMentions(row),
    onStatus: deps.onStatus,
  });

  /** The mentioning messages of `row`: straight to one, a choice for several. */
  function showMentions(row: import("./types.ts").DocumentRow): void {
    const mentions = row.mentions ?? [];
    if (mentions.length === 1) {
      deps.onJumpToMention?.(mentions[0]!);
      return;
    }
    if (mentions.length === 0) return;
    const panel = deps.byId("docs-mentions");
    panel.replaceChildren();
    for (const mention of mentions) {
      const item = document.createElement("button");
      item.setAttribute("type", "button");
      item.className = "doc-mention-item";
      const when = mention.posted_ms !== null ? new Date(mention.posted_ms).toLocaleString() : "";
      const snippet = (mention.label ?? "").replace(/\s+/g, " ").slice(0, 80);
      item.textContent = `${mention.author} · ${when}${snippet ? ` — ${snippet}` : ""}`;
      item.addEventListener("click", () => {
        panel.hidden = true;
        deps.onJumpToMention?.(mention);
      });
      panel.appendChild(item);
    }
    panel.hidden = false;
  }

  /** To the list, the chain forgotten: a view switch here, never a Back step. */
  function toList(redraw = true): void {
    const wasReading = readingNow;
    readingNow = false;
    chain = [];
    if (wasReading) deps.onLevelChange?.();
    reader.close();
    deps.byId("docs-reader").hidden = true;
    deps.byId("docs-list-view").hidden = false;
    // Back from a fetch: the rows redraw with their fetched state.
    if (redraw) void list.refresh().catch(() => undefined);
  }

  /**
   * One step back: the previous document, reopened at its scroller place, or
   * the list at the chain's start. This is what the history closing one
   * `document` level does, so gesture and button walk the same chain.
   */
  function back(): void {
    if (!readingNow || chain.length <= 1) {
      toList();
      return;
    }
    chain.pop();
    const previous = chain[chain.length - 1] as { key: string; scrollTop: number };
    // The depth changed before the reopen lands, so its entry syncs at once.
    deps.onLevelChange?.();
    // Stop the reading of the document being left, as reaching the list does.
    reader.close();
    void (async () => {
      try {
        await reader.open(previous.key);
      } catch {
        // The reader already said what happened; stay on the reader so Back works.
      }
      if (readingNow && chain[chain.length - 1] === previous) restoreScrollTop(previous.scrollTop);
    })();
  }

  async function openDocument(key: string): Promise<void> {
    const current = readingNow ? chain[chain.length - 1] : undefined;
    if (current) {
      // The place a Back to this document returns to.
      current.scrollTop = scrollTopNow();
      // Reopening the document already open adds no step of its own.
      if (current.key !== key) chain.push({ key, scrollTop: 0 });
    } else {
      chain = [{ key, scrollTop: 0 }];
    }
    deps.byId("docs-list-view").hidden = true;
    deps.byId("docs-reader").hidden = false;
    readingNow = true;
    deps.onLevelChange?.();
    try {
      await reader.open(key);
    } catch {
      // The reader already said what happened; stay on the reader so Back works.
    }
  }

  function wire(): void {
    if (wired) return;
    wired = true;
    const button = deps.byId("docs-fetch") as HTMLButtonElement;
    const menu = deps.byId("docs-fetch-menu");
    const since = deps.byId("docs-since");
    let clearArmed = false;
    const setBusy = (on: boolean, label?: string): void => {
      button.disabled = on;
      button.textContent = on ? (label ?? "Fetching…") : "Fetch";
    };
    const report = (run: Promise<unknown>, done: (value: never) => string): void => {
      setBusy(true);
      run.then(
        (value) => deps.onStatus?.(done(value as never)),
        (error: unknown) => deps.onStatus?.(error instanceof Error ? error.message : String(error)),
      ).finally(() => setBusy(false));
    };
    listening.listen(button, "click", () => {
      const open = menu.hidden;
      menu.hidden = !open;
      button.setAttribute("aria-expanded", String(open));
      if (!open) since.hidden = true;
    });
    for (const item of ["docs-fetch-visible", "docs-fetch-since", "docs-fetch-refresh", "docs-fetch-clear"].map(
      (id) => deps.byId(id),
    )) {
      listening.listen(item, "click", () => {
        const act = item.getAttribute("data-act");
        if (act !== "clear") {
          menu.hidden = true;
          button.setAttribute("aria-expanded", "false");
          clearArmed = false;
        }
        if (act === "visible") report(list.fetchVisible(), (n) => `Fetched ${n} document${n === 1 ? "" : "s"}.`);
        else if (act === "since") since.hidden = false;
        else if (act === "refresh") {
          setBusy(true, "Refreshing…");
          list
            .refreshFetched((doneCount, total) => {
              if (total > 0) button.textContent = `Refreshing ${doneCount}/${total}…`;
            })
            .then(
              ({ updated, current }) =>
                deps.onStatus?.(`${updated} updated, ${current} current.`),
              (error: unknown) => deps.onStatus?.(error instanceof Error ? error.message : String(error)),
            )
            .finally(() => setBusy(false));
        } else if (act === "clear") {
          if (!clearArmed) {
            clearArmed = true;
            item.textContent = "Delete every cached document?";
            return;
          }
          clearArmed = false;
          item.textContent = "Clear cached documents";
          menu.hidden = true;
          button.setAttribute("aria-expanded", "false");
          report(list.clearCached(), (n) => `Deleted ${n} cached document${n === 1 ? "" : "s"}.`);
        }
      });
    }
    listening.listen(deps.byId("docs-since-go"), "click", () => {
      const days = Number((deps.byId("docs-since-days") as HTMLInputElement).value) || 0;
      const hours = Number((deps.byId("docs-since-hours") as HTMLInputElement).value) || 0;
      since.hidden = true;
      report(
        list.fetchSince(Date.now() - ((days * 24 + hours) * 3_600_000)),
        (n) => `Fetched ${n} document${n === 1 ? "" : "s"}.`,
      );
    });
  }

  return {
    async show(key?: string): Promise<void> {
      wire();
      // While the view stays open, keep it current (a minute's re-check).
      if (liveTimer === 0) {
        const tick = (): void => {
          if (!deps.byId("pane-docs").hidden) void list.refresh().catch(() => undefined);
          liveTimer = setTimeout(tick, 60_000) as unknown as number;
        };
        liveTimer = setTimeout(tick, 60_000) as unknown as number;
      }
      if (key) {
        await openDocument(key);
        return;
      }
      // Shown with no document named — a switch to this view: the list, the
      // chain forgotten, however the reader was last left.
      toList(false);
      try {
        await list.refresh();
      } catch (error) {
        deps.onStatus?.(error instanceof Error ? error.message : "Could not load the documents.");
      }
    },
    showList: back,
    back,
    depth: () => (readingNow ? Math.max(1, chain.length) : 0),
    reading: () => readingNow,
    current(): OpenDocument | null {
      return readingNow ? reader.current() : null;
    },
    documentNotice(): string | null {
      const doc = readingNow ? reader.current() : null;
      return doc === null ? null : openDocumentNotice(doc);
    },
    unmount(): void {
      if (liveTimer !== 0) clearTimeout(liveTimer);
      liveTimer = 0;
      wired = false;
      listening.unmount();
      list.unmount();
      reader.unmount();
    },
  };
}

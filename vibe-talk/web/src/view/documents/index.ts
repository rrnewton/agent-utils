// view/documents (`documents-view-client`): the Documents view — the list of
// documents linked in chat, and the full-screen reader for one of them. Its
// public interface; the page mounts it once and shows it as the `docs` view.

export { createDocumentsList, dedupeDocumentRows, documentIdentity } from "./list.ts";
export type { DocumentsList, DocumentsListDependencies, DocumentsListPlace } from "./list.ts";
export { DOC_STATES_KEY, parseDocStates, serializeDocStates, withDocFlag } from "./docstate.ts";
export { createDocumentsReader, currentOutlineIndex, highlightSearchTerms, openDocumentNotice } from "./reader.ts";
export { createDocumentsSearch } from "./search.ts";
export type { DocumentsSearch, DocumentsSearchDependencies, DocumentSearchHit } from "./search.ts";
export type { DocumentsReader, DocumentsReaderDependencies, OpenDocument } from "./reader.ts";
export { renderDocumentMarkdown, documentHeadingId } from "./render.ts";
export { createViewSelector, nextView, otherViews, VIEW_CARDS_SETTLE_MS, VIEW_LABELS, VIEW_ORDER } from "./selector.ts";
export type { DocViewName, ViewSelector, ViewSelectorDependencies } from "./selector.ts";
export type { DocumentMention, DocumentRow, DocumentsListAnswer } from "./types.ts";

import { createListening } from "../dom/index.ts";
import { createDocStatesStore } from "./docstate.ts";
import type { DocStateStorage } from "./docstate.ts";
import { createDocumentsList, FOLLOW_SLACK_PX } from "./list.ts";
import type { DocumentsListPlace } from "./list.ts";
import { createDocumentsReader, openDocumentNotice } from "./reader.ts";
import { createDocumentsSearch } from "./search.ts";
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
  /**
   * The device storage per-document state is kept in (`#1265`), read on use so
   * an owner made before the ports still works; memory alone without one.
   */
  storage?(): DocStateStorage;
  /** Close the hamburger menu, where the page wired its owner (`#1265`). */
  closeActionMenu?(): unknown;
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
  /** Whether the reader's outline pop-up is open (its `menu:outline` back level). */
  outlineOpen(): boolean;
  /** Toggle the reader's outline pop-up, as its button does. */
  toggleOutline(): void;
  /** Close the outline pop-up where open; say whether it was. */
  closeOutline(): boolean;
  /** Whether hidden documents are being shown dimmed (the hamburger toggle). */
  showHidden(): boolean;
  /** Show or stop showing hidden documents, as the hamburger item does. */
  setShowHidden(on: boolean): void;
  /** Per-document state landed from another device: reread and redraw both surfaces. */
  reloadDocStates(): void;
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
  /**
   * The list's place when a document opened from it (`#1264
   * documents-scroll-landings`): where Back to the list lands — the bottom
   * where it was parked there, the exact offset where the reader had
   * scrolled up. Null where no document was opened from the list.
   */
  let listPlace: DocumentsListPlace | null = null;

  /** The shared scroller's place now, or 0 where it cannot be read. */
  function scrollTopNow(): number {
    try {
      const top = deps.byId("scroll-area").scrollTop;
      return typeof top === "number" && Number.isFinite(top) ? top : 0;
    } catch {
      return 0;
    }
  }

  /** The list's place now, where the list is the pane on screen; else null. */
  function listPlaceNow(): DocumentsListPlace | null {
    try {
      if (deps.byId("pane-docs").hidden || deps.byId("docs-list-view").hidden) return null;
      const scroller = deps.byId("scroll-area");
      const top = typeof scroller.scrollTop === "number" && Number.isFinite(scroller.scrollTop) ? scroller.scrollTop : 0;
      const follow = top + scroller.clientHeight >= scroller.scrollHeight - FOLLOW_SLACK_PX;
      return { top, follow };
    } catch {
      return null;
    }
  }

  /** Take the recorded list place, forgetting it; null where none stands. */
  function takeListPlace(): DocumentsListPlace | undefined {
    const place = listPlace;
    listPlace = null;
    return place ?? undefined;
  }

  /** Put the shared scroller back at `top`, where it can be written. */
  function restoreScrollTop(top: number): void {
    try {
      deps.byId("scroll-area").scrollTop = top;
    } catch {
      // A host without the scroller keeps the reader where it opened.
    }
  }

  // One store for the list's cards and the reader's header star (`#1265`),
  // over the device storage the page hands in, read on use.
  const docStates = deps.storage
    ? createDocStatesStore({
        getItem: (key) => deps.storage?.().getItem(key) ?? null,
        setItem: (key, value) => deps.storage?.().setItem(key, value),
        removeItem: (key) => deps.storage?.().removeItem(key),
      })
    : createDocStatesStore(null);

  const reader = createDocumentsReader({
    api: deps.api,
    byId: deps.byId,
    readDocument: deps.readDocument,
    stopSpeaking: deps.stopSpeaking,
    star: {
      isStarred: (key) => list.isStarred(key),
      toggle: (key) => list.setStarred(key, !list.isStarred(key)),
    },
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
    onOutlineChange: () => deps.onLevelChange?.(),
  });
  const list = createDocumentsList({
    api: deps.api,
    byId: deps.byId,
    docStates,
    onOpen: (key) => {
      void openDocument(key);
    },
    onMentions: (row) => showMentions(row),
    onStatus: deps.onStatus,
  });
  const search = createDocumentsSearch({
    api: deps.api,
    byId: deps.byId,
    onOpen: (key, terms) => {
      void openDocument(key, terms);
    },
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
    // Back from a fetch: the rows redraw with their fetched state, landing
    // where the list was left (`#1264`), applied by the refresh after its
    // rows render however late they arrive.
    if (redraw) void list.refresh(takeListPlace()).catch(() => undefined);
  }

  /**
   * One step back: the previous document, reopened at its scroller place, or
   * the list at the chain's start. This is what the history closing one
   * `document` level does, so gesture and button walk the same chain.
   */
  function back(): void {
    if (readingNow && reader.closeOutline()) return;
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

  async function openDocument(key: string, highlightTerms?: readonly string[]): Promise<void> {
    const current = readingNow ? chain[chain.length - 1] : undefined;
    // A new chain entry is a first open of that document in this chain
    // (`#1264`): it lands at the document's top once drawn. Reopening the
    // document already open is not a first open and keeps its place.
    const firstOpen = !current || current.key !== key;
    if (current) {
      // The place a Back to this document returns to.
      current.scrollTop = scrollTopNow();
      // Reopening the document already open adds no step of its own.
      if (current.key !== key) chain.push({ key, scrollTop: 0 });
    } else {
      // Opening from the list records the list's place for the return
      // (`#1264`), before the shared scroller becomes the reader's.
      listPlace = listPlaceNow() ?? listPlace;
      chain = [{ key, scrollTop: 0 }];
    }
    deps.byId("docs-list-view").hidden = true;
    deps.byId("docs-reader").hidden = false;
    readingNow = true;
    deps.onLevelChange?.();
    let opened = false;
    try {
      await reader.open(key, highlightTerms);
      opened = true;
    } catch {
      // The reader already said what happened; stay on the reader so Back works.
    }
    // Land a first open at the top after the content renders, however late
    // it was fetched (`#1264`), through the view's existing restore surface.
    // A `#fragment` open scrolls on to its heading after this (the reader's
    // click path awaits this open first), and Back to a previous document
    // restores its own saved place the same way.
    // A search open lands at its match instead (`#1266 documents-search`).
    if (opened && firstOpen && highlightTerms === undefined) restoreScrollTop(0);
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
    // A counted fetch (`#1307 doc-fetch-visible-viewport`): the button reads
    // "Fetching done/total" as each document lands, then returns to "Fetch".
    const reportFetching = (
      run: (progress: (done: number, total: number) => unknown) => Promise<number>,
    ): void => {
      setBusy(true);
      run((doneCount, total) => {
        if (total > 0) button.textContent = `Fetching ${doneCount}/${total}`;
      }).then(
        (n) => deps.onStatus?.(`Fetched ${n} document${n === 1 ? "" : "s"}.`),
        (error: unknown) => deps.onStatus?.(error instanceof Error ? error.message : String(error)),
      ).finally(() => setBusy(false));
    };
    listening.listen(button, "click", () => {
      const open = menu.hidden;
      menu.hidden = !open;
      button.setAttribute("aria-expanded", String(open));
      // A Fetch press puts the since prompt away, fetching nothing (`#1308`).
      since.hidden = true;
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
        if (act === "visible") reportFetching((progress) => list.fetchVisible(progress));
        else if (act === "since") {
          // Ask since when where the owner is looking (`#1308`): the prompt
          // card hangs above the bar, and its first field takes the focus
          // (its value selected, ready to overtype).
          since.hidden = false;
          try {
            const days = deps.byId("docs-since-days") as HTMLInputElement;
            days.focus();
            if (typeof days.select === "function") days.select();
          } catch {
            // A host whose field cannot take the focus still shows the prompt.
          }
        }
        else if (act === "refresh") {
          // The button names what the count counts (`#1309`); the list says
          // the full scope in a status sentence as the run starts.
          setBusy(true, "Checking fetched…");
          list
            .refreshFetched((doneCount, total) => {
              if (total > 0) button.textContent = `Checking ${doneCount}/${total} fetched…`;
            })
            .then(
              ({ updated, current, skipped }) =>
                deps.onStatus?.(
                  skipped > 0 && updated === 0 && current === 0
                    ? `All ${skipped} fetched document${skipped === 1 ? " was" : "s were"} checked just now.`
                    : skipped > 0
                      ? `${updated} updated, ${current} current, ${skipped} checked just now.`
                      : `${updated} updated, ${current} current.`,
                ),
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
    // The hamburger's "Show hidden" (`#1265`): an item in #action-menu, drawn
    // only in this view by the bar, toggling the list's dimmed hidden cards.
    try {
      const showHiddenItem = deps.byId("docs-show-hidden");
      const drawShowHidden = (): void => {
        const on = list.showHidden();
        showHiddenItem.setAttribute("aria-pressed", String(on));
        const label = deps.byId("docs-show-hidden-label");
        if (label) label.textContent = on ? "Hide hidden" : "Show hidden";
      };
      drawShowHidden();
      listening.listen(showHiddenItem, "click", () => {
        list.setShowHidden(!list.showHidden());
        drawShowHidden();
        deps.closeActionMenu?.();
      });
    } catch {
      // A host without the hamburger item keeps the list as it was.
    }
    // Cancel puts the prompt away and fetches nothing (`#1308`).
    listening.listen(deps.byId("docs-since-cancel"), "click", () => {
      since.hidden = true;
    });
    listening.listen(deps.byId("docs-since-go"), "click", () => {
      const days = Number((deps.byId("docs-since-days") as HTMLInputElement).value) || 0;
      const hours = Number((deps.byId("docs-since-hours") as HTMLInputElement).value) || 0;
      since.hidden = true;
      reportFetching((progress) =>
        list.fetchSince(Date.now() - ((days * 24 + hours) * 3_600_000), progress));
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
      // chain forgotten, however the reader was last left. A recorded list
      // place (`#1264`) lands the switch's return there too; with none, the
      // refresh keeps the place the page restored for this view.
      toList(false);
      try {
        await list.refresh(takeListPlace());
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
    outlineOpen: () => (readingNow ? reader.outlineOpen() : false),
    toggleOutline(): void {
      if (readingNow) reader.toggleOutline();
    },
    closeOutline: (): boolean => (readingNow ? reader.closeOutline() : false),
    showHidden: () => list.showHidden(),
    setShowHidden(on: boolean): void {
      wire();
      list.setShowHidden(on);
      try {
        const item = deps.byId("docs-show-hidden");
        item.setAttribute("aria-pressed", String(on));
        const label = deps.byId("docs-show-hidden-label");
        if (label) label.textContent = on ? "Hide hidden" : "Show hidden";
      } catch {
        // No hamburger item in this host.
      }
    },
    reloadDocStates(): void {
      list.reloadStates();
      if (readingNow) reader.refreshStar();
    },
    unmount(): void {
      if (liveTimer !== 0) clearTimeout(liveTimer);
      liveTimer = 0;
      wired = false;
      listening.unmount();
      list.unmount();
      search.unmount();
      reader.unmount();
    },
  };
}

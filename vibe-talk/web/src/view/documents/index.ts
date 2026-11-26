// view/documents (`documents-view-client`): the Documents view — the list of
// documents linked in chat, and the full-screen reader for one of them. Its
// public interface; the page mounts it once and shows it as the `docs` view.

export { createDocumentsList, dedupeDocumentRows, documentIdentity, documentMatchesNameFilter } from "./list.ts";
export type { DocumentsList, DocumentsListDependencies, DocumentsListPlace } from "./list.ts";
export { DOC_STATES_KEY, parseDocStates, serializeDocStates, withDocFlag } from "./docstate.ts";
export { createDocumentsReader, currentOutlineIndex, highlightSearchTerms, openDocumentNotice } from "./reader.ts";
export { createDocumentsSearch } from "./search.ts";
export type { DocumentsSearch, DocumentsSearchDependencies, DocumentSearchHit } from "./search.ts";
export type { DocumentsReader, DocumentsReaderDependencies, OpenDocument } from "./reader.ts";
export { renderDocumentMarkdown, documentHeadingId } from "./render.ts";
export { listAnchorDelta, listAnchorOf, readerAnchorDelta, readerAnchorOf } from "./viewstate.ts";
export type { ListAnchor, ReaderAnchor } from "./viewstate.ts";
export { documentReadBlocks, setInternalLinkHostProvider, setInternalLinkHosts } from "./blocks.ts";
export type { DocumentReadBlock } from "./blocks.ts";
export { createViewSelector, nextView, otherViews, VIEW_CARDS_SETTLE_MS, VIEW_LABELS, VIEW_ORDER } from "./selector.ts";
export type { DocViewName, ViewSelector, ViewSelectorDependencies } from "./selector.ts";
export type { DocumentMention, DocumentRow, DocumentsListAnswer } from "./types.ts";

import { createListening } from "../dom/index.ts";
import { createDocStatesStore } from "./docstate.ts";
import type { DocStateStorage } from "./docstate.ts";
import { createDocumentsList, FOLLOW_SLACK_PX } from "./list.ts";
import type { DocumentsListPlace } from "./list.ts";
import { createDocumentsReader, openDocumentNotice } from "./reader.ts";
import { setInternalLinkHostProvider } from "./blocks.ts";
import { centrePopupOver } from "../dom/index.ts";
import { createDocumentsSearch } from "./search.ts";
import type { OpenDocument } from "./reader.ts";
import type { ListAnchor, ReaderAnchor } from "./viewstate.ts";

/** What the Documents view needs from the page. */
export interface DocumentsViewDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  /** Bring an element into view through the scroll owner (`#1251 scroll-single-owner`). */
  intoView(element: Element, block: string, inline?: string): void;
  /** Move the shared scroller by a measured delta, put it at a measured position, or take it to its foot, through the scroll owner (`#1251 scroll-single-owner`). */
  nudge(scroller: HTMLElement, delta: number): void;
  toPosition(scroller: HTMLElement, top: number): void;
  toFoot(scroller: HTMLElement): void;
  byId(id: string): HTMLElement;
  /** The page's root element, where the stylesheet reads list-or-reader (`#2283`). Unwired, unmarked. */
  root?(): HTMLElement;
  /** Read one document aloud through the page's shared read-aloud owner (`#1039`). */
  readDocument(doc: {
    key: string; title?: string; texts: readonly string[];
    kinds?: readonly string[];
    sections?: readonly number[];
    onReadingBlock?: (block: number | null, element?: number | null) => unknown;
    onReact?: (block: number, text: string) => unknown;
  }, from?: { part: number; block: number | null; piece?: number; offset?: number }): unknown;
  /** `#2006`: where a Documents React posts, as kept in Settings. */
  reactPolicy(): "mention" | "discussion";
  /** `#2006`: open the quick reply for a document React, posting through `send`. */
  openDocumentReply(quote: string, box: { top: number; bottom: number; left: number; right: number } | null, context: string, send: (text: string) => Promise<void>): unknown;
  /** Stop the reading of document `key`, where that document's is the one playing; no other. */
  stopSpeaking?(key: string): unknown;
  /**
   * The deployment's internal link hosts as client-config names them
   * (`#2007 vibe-talk-speech-short-links`), read on use for the speech rule a
   * device voice applies to a document read. Absent, no host is internal.
   */
  internalLinkHosts?(): readonly string[];
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
  /**
   * Start reading the open document aloud (`#2004`): the bottom-bar Read's
   * start while a document is open, through the reader's one start path.
   * Nothing where the list is up.
   */
  readAloud(): void;
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
  // The reader's speech rule reads the configured internal hosts on use
  // (`#2007`), so a client-config answer landing after construction still counts.
  if (deps.internalLinkHosts) setInternalLinkHostProvider(deps.internalLinkHosts);
  const listening = createListening();
  /**
   * `#2283 vibe-talk-root-has-rules`. List or reader, on the root beside the two views'
   * own `hidden` writes: the stylesheet keys the bar's Documents members off this one fact
   * instead of re-searching the whole document for the views on every toggle.
   */
  function markDocs(which: "list" | "reader"): void {
    const root = deps.root?.();
    if (root && root.getAttribute("data-docs") !== which) root.setAttribute("data-docs", which);
  }
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
  let chain: { key: string; scrollTop: number; anchor: ReaderAnchor | null }[] = [];
  /**
   * The list's place when a document opened from it (`#1264
   * documents-scroll-landings`): where Back to the list lands — the bottom
   * where it was parked there, the exact offset where the reader had
   * scrolled up. Null where no document was opened from the list.
   */
  let listPlace: DocumentsListPlace | null = null;
  /**
   * The documents view state (`#2005 vibe-talk-documents-keep-place`): the
   * list's anchor (its top card and every card's offset, kept beside the raw
   * `listPlace` above) and whether the view was ever entered — the first
   * entry is fresh (the list at its bottom), every later bare showing is a
   * return that restores the anchors. Written by the scroll capture below
   * and at each open; read only by a return.
   */
  let listAnchor: ListAnchor | null = null;
  let entered = false;
  /**
   * A list landing is in flight (a Back or return refresh whose rows have
   * not landed): scrolls in that window describe the transition — the shared
   * scroller still holding the reader's offset over a half-swapped pane —
   * not the list, so the scroll capture must not record them (`#2005`).
   */
  let listLanding = false;

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
      deps.toPosition(deps.byId("scroll-area"), top);
    } catch {
      // A host without the scroller keeps the reader where it opened.
    }
  }

  /**
   * Record where the reader stands in the open document (`#2005`): the raw
   * offset, and its paragraph anchor where one can be measured (the anchor
   * already recorded stands where this look measures nothing).
   */
  function captureReaderPlace(): void {
    const topEntry = readingNow ? chain[chain.length - 1] : undefined;
    if (!topEntry) return;
    topEntry.scrollTop = scrollTopNow();
    topEntry.anchor = reader.captureAnchor() ?? topEntry.anchor;
  }

  /**
   * Record where the list stands (`#2005`): its anchor and its raw place,
   * each kept where this look finds nothing (off screen, unmeasured).
   */
  function captureListPlace(): void {
    if (readingNow || listLanding) return;
    const anchor = list.capturePlace();
    if (anchor !== null) listAnchor = anchor;
    const raw = listPlaceNow();
    if (raw !== null) listPlace = raw;
  }

  /** Land the open document back on its recorded place: raw, then anchored. */
  function restoreReaderPlace(entry: { scrollTop: number; anchor: ReaderAnchor | null }): void {
    restoreScrollTop(entry.scrollTop);
    const delta = reader.anchorDelta(entry.anchor);
    if (delta !== null && delta !== 0) {
      try {
        deps.nudge(deps.byId("scroll-area"), delta);
      } catch {
        // A host without the scroller keeps the raw place just restored.
      }
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
    intoView: deps.intoView,
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
    onDocumentReact: (react) => reactToDocument(react),
  });
  const list = createDocumentsList({
    api: deps.api,
    byId: deps.byId,
    nudge: deps.nudge,
    toPosition: deps.toPosition,
    toFoot: deps.toFoot,
    docStates,
    onOpen: (key) => {
      void openDocument(key);
    },
    onMentions: (row) => showMentions(row),
    onStatus: deps.onStatus,
    onStarredOnlyChange: (on) => {
      try {
        const toggle = deps.byId("docs-starred-toggle");
        toggle.setAttribute("aria-pressed", String(on));
        toggle.textContent = on ? "★" : "☆";
      } catch {
        // A host without the bar toggle keeps the filter as it was.
      }
    },
    onDownloadedOnlyChange: (on) => {
      try {
        deps.byId("docs-downloaded-toggle").setAttribute("aria-pressed", String(on));
      } catch {
        // A host without the bar toggle keeps the filter as it was.
      }
    },
  });
  const search = createDocumentsSearch({
    api: deps.api,
    byId: deps.byId,
    onOpen: (key, terms) => {
      void openDocument(key, terms);
    },
    // The default search is the list's local name filter (`#1767`); only the
    // "Search text" checkbox takes the field to the server.
    onFilter: (query) => list.setNameFilter(query),
    onStatus: deps.onStatus,
  });

  // ---- React and the discussion jumps (`#2006`) ---------------------------------------------
  //
  // The open document's row, held for its mentions: the list drew it, and
  // a document opened by a link the list has not drawn has none, which
  // both jumps and React say so for rather than guessing a channel.
  let currentRow: import("./types.ts").DocumentRow | null = null;

  /** The most recent mention of the open document, or null where none is held. */
  function lastMention(): import("./types.ts").DocumentMention | null {
    const mentions = currentRow?.mentions ?? [];
    let best: import("./types.ts").DocumentMention | null = null;
    for (const mention of mentions) {
      if (best === null || (mention.posted_ms ?? -1) >= (best.posted_ms ?? -1)) best = mention;
    }
    return best;
  }

  /** The name a discussion root calls this document: its title, else its file name. */
  function discussionName(): string {
    const doc = reader.current();
    return (currentRow?.title || currentRow?.file_name || doc?.title || "document").trim() || "document";
  }

  /** The discussion root's exact words (`#2006` policy B). */
  function discussionRootText(): string {
    return `Discussion of document ${discussionName()}`;
  }

  /** A fresh idempotency key for one React post. */
  function reactKey(part: string): string {
    return `doc-react-${part}-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e9).toString(36)}`;
  }

  /** The discussion root's message id in `channel`, or null where none stands (first page only). */
  async function findDiscussionRoot(channel: string): Promise<string | null> {
    const wanted = discussionRootText();
    const answer = await deps.api(
      `/api/v1/channels/${encodeURIComponent(channel)}/page?limit=100`) as {
        messages?: Array<{ id?: unknown; content?: unknown; text?: unknown; thread_id?: unknown }> } | null;
    for (const message of answer?.messages ?? []) {
      const words = typeof message.content === "string" ? message.content
        : typeof message.text === "string" ? message.text : "";
      if (words.trim() !== wanted) continue;
      if (message.thread_id !== null && message.thread_id !== undefined && String(message.thread_id) !== String(message.id)) continue;
      return typeof message.id === "string" ? message.id : String(message.id ?? "") || null;
    }
    return null;
  }

  /** Post as the owner through the one reply route. */
  async function postReply(channel: string, body: Record<string, unknown>): Promise<{ posted?: { id?: unknown } } | null> {
    return await deps.api(`/api/v1/channels/${encodeURIComponent(channel)}/reply`, {
      method: "POST", body,
    }) as { posted?: { id?: unknown } } | null;
  }

  /** React pressed on the open document's read: open its reply, resolved at Send. */
  function reactToDocument(react: { key: string; block: number; text: string; box: { top: number; bottom: number; left: number; right: number } | null }): void {
    const mention = lastMention();
    if (mention === null) {
      deps.onStatus?.("No message has mentioned this document yet, so there is nowhere for a reply to go.");
      return;
    }
    const policy = deps.reactPolicy();
    const name = discussionName();
    const context = policy === "discussion"
      ? `Replying in 'Discussion of document ${name}' — will start 'Discussion of document ${name}' in the channel that last mentioned it if it is not there yet`
      : `Replying in the thread of the last mention of this document${mention.author ? `, by ${mention.author}` : ""}`;
    deps.openDocumentReply(react.text, react.box, context, async (text) => {
      if (policy === "discussion") {
        let root = await findDiscussionRoot(mention.channel_id);
        if (root === null) {
          const rootText = discussionRootText();
          let hash = 0;
          for (const ch of react.key) hash = (hash * 31 + ch.charCodeAt(0)) | 0;
          await postReply(mention.channel_id, {
            text: rootText, idempotency_key: `doc-discussion-${(hash >>> 0).toString(36)}`,
          });
          root = await findDiscussionRoot(mention.channel_id);
          if (root === null) throw new Error("The discussion could not be started; try again.");
        }
        await postReply(mention.channel_id, {
          text, reply_to: root, thread_id: root, idempotency_key: reactKey("reply"),
        });
        return;
      }
      await postReply(mention.channel_id, {
        text,
        reply_to: mention.message_id,
        thread_id: mention.thread_id ?? mention.message_id,
        idempotency_key: reactKey("reply"),
      });
    });
  }

  /** Jump to last mention: the mentioning message, through the page's jump. */
  function jumpToLastMention(): void {
    const mention = lastMention();
    if (mention === null) {
      deps.onStatus?.("This document has not been mentioned yet.");
      return;
    }
    deps.onJumpToMention?.(mention);
  }

  /** Jump to discussion: policy B's root, or under A the mention's thread. */
  async function jumpToDiscussion(): Promise<void> {
    const mention = lastMention();
    if (mention === null) {
      deps.onStatus?.("This document has not been mentioned yet.");
      return;
    }
    if (deps.reactPolicy() === "discussion") {
      const root = await findDiscussionRoot(mention.channel_id);
      if (root === null) {
        deps.onStatus?.("No discussion yet — use Mention for the last mention.");
        return;
      }
      deps.onJumpToMention?.({
        channel_id: mention.channel_id, message_id: root, thread_id: root,
        posted_ms: null, author: "", label: null,
      });
      return;
    }
    deps.onJumpToMention?.(mention);
  }

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

  /** `#1545`: the hamburger's Show hidden belongs to the list alone. */
  function syncShowHiddenOffer(): void {
    try {
      deps.byId("docs-show-hidden").hidden = readingNow;
    } catch {
      // A host without the hamburger item keeps the list as it was.
    }
  }

  /** To the list, the chain forgotten: a view switch here, never a Back step. */
  function toList(redraw = true): void {
    const wasReading = readingNow;
    readingNow = false;
    syncShowHiddenOffer();
    chain = [];
    if (wasReading) deps.onLevelChange?.();
    reader.close();
    currentRow = null;
    try {
      deps.byId("doc-jumps").hidden = true;
    } catch {
      // A host without the jump buttons keeps the list as it was.
    }
    deps.byId("docs-reader").hidden = true;
    deps.byId("docs-list-view").hidden = false;
    markDocs("list");
    deps.byId("docs-fetch").hidden = false;
    try {
      deps.byId("docs-filter-bar").hidden = false;
      deps.byId("docs-starred-toggle").hidden = false;
      deps.byId("docs-downloaded-toggle").hidden = false;
    } catch {
      // A host without the fixed bar keeps the list as it was.
    }
    try {
      deps.byId("bar-pack").appendChild(deps.byId("audio-source"));
    } catch {
      // A host without the bar pack keeps the toggle where it is.
    }
    // Back from a fetch: the rows redraw with their fetched state, landing
    // where the list was left (`#1264`), applied by the refresh after its
    // rows render however late they arrive; the anchor recorded when the
    // document opened then puts the top card exactly back (`#2005`), where
    // it can be measured. The landing window is guarded: scrolls while the
    // rows are in flight are the transition, not the list, and must not
    // overwrite the anchor being landed on.
    if (redraw) {
      listLanding = true;
      const place = listAnchor;
      void list.refresh(takeListPlace())
        .then(() => {
          listLanding = false;
          if (!readingNow) list.restorePlace(place);
        })
        .catch(() => {
          listLanding = false;
        });
    }
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
    const previous = chain[chain.length - 1] as { key: string; scrollTop: number; anchor: ReaderAnchor | null };
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
      if (readingNow && chain[chain.length - 1] === previous) restoreReaderPlace(previous);
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
      captureReaderPlace();
      // Reopening the document already open adds no step of its own.
      if (current.key !== key) chain.push({ key, scrollTop: 0, anchor: null });
    } else {
      // Opening from the list records the list's place for the return
      // (`#1264`, anchored by `#2005`), before the shared scroller becomes
      // the reader's.
      captureListPlace();
      chain = [{ key, scrollTop: 0, anchor: null }];
    }
    currentRow = list.rowFor(key);
    deps.byId("docs-list-view").hidden = true;
    deps.byId("docs-reader").hidden = false;
    markDocs("reader");
    // The reader's jumps stand with it (`#2006`).
    try {
      deps.byId("doc-jumps").hidden = false;
    } catch {
      // A host without the jump buttons keeps the reader as it was.
    }
    // Fetch is the list's button and Outline the reader's (`#1544`): never both.
    deps.byId("docs-fetch").hidden = true;
    try {
      deps.byId("docs-filter-bar").hidden = true;
      deps.byId("docs-starred-toggle").hidden = true;
      deps.byId("docs-downloaded-toggle").hidden = true;
    } catch {
      // A host without the fixed bar keeps the list as it was.
    }
    // The audio toggle joins the reader's controls while a document is open
    // (`#1544`); the bar takes it back for the list (in toList, below).
    try {
      deps.byId("control-pane").insertBefore(deps.byId("audio-source"), deps.byId("read-speed"));
    } catch {
      // A host without the reader row keeps the bar's own toggle.
    }
    readingNow = true;
    syncShowHiddenOffer();
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
    // Keep the view state's places current while Documents is up (`#2005`):
    // a switch away hands the shared scroller to another view, so both
    // anchors must already stand recorded when it happens — every scroll
    // refreshes them. Synchronously, asking for no frame of its own: the
    // page's energy profile allows a burst of scrolls one frame in total
    // (`#226`), and a second requester would spend it.
    try {
      listening.listen(deps.byId("scroll-area"), "scroll", () => {
        try {
          if (deps.byId("pane-docs").hidden) return;
        } catch {
          return;
        }
        if (readingNow) captureReaderPlace();
        else captureListPlace();
      });
    } catch {
      // A host without the scroller keeps the places taken at each open.
    }

    // The reader's jumps (`#2006`), where the host drew them.
    try {
      listening.listen(deps.byId("doc-jump-mention"), "click", () => jumpToLastMention());
      listening.listen(deps.byId("doc-jump-discussion"), "click", () => { void jumpToDiscussion(); });
    } catch {
      // A host without the jump buttons keeps the view as it was.
    }
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
    // The bottom bar's Starred toggle (`#1909`, beside the hamburger since
    // `#2001`): the list owns the state; its render syncs this button's
    // pressed dress through the callback.
    try {
      const starredToggle = deps.byId("docs-starred-toggle");
      starredToggle.setAttribute("aria-pressed", String(list.starredOnly()));
      starredToggle.textContent = list.starredOnly() ? "★" : "☆";
      listening.listen(starredToggle, "click", () => {
        list.setStarredOnly(!list.starredOnly());
      });
    } catch {
      // A host without the bar toggle keeps the filter as it was.
    }
    // The Downloaded toggle (`#2001`), left of the star: the row's download
    // glyph, filtering to fetched documents only.
    try {
      const downloadedToggle = deps.byId("docs-downloaded-toggle");
      downloadedToggle.setAttribute("aria-pressed", String(list.downloadedOnly()));
      listening.listen(downloadedToggle, "click", () => {
        list.setDownloadedOnly(!list.downloadedOnly());
      });
    } catch {
      // A host without the bar toggle keeps the filter as it was.
    }
    listening.listen(button, "click", () => {
      const open = menu.hidden;
      menu.hidden = !open;
      if (open) centrePopupOver(menu, button);
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
          centrePopupOver(since, button);
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
        entered = true;
        await openDocument(key);
        return;
      }
      if (!entered) {
        // The first entry ever is FRESH (`#2002` split: fresh lands at the
        // bottom, a return restores): the list as it first stands, however
        // the reader was last left in a prior page life — none stands here.
        entered = true;
        toList(false);
        try {
          await list.refresh(takeListPlace());
        } catch (error) {
          deps.onStatus?.(error instanceof Error ? error.message : "Could not load the documents.");
        }
        return;
      }
      // A RETURN (`#2005 vibe-talk-documents-keep-place`): exactly the prior
      // screen. An open document was never closed — its content, outline and
      // chain stand — so only its chrome and its place are put back; the
      // list behind it is not redrawn under it. With no document open, the
      // list refreshes and lands where it was left: the recorded place
      // first (as Back lands), refined by its anchor card.
      if (readingNow) {
        deps.byId("docs-list-view").hidden = true;
        deps.byId("docs-reader").hidden = false;
        markDocs("reader");
        deps.byId("docs-fetch").hidden = true;
        try {
          deps.byId("docs-filter-bar").hidden = true;
          deps.byId("docs-starred-toggle").hidden = true;
        } catch {
          // A host without the fixed bar keeps the reader as it was.
        }
        try {
          deps.byId("control-pane").insertBefore(deps.byId("audio-source"), deps.byId("read-speed"));
        } catch {
          // A host without the reader row keeps the bar's own toggle.
        }
        syncShowHiddenOffer();
        const topEntry = chain[chain.length - 1];
        if (topEntry) restoreReaderPlace(topEntry);
        return;
      }
      listLanding = true;
      const place = listAnchor;
      try {
        await list.refresh(takeListPlace());
      } catch (error) {
        deps.onStatus?.(error instanceof Error ? error.message : "Could not load the documents.");
      } finally {
        listLanding = false;
      }
      list.restorePlace(place);
    },
    showList: back,
    back,
    depth: () => (readingNow ? Math.max(1, chain.length) : 0),
    reading: () => readingNow,
    readAloud(): void {
      if (readingNow) reader.readAloud();
    },
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

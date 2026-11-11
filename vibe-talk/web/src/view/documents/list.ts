// The Documents list (`#1022 documents-view-client`, batch A): one bordered card
// per document the fold knows, forward chronological like the message views
// (`#1079 documents-chronological`) — oldest at the top, newest at the bottom,
// opening at the bottom. Older documents page in upward; a drag up parked at
// the bottom refreshes, the message views' gesture. Unfetched cards are
// dimmed; fetched ones are full saturation, with when they were fetched said
// quietly. A card names the document only: its real title once fetched, its
// file name before that. The kind label is the small link out to the source.
// All words from the server are untrusted, so every visible string goes
// through `textContent`.

import { createListening } from "../dom/index.ts";
import { createDocStatesStore, docStateOf } from "./docstate.ts";
import type { DocStatesStore } from "./docstate.ts";
import type { DocumentRow, DocumentsListAnswer } from "./types.ts";

export interface DocumentsListDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  onOpen(key: string): unknown;
  onMentions?(row: DocumentRow): unknown;
  onStatus?(text: string): unknown;
  /** Per-document starred/hidden state (`#1265 documents-star-hide`); memory alone without one. */
  docStates?: DocStatesStore;
}

/** A list place a return lands on (`#1264 documents-scroll-landings`). */
export interface DocumentsListPlace {
  top: number;
  follow: boolean;
}

/** The Documents list's commands. */
export interface DocumentsList {
  /**
   * Redraw from the server. With `place`, land there after the rows render
   * (the bottom where `follow`, else exactly `top`); without it, keep the
   * scroller's current place by the same rule.
   */
  refresh(place?: DocumentsListPlace): Promise<void>;
  /** Fetch the unfetched rows whose cards are on screen, newest first; how many arrived. */
  fetchVisible(progress?: (done: number, total: number) => unknown): Promise<number>;
  /** Fetch unfetched rows last mentioned at or after `cutoffMs`. */
  fetchSince(cutoffMs: number, progress?: (done: number, total: number) => unknown): Promise<number>;
  /** Fetch the most recent unfetched documents, then redraw. */
  fetchRecent(): Promise<number>;
  /** Check fetched rows for newer versions, updating them; progress by key. */
  refreshFetched(progress?: (done: number, total: number) => unknown): Promise<{ updated: number; current: number }>;
  /** Drop every cached copy and redraw; how many were held. */
  clearCached(): Promise<number>;
  /** Whether hidden documents are being shown (dimmed, with Unhide). */
  showHidden(): boolean;
  /** Show or stop showing hidden documents; the hamburger's toggle (`#1265`). */
  setShowHidden(on: boolean): void;
  /** Star or unstar one document, as its card's and the reader's star do. */
  setStarred(key: string, on: boolean): void;
  /** Whether one document is starred. */
  isStarred(key: string): boolean;
  /** Hide one document, offering Undo as a swipe does; unhide with `undoHide`. */
  hide(key: string): void;
  /** Take back the hide the Undo chip currently offers; say whether it did. */
  undoHide(): boolean;
  /** State from another device landed: reread it and redraw. */
  reloadStates(): void;
  unmount(): void;
}

const LIST_ID = "documents-list";
const LIST_PATH = "/api/v1/documents?limit=100";
const FETCH_PATH = "/api/v1/documents/fetch";
const FETCH_RECENT_PATH = "/api/v1/documents/fetch-recent";
const FRESHNESS_PATH = (key: string) => `/api/v1/documents/${encodeURIComponent(key)}/freshness`;
const FETCH_RECENT_LIMIT = 10;

/** How many documents a bulk fetch holds in flight at once (`#1307 doc-fetch-visible-viewport`). */
const FETCH_CONCURRENCY = 4;

/** One fetched entry of `POST /documents/fetch`'s answer, as far as patching a row reads it. */
interface FetchedEntryPatch {
  title: string | null;
  version: string | null;
  fetched_ms: number | null;
}

/** The patch `answer` carries for `key`, or null where it carries none readable. */
function fetchedPatchOf(answer: unknown, key: string): FetchedEntryPatch | null {
  const fetched = (answer as { fetched?: unknown } | null)?.fetched;
  if (!Array.isArray(fetched)) return null;
  for (const entry of fetched) {
    const one = entry as { key?: unknown; title?: unknown; version?: unknown; fetched_ms?: unknown } | null;
    if (!one || one.key !== key) continue;
    return {
      title: typeof one.title === "string" ? one.title : null,
      version: typeof one.version === "string" ? one.version : null,
      fetched_ms: typeof one.fetched_ms === "number" && Number.isFinite(one.fetched_ms) ? one.fetched_ms : null,
    };
  }
  return null;
}

/** The type-filter buckets of `#1176 doc-kind-buckets`: gists sit with pastes. */
export type DocumentBucket = "markdown" | "pastes" | "issues" | "other";

/** Which bucket a wire kind belongs to. */
export function bucketOfKind(kind: string): DocumentBucket {
  if (kind === "github_issue") return "issues";
  if (kind === "paste" || kind === "github_gist") return "pastes";
  if (kind === "github_markdown") return "markdown";
  return "other";
}

const BUCKET_BUTTONS: readonly { bucket: DocumentBucket | "all"; label: string }[] = [
  { bucket: "all", label: "All" },
  { bucket: "markdown", label: "Markdown" },
  { bucket: "pastes", label: "Pastes" },
  { bucket: "issues", label: "Issues" },
  { bucket: "other", label: "Other files" },
];

const ICON_PATHS: Record<string, string[]> = {
  issue: ["M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z", "M12 8h.01", "M11 12h1v4h1"],
  github: [
    "M6 3v12",
    "M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
    "M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
    "M18 9a9 9 0 0 1-9 9",
  ],
  paste: ["M8 2h8v4H8z", "M8 4H6v18h12V4h-2", "M9 12h6M9 16h6"],
  file: ["M6 2h8l4 4v16H6z", "M14 2v4h4"],
};

function iconFor(kind: string): SVGSVGElement {
  // The SVG namespace from any icon the page already draws (no address is named here).
  const anySvg = typeof document.querySelector === "function" ? document.querySelector("svg") : null;
  const ns = anySvg ? anySvg.namespaceURI : "";
  const svg = (ns ? document.createElementNS(ns, "svg") : document.createElement("svg")) as unknown as SVGSVGElement;
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "doc-icon-svg");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  const paths =
    kind === "github_issue"
      ? ICON_PATHS.issue
      : kind === "paste" || kind === "github_gist"
        ? ICON_PATHS.paste
        : kind.startsWith("github")
          ? ICON_PATHS.github
          : ICON_PATHS.file;
  for (const d of paths ?? []) {
    const path = (ns ? document.createElementNS(ns, "path") : document.createElement("path")) as unknown as SVGPathElement;
    path.setAttribute("d", d);
    svg.appendChild(path);
  }
  return svg;
}

function fetchedAgo(ms: number | null, now: number): string {
  if (ms === null) return "";
  const seconds = Math.max(0, Math.floor((now - ms) / 1000));
  if (seconds < 60) return "fetched just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `fetched ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `fetched ${hours}h ago`;
  return `fetched ${Math.floor(hours / 24)}d ago`;
}

function mentionedAgo(ms: number | null, now: number): string {
  if (ms === null) return "";
  const seconds = Math.max(0, Math.floor((now - ms) / 1000));
  if (seconds < 60) return "mentioned just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `mentioned ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `mentioned ${hours}h ago`;
  return `mentioned ${Math.floor(hours / 24)}d ago`;
}

/** The name a card shows: the real title, unless it reads as message prose. */
function displayTitle(row: DocumentRow): string {
  if (!row.fetched) return row.file_name;
  const title = row.title.trim();
  if (title === "" || title.length > 90 || title.includes("\n")) return row.file_name;
  return title;
}

/** Oldest mention first, newest at the bottom, as the message views run. */
function byOldest(a: DocumentRow, b: DocumentRow): number {
  return (a.last_mentioned_ms ?? 0) - (b.last_mentioned_ms ?? 0);
}

/**
 * The document a row names, ignoring which address named it (`#1263
 * documents-list-cards`): a GitHub file linked at two refs (or by its blob
 * and raw addresses, which mint one key) is one document, so its identity
 * is owner/repo and path, never the ref. Other kinds are their key.
 */
export function documentIdentity(row: DocumentRow): string {
  if (row.key.startsWith("github:")) {
    const rest = row.key.slice("github:".length);
    const slash = rest.indexOf("/");
    const at = rest.indexOf("@", slash);
    const colon = rest.indexOf(":", at);
    if (slash > 0 && at > slash && colon > at) {
      const owner = rest.slice(0, slash).toLowerCase();
      const repo = rest.slice(slash + 1, at).toLowerCase();
      return `github:${owner}/${repo}:${rest.slice(colon + 1)}`;
    }
  }
  return row.key;
}

/**
 * One row per document (`#1263 documents-list-cards`): exact-key repeats
 * and same-document rows (two addresses for one file) collapse into one
 * entry. The fetched copy, else the newest-mentioned, names the entry;
 * mentions union by mentioning message, and the entry is fetched if any
 * of its copies is.
 */
export function dedupeDocumentRows(rows: readonly DocumentRow[]): DocumentRow[] {
  const groups = new Map<string, DocumentRow[]>();
  for (const row of rows) {
    const identity = documentIdentity(row);
    const group = groups.get(identity);
    if (group) group.push(row);
    else groups.set(identity, [row]);
  }
  const out: DocumentRow[] = [];
  for (const group of groups.values()) {
    if (group.length === 1) {
      out.push(group[0]!);
      continue;
    }
    const ranked = [...group].sort((a, b) => {
      if (a.fetched !== b.fetched) return a.fetched ? -1 : 1;
      return (b.last_mentioned_ms ?? 0) - (a.last_mentioned_ms ?? 0);
    });
    const rep = ranked[0]!;
    const byMessage = new Map<string, DocumentRow["mentions"][number]>();
    for (const row of group) {
      for (const mention of row.mentions ?? []) {
        byMessage.set(`${mention.channel_id}\u001f${mention.message_id}`, mention);
      }
    }
    const mentions = [...byMessage.values()].sort(
      (a, b) => (a.posted_ms ?? 0) - (b.posted_ms ?? 0),
    );
    const mentioned = group.flatMap((row) => row.mentions ?? []);
    const times = (pick: (row: DocumentRow) => number | null) =>
      group.map(pick).filter((ms): ms is number => ms !== null);
    const firsts = times((row) => row.first_mentioned_ms);
    const lasts = times((row) => row.last_mentioned_ms);
    const fetcheds = times((row) => row.fetched_ms);
    out.push({
      ...rep,
      fetched: group.some((row) => row.fetched),
      fetched_ms: fetcheds.length ? Math.max(...fetcheds) : rep.fetched_ms,
      mentions,
      mention_count: mentioned.length > 0 ? mentions.length : group.reduce((sum, row) => sum + row.mention_count, 0),
      first_mentioned_ms: firsts.length ? Math.min(...firsts) : rep.first_mentioned_ms,
      last_mentioned_ms: lasts.length ? Math.max(...lasts) : rep.last_mentioned_ms,
    });
  }
  return out;
}

/** How far past the bottom a drag up must travel to refresh, in px: the channel pull's arm. */
const REFRESH_PULL_PX = 64;
/** How close to the true bottom counts as parked there for the pull, in px (the channel's slack). */
const PULL_EDGE_SLACK_PX = 2;
/** How close to the bottom counts as following the newest across a refresh, in px. */
export const FOLLOW_SLACK_PX = 24;
/** How near the top scrolling pages older documents in, in px. */
const OLDER_TRIGGER_PX = 500;

/** How far a card swipe goes before letting go hides it, in px: the message rows' commit. */
const HIDE_SWIPE_COMMIT_PX = 90;
/** How far a finger moves before a card press is a swipe or a scroll, in px: the rows' start. */
const HIDE_SWIPE_START_PX = 12;
/** How long the Undo chip offers to put a hidden document back, in ms: the Done Undo's window. */
const HIDE_UNDO_MS = 10_000;

export function createDocumentsList(deps: DocumentsListDependencies): DocumentsList {
  const listening = createListening();
  const busy = new Set<string>();
  const errors = new Map<string, string>();
  const states: DocStatesStore = deps.docStates ?? createDocStatesStore(null);
  let rowsNow: readonly DocumentRow[] = [];
  // The type filter's state lives with the list's other view state (no new
  // persistence key): `#1176 doc-kind-buckets`.
  let bucketFilter: DocumentBucket | "all" = "all";
  // The Starred chip (`#1265`): beside the buckets, and a row must pass both.
  let starredOnly = false;
  // Hidden documents shown dimmed (the hamburger's "Show hidden"); this device only.
  let showingHidden = false;
  // The hide the Undo chip offers to take back, and its timer.
  let undoOffer: { key: string; timer: ReturnType<typeof setTimeout> } | null = null;
  // A committed swipe's click, claimed so the card does not also open.
  let swipeClaimedClick = false;

  function visibleRows(): readonly DocumentRow[] {
    const held = states.states();
    return rowsNow.filter((row) => {
      if (bucketFilter !== "all" && bucketOfKind(row.kind) !== bucketFilter) return false;
      const state = docStateOf(held, row.key);
      if (state.hidden && !showingHidden) return false;
      if (starredOnly && !state.starred) return false;
      return true;
    });
  }

  /** The vertical span `element` reports, or null where it reports none readable. */
  function rectOf(element: unknown): { top: number; bottom: number } | null {
    try {
      const host = element as { getBoundingClientRect?: () => { top?: unknown; bottom?: unknown } } | null | undefined;
      if (!host || typeof host.getBoundingClientRect !== "function") return null;
      const rect = host.getBoundingClientRect();
      if (!rect || typeof rect.top !== "number" || typeof rect.bottom !== "number") return null;
      if (!Number.isFinite(rect.top) || !Number.isFinite(rect.bottom)) return null;
      return { top: rect.top, bottom: rect.bottom };
    } catch {
      return null;
    }
  }

  /**
   * The band of the shared scroller the reader can see, in viewport coordinates: the
   * scroller's own span where it can be measured, else the viewport's, else null.
   */
  function visibleBand(): { top: number; bottom: number } | null {
    const scroller = scrollerOrNull();
    if (scroller) {
      const rect = rectOf(scroller);
      if (rect && rect.bottom > rect.top) return rect;
    }
    const height = (globalThis as { innerHeight?: unknown }).innerHeight;
    if (typeof height === "number" && Number.isFinite(height) && height > 0) {
      return { top: 0, bottom: height };
    }
    return null;
  }

  /** The rendered `.doc-card` elements, each with the document key it names. */
  function cardElements(): readonly { key: string; element: unknown }[] {
    let list: HTMLElement | null = null;
    try {
      list = deps.byId(LIST_ID) ?? null;
    } catch {
      return [];
    }
    if (!list) return [];
    const keyOf = (element: unknown): string | null => {
      const host = element as { getAttribute?: (name: string) => unknown } | null | undefined;
      if (!host || typeof host.getAttribute !== "function") return null;
      const key = host.getAttribute("data-key");
      return typeof key === "string" && key !== "" ? key : null;
    };
    const host = list as unknown as {
      querySelectorAll?: (selector: string) => Iterable<unknown>;
      children?: ArrayLike<unknown>;
    };
    if (typeof host.querySelectorAll === "function") {
      try {
        const found = [...host.querySelectorAll(".doc-card")]
          .map((element) => ({ key: keyOf(element), element }))
          .filter((card): card is { key: string; element: unknown } => card.key !== null);
        if (found.length > 0) return found;
      } catch {
        // Fall through to the children below.
      }
    }
    const kids = host.children;
    if (!kids) return [];
    const found: { key: string; element: unknown }[] = [];
    for (const kid of Array.from(kids)) {
      const className = String((kid as { className?: unknown } | null)?.className ?? "");
      if (!className.split(/\s+/).includes("doc-card")) continue;
      const key = keyOf(kid);
      if (key !== null) found.push({ key, element: kid });
    }
    return found;
  }

  /**
   * The unfetched rows "Fetch visible" reaches (`#1307 doc-fetch-visible-viewport`):
   * drawn, non-busy rows whose rendered cards intersect the scroller's visible band.
   * Degrade rule: where no drawn card reports usable geometry — no
   * `getBoundingClientRect`, or all-zero rects, as the fake-DOM tests draw them — the
   * screen cannot scope the fetch, so every drawn unfetched row counts as on screen
   * rather than none of them fetching. The same holds where no band can be measured.
   */
  function rowsOnScreen(): readonly DocumentRow[] {
    const drawn = visibleRows().filter((row) => !row.fetched && !busy.has(row.key));
    if (drawn.length === 0) return drawn;
    const cards = cardElements();
    if (cards.length === 0) return drawn;
    const rects = new Map<string, { top: number; bottom: number } | null>();
    for (const card of cards) rects.set(card.key, rectOf(card.element));
    const usable = (
      rect: { top: number; bottom: number } | null | undefined,
    ): rect is { top: number; bottom: number } =>
      rect !== null && rect !== undefined && !(rect.top === 0 && rect.bottom === 0);
    if (!drawn.some((row) => usable(rects.get(row.key)))) return drawn;
    const band = visibleBand();
    if (!band) return drawn;
    return drawn.filter((row) => {
      const rect = rects.get(row.key);
      return usable(rect) && rect.bottom > band.top && rect.top < band.bottom;
    });
  }

  function endUndoOffer(): void {
    if (undoOffer) clearTimeout(undoOffer.timer);
    undoOffer = null;
  }

  function hideRow(row: DocumentRow): void {
    states.setHidden(row.key, true);
    endUndoOffer();
    const offer = { key: row.key, timer: setTimeout(() => { undoOffer = null; render(rowsNow); }, HIDE_UNDO_MS) };
    undoOffer = offer;
    deps.onStatus?.(`Hid ${row.file_name}.`);
    render(rowsNow);
  }

  function cardFor(row: DocumentRow): HTMLElement {
    const isBusy = busy.has(row.key);
    const state = docStateOf(states.states(), row.key);
    const card = document.createElement("div");
    card.className = row.fetched ? "doc-card" : "doc-card doc-unfetched";
    if (isBusy) card.className += " doc-busy";
    if (state.hidden) card.className += " doc-hidden";
    if (state.starred) card.className += " doc-starred";
    card.setAttribute("role", "button");
    card.setAttribute("tabindex", "0");
    card.setAttribute("data-key", row.key);
    if (isBusy) card.setAttribute("aria-busy", "true");

    const icon = document.createElement("span");
    icon.className = "doc-icon";
    icon.appendChild(iconFor(row.kind));
    card.appendChild(icon);

    const texts = document.createElement("span");
    texts.className = "doc-texts";
    const title = document.createElement("span");
    title.className = "doc-title";
    title.textContent =
      row.kind === "github_issue" && row.issue_number !== null && row.issue_number !== undefined
        ? `#${row.issue_number} ${displayTitle(row)}`
        : displayTitle(row);
    // One meta line (`#1263 documents-list-cards`): file name, mentioned and
    // fetched times, the mentions as plain linked text, and the kind label
    // linking out — all on one line, the file name ellipsising first.
    const meta = document.createElement("span");
    meta.className = "doc-meta";
    const failed = errors.get(row.key);
    if (isBusy) meta.textContent = "Fetching…";
    else if (failed) meta.textContent = failed;
    else if (row.moved_to) meta.textContent = `moved to ${row.moved_to}`;
    else {
      const file = document.createElement("span");
      file.className = "doc-meta-file";
      file.textContent = row.file_name;
      file.title = row.file_name;
      meta.appendChild(file);
      meta.appendChild(document.createTextNode(` · ${mentionedAgo(row.last_mentioned_ms, Date.now())}`));
      if (row.kind === "github_issue" && row.issue_status) {
        meta.appendChild(document.createTextNode(` · ${row.issue_status}`));
      }
      if (row.fetched && row.fetched_ms !== null) {
        const fetched = document.createElement("span");
        fetched.textContent = ` · ${fetchedAgo(row.fetched_ms, Date.now())}`;
        fetched.title = new Date(row.fetched_ms).toLocaleString();
        meta.appendChild(fetched);
      }
      const mentions = document.createElement("button");
      mentions.setAttribute("type", "button");
      mentions.className = "doc-mentions-link";
      mentions.textContent = `${row.mention_count} ${row.mention_count === 1 ? "mention" : "mentions"}`;
      listening.listen(mentions, "click", (event: Event) => {
        event.stopPropagation();
        deps.onMentions?.(row);
      });
      meta.appendChild(document.createTextNode(" · "));
      meta.appendChild(mentions);
      const kindLink = document.createElement("a");
      kindLink.className = "doc-kind";
      kindLink.href = row.url;
      kindLink.target = "_blank";
      kindLink.rel = "noopener noreferrer";
      kindLink.textContent = row.kind;
      kindLink.addEventListener("click", (event) => event.stopPropagation());
      meta.appendChild(document.createTextNode(" · "));
      meta.appendChild(kindLink);
    }
    texts.append(title, meta);
    card.appendChild(texts);

    const download = document.createElement("button");
    download.setAttribute("type", "button");
    download.className = "doc-download";
    download.textContent = isBusy ? "…" : row.fetched ? "✓" : "↓";
    download.setAttribute("aria-label", row.fetched ? `${row.file_name} is fetched` : `Fetch ${row.file_name}`);
    download.title = download.getAttribute("aria-label") ?? "";
    listening.listen(download, "click", (event: Event) => {
      event.stopPropagation();
      if (!row.fetched) void fetchOnly(row);
    });
    card.appendChild(download);

    // The star (`#1265`): a small toggle on every card, filled where starred.
    const star = document.createElement("button");
    star.setAttribute("type", "button");
    star.className = "doc-star";
    star.textContent = state.starred ? "★" : "☆";
    star.setAttribute("aria-pressed", String(state.starred));
    star.setAttribute("aria-label", state.starred ? `Unstar ${row.file_name}` : `Star ${row.file_name}`);
    star.title = star.getAttribute("aria-label") ?? "";
    listening.listen(star, "click", (event: Event) => {
      event.stopPropagation();
      states.setStarred(row.key, !state.starred);
      render(rowsNow);
    });
    card.appendChild(star);

    // Hide, or Unhide where hidden cards are being shown: the desktop equivalent
    // of the swipe, and the only way back short of Undo.
    const hide = document.createElement("button");
    hide.setAttribute("type", "button");
    hide.className = state.hidden ? "doc-unhide" : "doc-hide";
    hide.textContent = state.hidden ? "Unhide" : "Hide";
    hide.setAttribute("aria-label", `${hide.textContent} ${row.file_name}`);
    listening.listen(hide, "click", (event: Event) => {
      event.stopPropagation();
      if (state.hidden) {
        states.setHidden(row.key, false);
        render(rowsNow);
      } else {
        hideRow(row);
      }
    });
    card.appendChild(hide);

    // Swipe to hide (`#1265`): the message rows' gesture in shape — a touch
    // pointer stream, the axis decided once at 12px, the card following the
    // finger, 90px committing on release, and the click behind a committed
    // swipe claimed so it does not also open. A mouse never swipes here; the
    // Hide button above is its equivalent.
    let swipe: { x: number; y: number; dragging: boolean; active: boolean } | null = null;
    listening.listen(card, "pointerdown", (event: PointerEvent) => {
      swipeClaimedClick = false;
      if (!event || event.pointerType === "mouse") {
        swipe = null;
        return;
      }
      swipe = { x: event.clientX, y: event.clientY, dragging: false, active: true };
    });
    listening.listen(card, "pointermove", (event: PointerEvent) => {
      if (!swipe?.active) return;
      const dx = event.clientX - swipe.x;
      const dy = event.clientY - swipe.y;
      if (!swipe.dragging) {
        if (Math.abs(dx) < HIDE_SWIPE_START_PX && Math.abs(dy) < HIDE_SWIPE_START_PX) return;
        if (Math.abs(dy) >= Math.abs(dx)) {
          swipe.active = false;
          return;
        }
        swipe.dragging = true;
      }
      const style = (card as HTMLElement).style;
      if (style) style.transform = `translateX(${dx}px)`;
    });
    const finishSwipe = (event: PointerEvent): void => {
      if (!swipe?.active) return;
      const dx = swipe.dragging ? event.clientX - swipe.x : 0;
      swipe = null;
      const style = (card as HTMLElement).style;
      if (style) style.transform = "";
      if (Math.abs(dx) >= HIDE_SWIPE_COMMIT_PX && !state.hidden) {
        swipeClaimedClick = true;
        hideRow(row);
      }
    };
    listening.listen(card, "pointerup", finishSwipe);
    listening.listen(card, "pointercancel", () => {
      swipe = null;
      const style = (card as HTMLElement).style;
      if (style) style.transform = "";
    });

    listening.listen(card, "click", () => {
      if (swipeClaimedClick) {
        swipeClaimedClick = false;
        return;
      }
      void open(row);
    });
    listening.listen(card, "keydown", (event: KeyboardEvent) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        void open(row);
      }
    });
    return card;
  }

  const outer = createListening();
  let wiredOuter = false;
  let loadingOlder = false;
  let totalNow = 0;

  /** The shared scroller, where the page has one; null in a host without it. */
  function scrollerOrNull(): HTMLElement | null {
    try {
      return deps.byId("scroll-area") ?? null;
    } catch {
      return null;
    }
  }

  /** Whether `scroller` is parked at its bottom, within `slack` px. */
  function atBottom(scroller: HTMLElement, slack: number): boolean {
    return scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - slack;
  }

  /**
   * Whether the list is the pane on screen. The scroller is shared with the other views, so
   * its scrolling must page documents only while documents are what is being read; where the
   * panes cannot be asked (a partial host), the list counts as on screen as it always did.
   */
  function listOnScreen(): boolean {
    try {
      const pane = deps.byId("pane-docs");
      const view = deps.byId("docs-list-view");
      if (pane && pane.hidden) return false;
      if (view && view.hidden) return false;
      return true;
    } catch {
      return true;
    }
  }

  function wireOuter(list: HTMLElement): void {
    if (wiredOuter) return;
    wiredOuter = true;
    const scroller = deps.byId("scroll-area");
    outer.listen(scroller, "scroll", () => {
      if (!listOnScreen()) return;
      const near = scroller.scrollTop < OLDER_TRIGGER_PX;
      if (near && !loadingOlder && rowsNow.length < totalNow) void loadOlder();
    });
    // Drag up parked at the bottom to refresh: the channel list's pull-up gesture, in kind.
    // The list's own handling rather than the channel's owner, whose every dependency is a
    // channel read; the meaning is the same — parked at the newest end, drag past the edge,
    // release to refresh. The shared scroller's contained overscroll leaves the platform's
    // own stretch showing on both views, which is the only animation either needs.
    let pullY: number | null = null;
    outer.listen(list, "touchstart", (event: TouchEvent) => {
      pullY = atBottom(scroller, PULL_EDGE_SLACK_PX) ? (event.touches[0]?.clientY ?? null) : null;
    });
    outer.listen(list, "touchend", (event: TouchEvent) => {
      const start = pullY;
      pullY = null;
      const end = event.changedTouches[0]?.clientY;
      if (start !== null && end !== undefined && start - end > REFRESH_PULL_PX && atBottom(scroller, PULL_EDGE_SLACK_PX)) {
        void refresh()
          .then(() => deps.onStatus?.("The list is current."))
          .catch(() => undefined);
      }
    });
    outer.listen(list, "touchcancel", () => {
      pullY = null;
    });
  }

  async function loadOlder(): Promise<void> {
    const oldest = rowsNow.reduce<number | null>(
      (min, row) => (row.last_mentioned_ms !== null && (min === null || row.last_mentioned_ms < min) ? row.last_mentioned_ms : min),
      null,
    );
    if (oldest === null) return;
    loadingOlder = true;
    try {
      const answer = (await deps.api(`/api/v1/documents?limit=100&before_ms=${oldest}`)) as DocumentsListAnswer;
      const older = Array.isArray(answer?.documents) ? answer.documents : [];
      const seen = new Set(rowsNow.map((row) => row.key));
      // Older cards land ABOVE the reader: keep the card under their eye in place by the
      // height the redraw added over it, measured across the render itself.
      const scroller = scrollerOrNull();
      const heightBefore = scroller ? scroller.scrollHeight : 0;
      render([...rowsNow, ...older.filter((row) => !seen.has(row.key))]);
      if (scroller && listOnScreen()) {
        const grew = scroller.scrollHeight - heightBefore;
        if (grew > 0) scroller.scrollTop += grew;
      }
    } catch {
      // Scrolling on is its own retry.
    } finally {
      loadingOlder = false;
    }
  }

  function bucketBar(): HTMLElement {
    const bar = document.createElement("div");
    bar.className = "doc-buckets";
    bar.setAttribute("role", "group");
    bar.setAttribute("aria-label", "Document type");
    const hasOther = rowsNow.some((row) => bucketOfKind(row.kind) === "other");
    for (const { bucket, label } of BUCKET_BUTTONS) {
      // "Other files" exists only while a row needs it (`#1176`).
      if (bucket === "other" && !hasOther && bucketFilter !== "other") continue;
      const button = document.createElement("button");
      button.setAttribute("type", "button");
      button.className = "doc-bucket";
      button.setAttribute("data-bucket", bucket);
      button.textContent = label;
      button.setAttribute("aria-pressed", String(bucketFilter === bucket));
      listening.listen(button, "click", () => {
        bucketFilter = bucket;
        render(rowsNow);
      });
      bar.appendChild(button);
    }
    // The Starred chip (`#1265`): the existing chips, reused — a row must pass
    // it and the bucket both, so Starred narrows whatever bucket is chosen.
    const starred = document.createElement("button");
    starred.setAttribute("type", "button");
    starred.className = "doc-bucket doc-starred-chip";
    starred.setAttribute("data-bucket", "starred");
    starred.textContent = "Starred";
    starred.setAttribute("aria-pressed", String(starredOnly));
    listening.listen(starred, "click", () => {
      starredOnly = !starredOnly;
      render(rowsNow);
    });
    bar.appendChild(starred);
    return bar;
  }

  /** The Undo chip a hide leaves behind, in the Done Undo's shape. */
  function undoChip(): HTMLElement | null {
    if (!undoOffer) return null;
    const row = rowsNow.find((one) => one.key === undoOffer?.key);
    const chip = document.createElement("button");
    chip.setAttribute("type", "button");
    chip.className = "chip doc-undo";
    chip.setAttribute("id", "docs-undo");
    chip.textContent = `Undo hide ${row ? row.file_name : "document"}`;
    listening.listen(chip, "click", () => {
      if (undoHide()) deps.onStatus?.("Document is back in the list.");
    });
    return chip;
  }

  function undoHide(): boolean {
    if (!undoOffer) return false;
    const key = undoOffer.key;
    endUndoOffer();
    states.setHidden(key, false);
    render(rowsNow);
    return true;
  }

  function render(rows: readonly DocumentRow[]): void {
    rowsNow = dedupeDocumentRows(rows).sort(byOldest);
    listening.unmount();
    const list = deps.byId(LIST_ID);
    wireOuter(list);
    if (rows.length === 0) {
      const note = document.createElement("p");
      note.className = "doc-empty";
      note.textContent = "No documents yet.";
      list.replaceChildren(note);
      return;
    }
    const head = document.createElement("p");
    head.className = "doc-list-head";
    head.textContent = `Oldest first · ${totalNow || rowsNow.length} documents · newest at the bottom`;
    const shown = visibleRows();
    const cards = shown.map(cardFor);
    const undo = undoChip();
    if (shown.length === 0) {
      const note = document.createElement("p");
      note.className = "doc-empty";
      note.textContent = starredOnly
        ? "No starred documents."
        : "No documents of this type.";
      list.replaceChildren(head, note, ...(undo ? [undo] : []), bucketBar());
      return;
    }
    // The filter chips dock at the bottom (`#1263 documents-list-cards`):
    // the list opens at its newest end, so the chips are the last row and
    // stick above the bottom bar. One `.doc-buckets` bar (see bucketBar),
    // and the Starred chip (`#1265`) joins this same element. The Undo chip
    // sits just above it, where the reader's eye already is.
    list.replaceChildren(head, ...cards, ...(undo ? [undo] : []), bucketBar());
    // Arrow keys walk the cards; Enter opens (above).
    listening.listen(list, "keydown", (event: KeyboardEvent) => {
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const cardsNow = [...list.querySelectorAll(".doc-card")];
      const at = cardsNow.indexOf(document.activeElement as Element);
      if (at === -1) return;
      event.preventDefault();
      const next = cardsNow[at + (event.key === "ArrowDown" ? 1 : -1)] as HTMLElement | undefined;
      if (next) next.focus();
    });
  }

  /**
   * Fetch one row, saying what landed: the answer's patch for it, `null` where the fetch
   * landed without a readable entry, or `false` where it failed — the failure recorded
   * on the row and said, as a single fetch always has.
   */
  async function fetchRowEntry(row: DocumentRow): Promise<FetchedEntryPatch | null | false> {
    try {
      const answer = await deps.api(FETCH_PATH, { method: "POST", body: { key: row.key } });
      return fetchedPatchOf(answer, row.key);
    } catch (error) {
      const words = error instanceof Error ? error.message : String(error);
      errors.set(row.key, `Could not fetch it. ${words}`);
      deps.onStatus?.(`Could not fetch ${row.file_name}. ${words}`);
      return false;
    }
  }

  async function fetchRow(row: DocumentRow): Promise<boolean> {
    return (await fetchRowEntry(row)) !== false;
  }

  /** Mark `key` fetched in the drawn rows, from its fetch answer's patch where one landed. */
  function patchFetched(key: string, patch: FetchedEntryPatch | null): void {
    rowsNow = rowsNow.map((row) => (row.key === key
      ? {
          ...row,
          fetched: true,
          title: patch?.title ?? row.title,
          version: patch ? patch.version : row.version,
          fetched_ms: patch ? patch.fetched_ms : row.fetched_ms,
        }
      : row));
  }

  async function fetchOnly(row: DocumentRow): Promise<void> {
    if (busy.has(row.key) || row.fetched) return;
    busy.add(row.key);
    errors.delete(row.key);
    render(rowsNow);
    const ok = await fetchRow(row);
    busy.delete(row.key);
    if (ok) await refresh();
    else render(rowsNow);
  }

  async function open(row: DocumentRow): Promise<void> {
    if (busy.has(row.key)) return;
    if (row.fetched) {
      deps.onOpen(row.key);
      return;
    }
    busy.add(row.key);
    errors.delete(row.key);
    render(rowsNow);
    const ok = await fetchRow(row);
    busy.delete(row.key);
    if (!ok) {
      render(rowsNow);
      return;
    }
    deps.onOpen(row.key);
  }

  async function refresh(place?: DocumentsListPlace): Promise<void> {
    // Keep the reader's place across the redraw, as the message views do: a reader parked
    // at the bottom follows newly appended documents there (which is also how entering the
    // list at its newest end lands at the bottom); anyone else stays where they were.
    // A caller returning to the list (`#1264`) names the place instead: it was
    // recorded when the document opened, because by return time the shared
    // scroller holds the reader's place, not the list's.
    const scroller = scrollerOrNull();
    const follow = place ? place.follow : scroller !== null && listOnScreen() && atBottom(scroller, FOLLOW_SLACK_PX);
    const top = place ? place.top : scroller ? scroller.scrollTop : 0;
    const answer = (await deps.api(LIST_PATH)) as DocumentsListAnswer;
    totalNow = typeof answer?.total === "number" ? answer.total : 0;
    render(Array.isArray(answer?.documents) ? answer.documents : []);
    if (scroller && listOnScreen()) {
      scroller.scrollTop = follow ? scroller.scrollHeight : top;
    }
  }

  /**
   * Fetch `targets` newest first with at most `FETCH_CONCURRENCY` in flight (`#1307`).
   * Each row patches into `rowsNow` from its own fetch answer and redraws as it lands,
   * so cards flip one by one and no list re-read follows the run. `progress` hears
   * (done, total) at the start and as each fetch completes, landed or failed.
   */
  async function fetchMany(
    targets: readonly DocumentRow[],
    progress?: (done: number, total: number) => unknown,
  ): Promise<number> {
    const pending = targets
      .filter((row) => !row.fetched && !busy.has(row.key))
      .sort((a, b) => (b.last_mentioned_ms ?? 0) - (a.last_mentioned_ms ?? 0));
    const total = pending.length;
    let arrived = 0;
    let done = 0;
    let next = 0;
    progress?.(0, total);
    async function worker(): Promise<void> {
      while (true) {
        const at = next;
        next += 1;
        if (at >= total) return;
        const row = pending[at]!;
        if (busy.has(row.key)) {
          done += 1;
          progress?.(done, total);
          continue;
        }
        busy.add(row.key);
        errors.delete(row.key);
        render(rowsNow);
        const entry = await fetchRowEntry(row);
        busy.delete(row.key);
        if (entry !== false) {
          patchFetched(row.key, entry);
          arrived += 1;
        }
        done += 1;
        progress?.(done, total);
        render(rowsNow);
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(FETCH_CONCURRENCY, total) }, () => worker()),
    );
    return arrived;
  }

  return {
    refresh,
    fetchVisible: (progress) => fetchMany(rowsOnScreen(), progress),
    fetchSince: (cutoffMs: number, progress) =>
      fetchMany(visibleRows().filter((row) => (row.last_mentioned_ms ?? 0) >= cutoffMs), progress),
    async fetchRecent(): Promise<number> {
      const answer = (await deps.api(FETCH_RECENT_PATH, {
        method: "POST",
        body: { limit: FETCH_RECENT_LIMIT },
      })) as { fetched?: unknown } | null;
      const fetched = Array.isArray(answer?.fetched) ? answer.fetched.length : 0;
      await refresh();
      return fetched;
    },
    async refreshFetched(progress) {
      const fetchedRows = rowsNow.filter((row) => row.fetched);
      let updated = 0;
      let current = 0;
      for (let i = 0; i < fetchedRows.length; i += 1) {
        const row = fetchedRows[i]!;
        progress?.(i, fetchedRows.length);
        let fresh: { update_available?: unknown } | null = null;
        try {
          fresh = (await deps.api(FRESHNESS_PATH(row.key))) as { update_available?: unknown } | null;
        } catch {
          current += 1;
          continue;
        }
        if (fresh?.update_available === true) {
          try {
            await deps.api(FETCH_PATH, { method: "POST", body: { key: row.key, refresh: true } });
            updated += 1;
          } catch {
            current += 1;
          }
        } else {
          current += 1;
        }
      }
      progress?.(fetchedRows.length, fetchedRows.length);
      await refresh();
      return { updated, current };
    },
    async clearCached(): Promise<number> {
      const answer = (await deps.api("/api/v1/documents/cache", { method: "DELETE" })) as {
        cleared?: unknown;
      } | null;
      const cleared = typeof answer?.cleared === "number" ? answer.cleared : 0;
      await refresh();
      return cleared;
    },
    showHidden: () => showingHidden,
    setShowHidden(on: boolean): void {
      showingHidden = on === true;
      render(rowsNow);
    },
    setStarred(key: string, on: boolean): void {
      states.setStarred(key, on);
      render(rowsNow);
    },
    isStarred: (key: string) => docStateOf(states.states(), key).starred,
    hide(key: string): void {
      const row = rowsNow.find((one) => one.key === key);
      if (row) hideRow(row);
      else {
        states.setHidden(key, true);
        render(rowsNow);
      }
    },
    undoHide,
    reloadStates(): void {
      states.reload();
      render(rowsNow);
    },
    unmount(): void {
      endUndoOffer();
      wiredOuter = false;
      outer.unmount();
      listening.unmount();
    },
  };
}

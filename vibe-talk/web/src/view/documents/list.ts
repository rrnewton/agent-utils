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
import type { DocumentRow, DocumentsListAnswer } from "./types.ts";

export interface DocumentsListDependencies {
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  byId(id: string): HTMLElement;
  onOpen(key: string): unknown;
  onMentions?(row: DocumentRow): unknown;
  onStatus?(text: string): unknown;
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
  /** Fetch the unfetched among the rows just drawn; how many arrived. */
  fetchVisible(): Promise<number>;
  /** Fetch unfetched rows last mentioned at or after `cutoffMs`. */
  fetchSince(cutoffMs: number): Promise<number>;
  /** Fetch the most recent unfetched documents, then redraw. */
  fetchRecent(): Promise<number>;
  /** Check fetched rows for newer versions, updating them; progress by key. */
  refreshFetched(progress?: (done: number, total: number) => unknown): Promise<{ updated: number; current: number }>;
  /** Drop every cached copy and redraw; how many were held. */
  clearCached(): Promise<number>;
  unmount(): void;
}

const LIST_ID = "documents-list";
const LIST_PATH = "/api/v1/documents?limit=100";
const FETCH_PATH = "/api/v1/documents/fetch";
const FETCH_RECENT_PATH = "/api/v1/documents/fetch-recent";
const FRESHNESS_PATH = (key: string) => `/api/v1/documents/${encodeURIComponent(key)}/freshness`;
const FETCH_RECENT_LIMIT = 10;

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

export function createDocumentsList(deps: DocumentsListDependencies): DocumentsList {
  const listening = createListening();
  const busy = new Set<string>();
  const errors = new Map<string, string>();
  let rowsNow: readonly DocumentRow[] = [];
  // The type filter's state lives with the list's other view state (no new
  // persistence key): `#1176 doc-kind-buckets`.
  let bucketFilter: DocumentBucket | "all" = "all";

  function visibleRows(): readonly DocumentRow[] {
    if (bucketFilter === "all") return rowsNow;
    return rowsNow.filter((row) => bucketOfKind(row.kind) === bucketFilter);
  }

  function cardFor(row: DocumentRow): HTMLElement {
    const isBusy = busy.has(row.key);
    const card = document.createElement("div");
    card.className = row.fetched ? "doc-card" : "doc-card doc-unfetched";
    if (isBusy) card.className += " doc-busy";
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

    listening.listen(card, "click", () => void open(row));
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
    return bar;
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
    if (shown.length === 0) {
      const note = document.createElement("p");
      note.className = "doc-empty";
      note.textContent = "No documents of this type.";
      list.replaceChildren(head, note, bucketBar());
      return;
    }
    // The filter chips dock at the bottom (`#1263 documents-list-cards`):
    // the list opens at its newest end, so the chips are the last row and
    // stick above the bottom bar. One `.doc-buckets` bar (see bucketBar),
    // so a later Starred chip joins this same element.
    list.replaceChildren(head, ...cards, bucketBar());
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

  async function fetchRow(row: DocumentRow): Promise<boolean> {
    try {
      await deps.api(FETCH_PATH, { method: "POST", body: { key: row.key } });
      return true;
    } catch (error) {
      const words = error instanceof Error ? error.message : String(error);
      errors.set(row.key, `Could not fetch it. ${words}`);
      deps.onStatus?.(`Could not fetch ${row.file_name}. ${words}`);
      return false;
    }
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

  async function fetchMany(targets: readonly DocumentRow[]): Promise<number> {
    let arrived = 0;
    for (const row of targets) {
      if (row.fetched || busy.has(row.key)) continue;
      busy.add(row.key);
      render(rowsNow);
      if (await fetchRow(row)) arrived += 1;
      busy.delete(row.key);
    }
    await refresh();
    return arrived;
  }

  return {
    refresh,
    fetchVisible: () => fetchMany(visibleRows()),
    fetchSince: (cutoffMs: number) =>
      fetchMany(visibleRows().filter((row) => (row.last_mentioned_ms ?? 0) >= cutoffMs)),
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
    unmount(): void {
      wiredOuter = false;
      outer.unmount();
      listening.unmount();
    },
  };
}

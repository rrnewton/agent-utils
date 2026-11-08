// The Documents list (`#1022 documents-view-client`, batch A): one bordered card
// per document the fold knows, in the order the server gives them — mention
// order, newest last. Unfetched cards are dimmed; fetched ones are full
// saturation, with when they were fetched said quietly. A card names the
// document only: its real title once fetched, its file name before that. The
// kind label is the small link out to the source. All words from the server
// are untrusted, so every visible string goes through `textContent`.

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
  refresh(): Promise<void>;
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

const ICON_PATHS: Record<string, string[]> = {
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
  const paths = kind.startsWith("github") ? ICON_PATHS.github : kind === "paste" ? ICON_PATHS.paste : ICON_PATHS.file;
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

export function createDocumentsList(deps: DocumentsListDependencies): DocumentsList {
  const listening = createListening();
  const busy = new Set<string>();
  const errors = new Map<string, string>();
  let rowsNow: readonly DocumentRow[] = [];

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
    title.textContent = row.fetched ? row.title : row.file_name;
    const subtitle = document.createElement("span");
    subtitle.className = "doc-subtitle";
    const failed = errors.get(row.key);
    if (isBusy) subtitle.textContent = "Fetching…";
    else if (failed) subtitle.textContent = failed;
    else if (row.moved_to) subtitle.textContent = `moved to ${row.moved_to}`;
    else {
      subtitle.textContent = `${row.file_name} · ${row.mention_count} ${row.mention_count === 1 ? "mention" : "mentions"}`;
      if (row.fetched && row.fetched_ms !== null) {
        subtitle.textContent += ` · ${fetchedAgo(row.fetched_ms, Date.now())}`;
        subtitle.title = new Date(row.fetched_ms).toLocaleString();
      }
    }
    const kindLink = document.createElement("a");
    kindLink.className = "doc-kind";
    kindLink.href = row.url;
    kindLink.target = "_blank";
    kindLink.rel = "noopener noreferrer";
    kindLink.textContent = row.kind;
    kindLink.addEventListener("click", (event) => event.stopPropagation());
    texts.append(title, subtitle, kindLink);
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

  function render(rows: readonly DocumentRow[]): void {
    rowsNow = rows;
    listening.unmount();
    const list = deps.byId(LIST_ID);
    if (rows.length === 0) {
      const note = document.createElement("p");
      note.className = "doc-empty";
      note.textContent = "No documents yet.";
      list.replaceChildren(note);
      return;
    }
    const cards = rows.map(cardFor);
    list.replaceChildren(...cards);
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

  async function refresh(): Promise<void> {
    const answer = (await deps.api(LIST_PATH)) as DocumentsListAnswer;
    render(Array.isArray(answer?.documents) ? answer.documents : []);
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
    fetchVisible: () => fetchMany(rowsNow),
    fetchSince: (cutoffMs: number) =>
      fetchMany(rowsNow.filter((row) => (row.last_mentioned_ms ?? 0) >= cutoffMs)),
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
      listening.unmount();
    },
  };
}

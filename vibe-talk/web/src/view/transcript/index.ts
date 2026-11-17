// The Transcripts view (`#1608 agent-transcripts-timeline`): agent · day
// trace threads and their chunks, the one home of transcript posts.
// No thread names, no raw-feed rows: a day opens as plain chunks in time
// order, and a search hit can open the day at its message, in context.

import {
  chunkMatches,
  filterTranscripts,
  sortTranscripts,
  transcriptLabel,
} from "../../core/transcripts/index.ts";
import type { TranscriptThread } from "../../core/transcripts/index.ts";

/** A served message, the parts this view reads. */
export interface TranscriptMessage {
  id: string;
  timestamp: string;
  content: string;
  author?: string;
}

/** What the Transcripts view asks of the page. */
export interface TranscriptsDependencies {
  el(id: string): HTMLElement;
  api(path: string): Promise<unknown>;
  intoView?(element: Element, block: string): void;
  setStatus?(text: string): void;
}

/** The Transcripts view's commands. */
export interface TranscriptsView {
  /** Whether the Transcripts screen is the one showing. */
  active(): boolean;
  /** Show the index, loading it first where it has never loaded. */
  open(): Promise<void>;
  /** Leave the screen (the caller shows another view). */
  close(): void;
  /** Reload the index from the server (store-only there). */
  refresh(): Promise<void>;
  /** Open one agent-day at a message: the search hit's way in. */
  openAt(hit: { channel_id: string; thread?: { id: string } | null; id: string }): Promise<void>;
  /** The scoped search text changed: filter the index and the open day. */
  searchChanged(): void;
}

/** Create the view over its panel in the page. */
export function createTranscriptsView(deps: TranscriptsDependencies): TranscriptsView {
  let active = false;
  let loaded = false;
  let rows: TranscriptThread[] = [];
  let openRow: TranscriptThread | null = null;
  let chunks: TranscriptMessage[] = [];
  let highlightId = "";

  const panel = () => deps.el("transcripts-view");

  function renderList(): void {
    const list = deps.el("transcripts-list");
    const query = (deps.el("transcripts-search") as HTMLInputElement).value || "";
    const shown = filterTranscripts(sortTranscripts(rows), query);
    list.replaceChildren();
    for (const row of shown) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.setAttribute("type", "button");
      button.className = "transcript-row";
      button.dataset.channel = String(row.channel.id);
      button.dataset.thread = row.thread_id;
      button.textContent = transcriptLabel(row);
      button.addEventListener("click", () => { void openDay(row, ""); });
      item.append(button);
      list.append(item);
    }
    const note = deps.el("transcripts-note");
    note.textContent = rows.length === 0
      ? "No agent transcripts yet."
      : shown.length === 0
        ? "No transcripts match."
        : "";
  }

  function renderDay(): void {
    const log = deps.el("transcripts-log");
    const query = (deps.el("transcripts-search") as HTMLInputElement).value || "";
    log.replaceChildren();
    for (const chunk of chunks) {
      if (!chunkMatches(chunk.content, query)) continue;
      const item = document.createElement("li");
      item.className = "transcript-chunk";
      item.dataset.transcriptMessage = String(chunk.id);
      const when = document.createElement("div");
      when.className = "transcript-when";
      const at = Date.parse(chunk.timestamp);
      when.textContent = Number.isNaN(at) ? "" : new Date(at).toISOString();
      const body = document.createElement("div");
      body.className = "transcript-body";
      body.textContent = chunk.content;
      item.append(when, body);
      log.append(item);
    }
    deps.el("transcripts-title").textContent = openRow ? transcriptLabel(openRow) : "";
    deps.el("transcripts-day").hidden = openRow === null;
    deps.el("transcripts-list").hidden = openRow !== null;
    if (highlightId !== "") {
      const target = log.querySelector(`[data-transcript-message="${CSS.escape(highlightId)}"]`);
      if (target) {
        target.classList.add("transcript-highlight");
        if (deps.intoView) deps.intoView(target, "center");
      }
      highlightId = "";
    }
  }

  async function openDay(row: TranscriptThread, atMessage: string): Promise<void> {
    openRow = row;
    highlightId = atMessage;
    const answer = await deps.api(
      `/api/v1/transcripts/${encodeURIComponent(String(row.channel.id))}/${encodeURIComponent(row.thread_id)}`,
    ) as { messages?: TranscriptMessage[] };
    chunks = [...(answer.messages || [])].sort(
      (a, b) => (Date.parse(a.timestamp) || 0) - (Date.parse(b.timestamp) || 0),
    );
    renderDay();
  }

  async function refresh(): Promise<void> {
    const answer = await deps.api("/api/v1/transcripts") as { transcripts?: TranscriptThread[] };
    rows = answer.transcripts || [];
    loaded = true;
    renderList();
    if (openRow) renderDay();
  }

  return {
    active: () => active,
    async open() {
      active = true;
      panel().hidden = false;
      if (!loaded) await refresh();
      else { renderList(); if (openRow) renderDay(); }
    },
    close() {
      active = false;
      panel().hidden = true;
    },
    refresh,
    async openAt(hit) {
      const threadId = hit.thread ? String(hit.thread.id) : "";
      if (!loaded) await refresh();
      const row = rows.find(
        (one) => String(one.channel.id) === String(hit.channel_id) && one.thread_id === threadId,
      ) || rows.find((one) => String(one.channel.id) === String(hit.channel_id));
      if (!row) {
        if (deps.setStatus) deps.setStatus("That transcript is not in the local index yet.");
        return;
      }
      await openDay(row, String(hit.id));
    },
    searchChanged() {
      if (openRow) renderDay();
      else renderList();
    },
  };
}

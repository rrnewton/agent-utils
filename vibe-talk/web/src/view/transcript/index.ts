// The Transcripts view (`#1608 agent-transcripts-timeline`): agent · day
// trace threads and their chunks, the one home of transcript posts.
// No thread names, no raw-feed rows: a day opens at its newest chunk,
// follows what arrives, and steps back a window at a time as the reader
// scrolls up (`#1886 vibe-talk-transcripts-view` part B). A search hit
// can open the day at its message, in context.

import {
  chunkHeld,
  chunkMatches,
  filterTranscripts,
  mergeChunks,
  sortChunks,
  sortTranscripts,
  transcriptLabel,
} from "../../core/transcripts/index.ts";
import type { TranscriptWindow } from "../../core/transcripts/index.ts";
import { messageTimeLabel } from "../../present/time/index.ts";
import type { TranscriptThread } from "../../core/transcripts/index.ts";

/** A served message, the parts this view reads. */
export interface TranscriptMessage {
  id: string;
  timestamp: string;
  content: string;
  author?: string;
  /** A resolved sent message standing vestigial in the timeline (`#1886` part C). */
  sent_vestigial?: boolean;
  /** Its thread, for the tint of a vestigial entry. */
  thread?: { id?: string; hue?: number | null } | null;
}

/** What the Transcripts view asks of the page. */
export interface TranscriptsDependencies {
  el(id: string): HTMLElement;
  api(path: string): Promise<unknown>;
  intoView?(element: Element, block: string): void;
  setStatus?(text: string): void;
  /** Run `fn` every `ms` until the answer is called; the follow timer. */
  every?(ms: number, fn: () => void): () => void;
}

/** Chunks asked for per window, the server's own page size. */
export const DAY_WINDOW = 50;
/** How often an open day looks for newer chunks while it is followed. */
export const FOLLOW_MS = 4_000;

/**
 * A chunk's time the way the channel rows say it (`#1608` review): the
 * page's own formatter on the reader's clock — Eastern for the owner —
 * never a raw ISO stamp. A time that cannot be parsed shows as sent.
 */
export function chunkWhen(chunk: { timestamp: string }): string {
  const at = Date.parse(chunk.timestamp);
  if (Number.isNaN(at)) return String(chunk.timestamp || "");
  return messageTimeLabel({ timestamp: chunk.timestamp });
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
  /** Back from an open day to the index. */
  closeDay(): void;
  /** Load the next older window of the open day, where there is one. */
  loadOlder(): Promise<void>;
}

/** Create the view over its panel in the page. */
export function createTranscriptsView(deps: TranscriptsDependencies): TranscriptsView {
  let active = false;
  let loaded = false;
  let rows: TranscriptThread[] = [];
  let openRow: TranscriptThread | null = null;
  let chunks: TranscriptMessage[] = [];
  let nextBefore: string | null = null;
  let hasMore = false;
  let loadingOlder = false;
  let following = true;
  let highlightId = "";
  /** Sent messages the reader has opened out of their vestigial line. */
  const expandedSent = new Set<string>();
  let stopFollow: (() => void) | null = null;
  let listening = false;
  let olderBound = false;

  const panel = () => deps.el("transcripts-view");
  const scroller = (): HTMLElement => deps.el("scroll-area");
  const atBottom = (): boolean => {
    const area = scroller();
    return area.scrollHeight - area.scrollTop - area.clientHeight < 160;
  };
  const toBottom = (): void => {
    const area = scroller();
    area.scrollTop = area.scrollHeight;
  };

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

  function renderDay(stay: { top: number; height: number } | null = null): void {
    const log = deps.el("transcripts-log");
    const query = (deps.el("transcripts-search") as HTMLInputElement).value || "";
    const area = scroller();
    log.replaceChildren();
    for (const chunk of chunks) {
      if (!chunkMatches(chunk.content, query)) continue;
      const item = document.createElement("li");
      item.dataset.transcriptMessage = String(chunk.id);
      const when = document.createElement("div");
      when.className = "transcript-when";
      when.textContent = chunkWhen(chunk);
      const body = document.createElement("div");
      body.className = "transcript-body";
      if (chunk.sent_vestigial) {
        // `#1886` part C: a chat message the agent sent, standing in
        // the timeline where it happened - small and tinted with its
        // thread's colour until tapped, then its full words, as a
        // regular card shows them.
        const open = expandedSent.has(String(chunk.id));
        item.className = open ? "transcript-chunk transcript-sent transcript-sent-open" : "transcript-chunk transcript-sent";
        item.setAttribute("role", "button");
        item.setAttribute("aria-expanded", String(open));
        if (chunk.thread && typeof chunk.thread.hue === "number") {
          item.style.setProperty("--thread-hue", String(chunk.thread.hue));
        }
        const who = chunk.author ? `${chunk.author} sent` : "sent";
        if (open) {
          body.textContent = chunk.content;
          item.append(when, body);
        } else {
          const first = chunk.content.split("\n")[0] || "";
          body.textContent = `${who}: ${first.length > 90 ? `${first.slice(0, 90)}…` : first}`;
          item.append(body);
        }
        item.addEventListener("click", () => {
          if (open) expandedSent.delete(String(chunk.id));
          else expandedSent.add(String(chunk.id));
          renderDay();
        });
      } else {
        item.className = "transcript-chunk";
        body.textContent = chunk.content;
        item.append(when, body);
      }
      log.append(item);
    }
    deps.el("transcripts-title").textContent = openRow ? transcriptLabel(openRow) : "";
    deps.el("transcripts-day").hidden = openRow === null;
    deps.el("transcripts-list").hidden = openRow !== null;
    const older = deps.el("transcripts-older") as HTMLButtonElement;
    older.hidden = openRow === null || !hasMore;
    older.disabled = loadingOlder;
    older.textContent = loadingOlder ? "Loading earlier chunks…" : "Earlier chunks";
    if (highlightId !== "") {
      const target = log.querySelector(`[data-transcript-message="${CSS.escape(highlightId)}"]`);
      if (target) {
        target.classList.add("transcript-highlight");
        if (deps.intoView) deps.intoView(target, "center");
      }
      highlightId = "";
    } else if (stay) {
      // Older chunks landed above: keep the reader on the same words.
      area.scrollTop = stay.top + (area.scrollHeight - stay.height);
    } else if (following) {
      toBottom();
    }
  }

  function dayPath(row: TranscriptThread, before: string | null): string {
    const base = `/api/v1/transcripts/${encodeURIComponent(String(row.channel.id))}/${encodeURIComponent(row.thread_id)}`;
    const query = before === null ? `?limit=${DAY_WINDOW}` : `?limit=${DAY_WINDOW}&before=${encodeURIComponent(before)}`;
    return base + query;
  }

  async function readWindow(row: TranscriptThread, before: string | null): Promise<TranscriptWindow<TranscriptMessage>> {
    return await deps.api(dayPath(row, before)) as TranscriptWindow<TranscriptMessage>;
  }

  async function openDay(row: TranscriptThread, atMessage: string): Promise<void> {
    // Read first, swap after: the day on screen stays until its
    // replacement is in hand (no placeholder flash, `#1886` part B).
    const answer = await readWindow(row, null);
    openRow = row;
    expandedSent.clear();
    chunks = sortChunks(answer.messages || []);
    nextBefore = answer.next_before ?? null;
    hasMore = answer.has_more === true && nextBefore !== null;
    loadingOlder = false;
    following = atMessage === "";
    highlightId = atMessage;
    // A search hit below the newest window: step back until it is held.
    while (atMessage !== "" && !chunkHeld(chunks, atMessage) && hasMore) {
      const older = await readWindow(row, nextBefore);
      chunks = mergeChunks(chunks, older.messages || []);
      nextBefore = older.next_before ?? null;
      hasMore = older.has_more === true && nextBefore !== null;
    }
    renderDay();
    startFollow();
  }

  async function loadOlder(): Promise<void> {
    if (!openRow || !hasMore || loadingOlder || nextBefore === null) return;
    loadingOlder = true;
    const area = scroller();
    const stay = { top: area.scrollTop, height: area.scrollHeight };
    renderDay(stay);
    try {
      const answer = await readWindow(openRow, nextBefore);
      chunks = mergeChunks(chunks, answer.messages || []);
      nextBefore = answer.next_before ?? null;
      hasMore = answer.has_more === true && nextBefore !== null;
    } finally {
      loadingOlder = false;
      renderDay(stay);
    }
  }

  /** Bring an open day up to its newest chunk, where it is followed. */
  async function followNew(): Promise<void> {
    if (!active || !openRow) return;
    const answer = await readWindow(openRow, null);
    const merged = mergeChunks(chunks, answer.messages || []);
    if (merged.length === chunks.length && chunks.every((chunk, at) => chunk.id === merged[at]?.id)) return;
    chunks = merged;
    renderDay();
  }

  function startFollow(): void {
    stopFollow?.();
    stopFollow = null;
    if (!deps.every) return;
    stopFollow = deps.every(FOLLOW_MS, () => {
      if (following) void followNew().catch(() => {});
    });
  }

  async function refresh(): Promise<void> {
    const answer = await deps.api("/api/v1/transcripts") as { transcripts?: TranscriptThread[] };
    rows = answer.transcripts || [];
    loaded = true;
    if (openRow) renderDay();
    else renderList();
  }

  // The scroll that steps a day back, and the follow that a scroll away
  // pauses: one listener on the shared scroller, live while this view is.
  const onScroll = (): void => {
    if (!active || !openRow) return;
    following = atBottom();
    const area = scroller();
    if (area.scrollTop < 240) void loadOlder();
  };

  return {
    active: () => active,
    async open() {
      active = true;
      panel().hidden = false;
      if (!listening) {
        listening = true;
        scroller().addEventListener("scroll", onScroll);
      }
      if (!olderBound) {
        olderBound = true;
        deps.el("transcripts-older").addEventListener("click", () => { void loadOlder(); });
      }
      if (!loaded) await refresh();
      else if (openRow) renderDay();
      else renderList();
    },
    close() {
      active = false;
      panel().hidden = true;
      stopFollow?.();
      stopFollow = null;
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
    closeDay() {
      stopFollow?.();
      stopFollow = null;
      openRow = null;
      chunks = [];
      nextBefore = null;
      hasMore = false;
      highlightId = "";
      renderList();
      deps.el("transcripts-day").hidden = true;
      deps.el("transcripts-list").hidden = false;
      deps.el("transcripts-title").textContent = "";
    },
    loadOlder,
  };
}

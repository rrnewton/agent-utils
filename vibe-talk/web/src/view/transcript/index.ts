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
  pickAnchor,
  sortChunks,
  sortTranscripts,
  transcriptLabel,
} from "../../core/transcripts/index.ts";
import type { TranscriptPlace, TranscriptWindow } from "../../core/transcripts/index.ts";
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
  /** Open a sent message's thread in the channel view (`#1886` part C). */
  openSentThread?(message: TranscriptMessage): void;
  /** Note where the reader is, for a reload to restore (`#2153`). */
  notePlace?(place: TranscriptPlace | null): void;
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
  /** Where the reader is now, for the UI-state record (`#2153`). */
  place(): TranscriptPlace | null;
  /** Restore a saved place after a reload: its day, at its chunk (`#2153`). */
  restore(place: TranscriptPlace): Promise<void>;
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
  // `#1886` addendum-1 (b): which showing of the view async reads belong
  // to. Leaving the view (or a day) bumps it, and a read that lands
  // after counts for nothing - it neither renders nor moves the
  // scroller the reader has since taken to another view.
  let generation = 0;
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

  // `#2153`: the reader's place is a chunk, not a pixel count. Capturing
  // the anchor chunk and its offset before a redraw and putting that
  // same chunk back at that offset after it keeps the eye on the same
  // words while windows land above or below — where the old
  // scroll-height delta broke whenever anything over the log changed
  // height (the Earlier button, the title) or the reader scrolled
  // while a read was out.
  function captureAnchor(): { id: string; offset: number } | null {
    const area = scroller();
    const areaTop = area.getBoundingClientRect().top;
    const geometry = [...deps.el("transcripts-log").querySelectorAll("[data-transcript-message]")]
      .map((item) => {
        const rect = item.getBoundingClientRect();
        return {
          id: (item as HTMLElement).dataset.transcriptMessage || "",
          top: rect.top - areaTop + area.scrollTop,
          bottom: rect.bottom - areaTop + area.scrollTop,
        };
      });
    return pickAnchor(geometry, area.scrollTop);
  }

  function restoreAnchor(anchor: { id: string; offset: number }): void {
    const area = scroller();
    const target = deps.el("transcripts-log")
      .querySelector(`[data-transcript-message="${CSS.escape(anchor.id)}"]`);
    if (!target) return;
    area.scrollTop += target.getBoundingClientRect().top - area.getBoundingClientRect().top - anchor.offset;
  }

  /** Where the reader is now, as a reload restores it (`#2153`). */
  function currentPlace(): TranscriptPlace | null {
    if (!active) return null;
    if (!openRow) return { channel: "", thread: "", anchor: null, atNewest: false, offset: 0 };
    const newest = atBottom();
    const anchor = captureAnchor();
    return {
      channel: String(openRow.channel.id),
      thread: openRow.thread_id,
      anchor: newest && chunks.length > 0 ? String(chunks[chunks.length - 1]!.id) : anchor ? anchor.id : null,
      atNewest: newest,
      offset: anchor ? anchor.offset : 0,
    };
  }

  function notePlace(): void {
    if (deps.notePlace) deps.notePlace(currentPlace());
  }

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
    // `#2153`: a redraw keeps the reader's chunk at its offset, unless
    // a search hit is being landed (it centres, below).
    const anchor = highlightId === "" ? captureAnchor() : null;
    const log = deps.el("transcripts-log");
    const query = (deps.el("transcripts-search") as HTMLInputElement).value || "";
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
          // The regular message card (`#1886` part C, owner thread
          // 213): the rows' own anatomy - author and time on the meta
          // line, the full words in the body, and the thread's chip
          // and actions at hand. The chip and Reply go to the message's
          // thread in the channel view; Done folds the card back.
          item.classList.add("discord-message");
          const meta = document.createElement("div");
          meta.className = "meta";
          const authorEl = document.createElement("span");
          authorEl.className = "msg-author";
          authorEl.textContent = String(chunk.author || "");
          const timeEl = document.createElement("span");
          timeEl.className = "msg-time";
          timeEl.textContent = chunkWhen(chunk);
          meta.append(authorEl, timeEl);
          body.textContent = chunk.content;
          const actions = document.createElement("div");
          actions.className = "transcript-sent-actions";
          const threadId = chunk.thread && chunk.thread.id ? String(chunk.thread.id) : "";
          const chip = document.createElement("button");
          chip.setAttribute("type", "button");
          chip.className = "chip";
          chip.textContent = "Thread";
          chip.title = "Open this message's thread";
          const reply = document.createElement("button");
          reply.setAttribute("type", "button");
          reply.textContent = "Reply";
          const done = document.createElement("button");
          done.setAttribute("type", "button");
          done.textContent = "Done";
          const goThread = (event: Event): void => {
            event.stopPropagation();
            if (threadId !== "" && deps.openSentThread) deps.openSentThread(chunk);
          };
          chip.addEventListener("click", goThread);
          reply.addEventListener("click", goThread);
          done.addEventListener("click", (event) => {
            event.stopPropagation();
            expandedSent.delete(String(chunk.id));
            renderDay();
          });
          actions.append(chip, reply, done);
          item.append(meta, body, actions);
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
    } else if (anchor) {
      restoreAnchor(anchor);
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
    const gen = ++generation;
    const answer = await readWindow(row, null);
    if (gen !== generation) return;
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
      if (gen !== generation) return;
      chunks = mergeChunks(chunks, older.messages || []);
      nextBefore = older.next_before ?? null;
      hasMore = older.has_more === true && nextBefore !== null;
    }
    renderDay();
    if (atMessage === "") toBottom();
    notePlace();
    startFollow();
  }

  async function loadOlder(): Promise<void> {
    if (!openRow || !hasMore || loadingOlder || nextBefore === null) return;
    loadingOlder = true;
    const gen = generation;
    renderDay();
    try {
      const answer = await readWindow(openRow, nextBefore);
      // Left the view (or the day) while this read was out: it counts
      // for nothing here (addendum-1 (b)).
      if (gen !== generation) return;
      chunks = mergeChunks(chunks, answer.messages || []);
      nextBefore = answer.next_before ?? null;
      hasMore = answer.has_more === true && nextBefore !== null;
    } finally {
      loadingOlder = false;
      // renderDay's anchor keep is measured as the new window lands,
      // so a reader who scrolled while the read was out keeps the
      // place they scrolled to, not the one the read started from.
      if (gen === generation) {
        renderDay();
        notePlace();
      }
    }
  }

  /** Bring an open day up to its newest chunk, where it is followed. */
  async function followNew(): Promise<void> {
    if (!active || !openRow) return;
    const gen = generation;
    const answer = await readWindow(openRow, null);
    if (gen !== generation) return;
    const merged = mergeChunks(chunks, answer.messages || []);
    if (merged.length === chunks.length && chunks.every((chunk, at) => chunk.id === merged[at]?.id)) return;
    chunks = merged;
    // `#2153`: pinned is decided as the new chunks land, not from a
    // scroll event the redraw itself fired: a reader at the bottom
    // stays pinned to the new foot; anyone else keeps their chunk.
    const pinned = atBottom();
    renderDay();
    if (pinned) toBottom();
    notePlace();
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
    notePlace();
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
      notePlace();
    },
    close() {
      active = false;
      generation += 1;
      panel().hidden = true;
      stopFollow?.();
      stopFollow = null;
      notePlace();
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
      generation += 1;
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
      notePlace();
    },
    loadOlder,
    place: () => currentPlace(),
    async restore(place) {
      // A reload's way back (`#2153`): the index first (it says whether
      // the day is still there), then the day at its saved chunk —
      // stepping back a window at a time until the chunk is held, as
      // a search hit does, then putting the chunk at its saved offset
      // instead of centring it.
      await this.open();
      if (!place || place.thread === "") return;
      const row = rows.find(
        (one) => String(one.channel.id) === place.channel && one.thread_id === place.thread,
      );
      if (!row) {
        if (deps.setStatus) deps.setStatus("That transcript is not in the local index yet.");
        return;
      }
      const gen = ++generation;
      let answer = await readWindow(row, null);
      if (gen !== generation) return;
      let held = sortChunks(answer.messages || []);
      let before = answer.next_before ?? null;
      let more = answer.has_more === true && before !== null;
      while (place.anchor && !chunkHeld(held, place.anchor) && more) {
        const older = await readWindow(row, before);
        if (gen !== generation) return;
        held = mergeChunks(held, older.messages || []);
        before = older.next_before ?? null;
        more = older.has_more === true && before !== null;
      }
      openRow = row;
      expandedSent.clear();
      chunks = held;
      nextBefore = before;
      hasMore = more;
      loadingOlder = false;
      following = place.atNewest;
      highlightId = "";
      renderDay();
      if (place.atNewest) toBottom();
      else if (place.anchor) restoreAnchor({ id: place.anchor, offset: place.offset });
      notePlace();
      startFollow();
    },
  };
}

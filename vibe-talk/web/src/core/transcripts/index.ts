// Agent transcripts (`#1608 agent-transcripts-timeline`, design
// 2026-11-03 Owner 101): the Transcripts view's own copy — the index of
// classed agent-day trace threads and the pure shaping the view draws.
// Transcripts never enter the raw feed and never get a thread name; here
// they are only ever `agent · day`.

/** One classed transcript thread, as `GET /api/v1/transcripts` serves it. */
export interface TranscriptThread {
  channel: { id: string };
  thread_id: string;
  agent: string;
  team?: string | null;
  day: string;
  source: string;
}

/** The row's words: `agent · day`, factual and stable — never a slug. */
export function transcriptLabel(row: TranscriptThread): string {
  return `${row.agent} · ${row.day}`;
}

/** Index order: newest day first, then agent, then thread id. */
export function sortTranscripts(rows: TranscriptThread[]): TranscriptThread[] {
  return [...rows].sort((a, b) =>
    b.day.localeCompare(a.day)
    || a.agent.localeCompare(b.agent)
    || a.thread_id.localeCompare(b.thread_id));
}

/** The scoped search inside Transcripts: agent, day, or team words. */
export function filterTranscripts(rows: TranscriptThread[], query: string): TranscriptThread[] {
  const words = query.toLowerCase().split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return rows;
  return rows.filter((row) => {
    const hay = `${row.agent} ${row.day} ${row.team || ""}`.toLowerCase();
    return words.every((word) => hay.includes(word));
  });
}

/**
 * The fixed trace-root title (`<agent> main session trace, YYYY-MM-DD ET`),
 * as the server classes it (`#1608`, design 2.1). A root whose whole text
 * is this is a transcript root even on a copy that carries no class.
 */
const TRACE_ROOT_TITLE = /^\S+ main session trace, \d{4}-\d{2}-\d{2} ET$/;

/** The parts of a message or summary this module reads. */
export interface TranscriptMark {
  content?: string;
  thread?: { class?: unknown; is_root?: boolean; id?: unknown } | null;
  class?: unknown;
}

/**
 * Whether a message belongs to an agent transcript thread (`#1886
 * vibe-talk-transcripts-view` part A): its thread record is classed, or it
 * is a root whose whole text is the fixed trace title. Such a message
 * never enters the raw feed's store, on any path that brings it — a page
 * read, the live stream, or the device's saved copy. Its home is the
 * Transcripts view, which reads it from the transcripts routes instead.
 */
export function isTranscriptMessage(message: TranscriptMark): boolean {
  if (!message || typeof message !== "object") return false;
  if (message.thread && message.thread.class) return true;
  if (message.thread && message.thread.is_root === true && typeof message.content === "string") {
    return TRACE_ROOT_TITLE.test(message.content.trim());
  }
  return false;
}

/** Whether a thread summary is a transcript thread (its class says so). */
export function isTranscriptSummary(summary: TranscriptMark): boolean {
  return Boolean(summary && typeof summary === "object" && summary.class);
}

/** Whether a chunk's text holds every word of `query` (the day search). */
export function chunkMatches(text: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const hay = text.toLowerCase();
  return words.every((word) => hay.includes(word));
}

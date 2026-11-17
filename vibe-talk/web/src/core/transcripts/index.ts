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

/** Whether a chunk's text holds every word of `query` (the day search). */
export function chunkMatches(text: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const hay = text.toLowerCase();
  return words.every((word) => hay.includes(word));
}

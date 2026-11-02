// What the lines between a list's rows say (`#249 search-highlight`, `#250 date-separators`, `#439
// search-unmatched-separators`; `#624 voice-page-modularity`, moved from the legacy script into
// present/rows). The decision only: given what each row is — shown or not, a collapsed run's head or
// member, its day, how many messages it stands for — which line stands above or below each shown row
// and what a collapsed run's line says of its days. Drawing the lines is view/channel's.

import { dayRangeWords } from "../time/index.ts";
import type { DayClock } from "../time/index.ts";
import { gapWords } from "./labels.ts";

/** One row of a list, as far as its lines read it. */
export interface SeamRow {
  /** Its part in a collapsed run (`data-read-run`): "head", "member", or null for none. */
  readonly role: string | null;
  /** Whether it is shown: not filtered out, not hidden as read. */
  readonly shown: boolean;
  /** Its day (`dayKeyOf`), or null: no readable time, or a gathered reply drawn out of time order. */
  readonly day: number | null;
  /** How many messages it stands for. */
  readonly count: number;
}

/** A line: a day (`kind` "day") or an unmatched gap ("gap"), and its words. */
export interface Seam {
  readonly kind: string;
  readonly words: string;
}

/** The lines a list wants, by the index of the row each stands on. */
export interface SeamPlan {
  /** What each shown row that is not a run's member has above and below it. */
  readonly wanted: Map<number, { above: Seam | null; below: Seam | null }>;
  /** What each collapsed run's line says of its days, by the index of its head ("" for nothing). */
  readonly runDays: Map<number, string>;
}

/**
 * The lines between `rows`. For each row shown, the days it brings — its own, and those of the
 * stretch hidden before it — later than the last day already said are said once, as one range: on
 * the stretch's line where there is one (an unmatched gap while `gaps`, a filter choosing rows, or a
 * collapsed run's own line), and otherwise as a day rule of its own above the row. A gap after the
 * last row shown is drawn below that row. `clock` is "today", read once a pass.
 */
function seamPlan(rows: readonly SeamRow[], gaps: boolean, clock: DayClock): SeamPlan {
  const wanted = new Map<number, { above: Seam | null; below: Seam | null }>();
  const runDays = new Map<number, string>();
  // The latest day already said, and the stretch since the last row shown: its messages, its days.
  let said: number | null = null;
  let hidden = 0;
  let hiddenDays: (number | null)[] = [];
  let last: number | null = null;
  // The days among `days` later than the last said, as `{from, to}`, or null.
  const fresh = (days: (number | null)[]): { from: number; to: number } | null => {
    const later = days.filter((day): day is number => day !== null && (said === null || day > said));
    return later.length ? { from: Math.min(...later), to: Math.max(...later) } : null;
  };
  for (let at = 0; at < rows.length; at += 1) {
    const row = rows[at] as SeamRow;
    // A row hidden in a collapsed run is said by its run's line.
    if (row.role === "member") continue;
    if (!row.shown) {
      hidden += row.count;
      hiddenDays.push(row.day);
      continue;
    }
    if (row.role === "head") {
      // The run's line says the days the run brings, from the first of them to its last.
      const days = [row.day];
      for (let next = at + 1; next < rows.length && (rows[next] as SeamRow).role === "member"; next += 1) {
        days.push((rows[next] as SeamRow).day);
      }
      const span = fresh([...hiddenDays, ...days]);
      runDays.set(at, span ? dayRangeWords(span.from, span.to, clock) : "");
      if (span) said = span.to;
      wanted.set(at, { above: null, below: null });
    } else {
      const own = row.day;
      const span = fresh([...hiddenDays, own]);
      // Up to the row's own day, which the line stands right above.
      const days = span ? dayRangeWords(span.from, own === null ? span.to : own, clock) : "";
      let above: Seam | null = null;
      if (gaps && hidden > 0) above = { kind: "gap", words: gapWords(hidden, days) };
      else if (days) above = { kind: "day", words: days };
      wanted.set(at, { above, below: null });
      if (span) said = Math.max(span.to, own === null ? span.to : own);
    }
    hidden = 0;
    hiddenDays = [];
    last = at;
  }
  if (last !== null && gaps && hidden > 0) {
    const span = fresh(hiddenDays);
    const lines = wanted.get(last);
    if (lines) lines.below = { kind: "gap", words: gapWords(hidden, span ? dayRangeWords(span.from, span.to, clock) : "") };
  }
  return { wanted, runDays };
}

// What the page and the tests use of it.
export { seamPlan };

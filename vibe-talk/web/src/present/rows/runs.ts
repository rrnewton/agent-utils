// Runs of read messages under Fold read (`#624 voice-page-modularity`, moved from the legacy script
// into present/rows): which rows of a list collapse into a run behind one line, and the runs the reader
// opened, remembered per list. Rows of any kind: the legacy script passes its drawn rows, and says
// which of them may join a run and what each stands for.

/**
 * How few rows a run of read messages may have and still collapse: two. A line in place of ONE row
 * saves nothing — the line is as tall as a short message — and would hide the reader's own words
 * between the two answers on either side of them, which is the commonest read message there is.
 */
const READ_RUN_MIN_ROWS = 2;

/** How many messages one peel of a folded run reveals from either edge. */
const READ_RUN_PEEL_SIZE = 5;

/** At or above this many still-folded messages a run offers its `(first 5)` / `(last 5)` peels. */
const READ_RUN_PEEL_MIN = 10;

/** Where a row goes under Fold read: the "head" its run's line is drawn on, or a "member", and how many messages the run stands for. */
interface RunPlace {
  role: string;
  count: number;
}

/** The runs of read messages, and the ones the reader opened (`createReadRuns`). */
interface ReadRuns {
  /** The ids of the messages in the runs the reader opened in the list `key` names, or undefined. */
  opened(key: string): ReadonlySet<string> | undefined;
  /** The reader opened a run: its messages `ids` in the list `key` names stay open as the run grows. */
  open(key: string, ids: Iterable<string>): void;
  /** Messages `ids` folded away one by one in the list `key` names: no longer held open. */
  close(key: string, ids: Iterable<string>): void;
  /** The ids peeled from a run's edges in the list `key` names, or undefined. */
  peeled(key: string): ReadonlySet<string> | undefined;
  /** The reader peeled a run's edge: its messages `ids` in the list `key` names stand revealed while the rest stays folded. */
  peel(key: string, ids: Iterable<string>): void;
  /** Forget the opened runs of the list `key` names: a thread entered is laid out afresh. */
  forget(key: string): void;
  /** Forget every opened run: the reader chose another read mode. */
  clear(): void;
  /**
   * Where each row of `rows`, a list's rows in order, goes under Fold read: every run of at least
   * `READ_RUN_MIN_ROWS` rows next to each other that `joins` is a run, its first row the "head" its line
   * is drawn on and the rest its "members", with how many messages it stands for; the last row never
   * joins one. A run with a message the reader opened in it, in the list `key` names (`viewKey`), stays
   * open, and what has joined it since is remembered as opened with it. The one rule of the runs, for
   * the channel's list (`applyReadRuns`) and for each thread unpacked under its card (`unpackBlock`,
   * `#407 unpack-row-parity`), which differ only in what may join one.
   *
   * `idsFor` says what a row stands for in a run, for remembering it opened: its messages' ids, or over the
   * Threads view's cards a card's thread (`cardRunIds`, `#432 read-mode-unify`).
   *
   * `forcedIds`: messages folded away one by one (`#480 fold-one-message-key`):
   * a run holding one collapses however short it is — one message is a run when the reader folded
   * it themselves — and the last row may join for one, where a read run never takes it.
   */
  plan<Row>(rows: readonly Row[], key: string, joins: (row: Row) => boolean, idsFor: (row: Row) => string[],
    forcedIds?: ReadonlySet<string>): Map<Row, RunPlace>;
}

/** The read runs' owner, with no run opened. */
function createReadRuns(): ReadRuns {
  /**
   * The runs of read messages the reader has opened, per view (`viewKey`), as the ids of the messages
   * in them. Ids, because the rows are rebuilt by every read; and a run with any opened message in it
   * stays open as it grows, so a message that joins it later is recorded too. Forgotten when the mode
   * changes, and a thread's when it is entered, since entering lays the thread out afresh.
   */
  const openedReadRuns = new Map<string, Set<string>>();
  const peeledReadRuns = new Map<string, Set<string>>();
  return {
    opened: (key) => openedReadRuns.get(key),
    open(key, ids) {
      const opened = openedReadRuns.get(key) || new Set<string>();
      for (const id of ids) opened.add(id);
      openedReadRuns.set(key, opened);
    },
    close(key, ids) {
      const opened = openedReadRuns.get(key);
      if (opened) for (const id of ids) opened.delete(id);
      const peeled = peeledReadRuns.get(key);
      if (peeled) for (const id of ids) peeled.delete(id);
    },
    peeled: (key) => peeledReadRuns.get(key),
    peel(key, ids) {
      const peeled = peeledReadRuns.get(key) || new Set<string>();
      for (const id of ids) peeled.add(id);
      peeledReadRuns.set(key, peeled);
    },
    forget(key) {
      openedReadRuns.delete(key);
      peeledReadRuns.delete(key);
    },
    clear() {
      openedReadRuns.clear();
      peeledReadRuns.clear();
    },
    plan<Row>(rows: readonly Row[], key: string, joins: (row: Row) => boolean, idsFor: (row: Row) => string[],
      forcedIds: ReadonlySet<string> = new Set<string>()): Map<Row, RunPlace> {
      const opened = openedReadRuns.get(key) || new Set<string>();
      const peeled = peeledReadRuns.get(key) || new Set<string>();
      const placed = new Map<Row, RunPlace>();
      let run: Row[] = [];
      const place = (part: Row[]): void => {
        const ids = part.flatMap(idsFor);
        part.forEach((row, index) => placed.set(row, { role: index === 0 ? "head" : "member", count: ids.length }));
      };
      const close = (): void => {
        const ids = run.flatMap(idsFor);
        const forced = ids.some((id) => forcedIds.has(id));
        if (run.length >= READ_RUN_MIN_ROWS && ids.some((id) => opened.has(id)) && !forced) {
          // Opened: it stays open as it grows, so what joined it since is remembered with it.
          for (const id of ids) opened.add(id);
        } else if (ids.some((id) => peeled.has(id)) && !ids.some((id) => opened.has(id))) {
          // Peeled from an edge: the peeled rows stand revealed and each still-folded stretch
          // between them folds as a run of its own, its count the messages it still stands for.
          let segment: Row[] = [];
          const flush = (): void => {
            if (segment.length === 0) return;
            const segIds = segment.flatMap(idsFor);
            if (segment.length >= READ_RUN_MIN_ROWS || segIds.some((id) => forcedIds.has(id))) place(segment);
            segment = [];
          };
          for (const row of run) {
            const rowIds = idsFor(row);
            if (rowIds.length > 0 && rowIds.every((id) => peeled.has(id))) flush();
            else segment.push(row);
          }
          flush();
        } else if (run.length >= READ_RUN_MIN_ROWS || forced) {
          if (forced && ids.some((id) => opened.has(id))) {
            // One folded away inside a run the reader opened: only its own stretch collapses, and
            // the rest of the run stays open as it was left.
            let part: Row[] = [];
            for (const row of run) {
              if (idsFor(row).some((id) => forcedIds.has(id))) part.push(row);
              else if (part.length > 0) {
                place(part);
                part = [];
              }
            }
            if (part.length > 0) place(part);
            for (const id of ids) if (!forcedIds.has(id)) opened.add(id);
          } else {
            place(run);
          }
        }
        run = [];
      };
      rows.forEach((row, index) => {
        const forced = idsFor(row).some((id) => forcedIds.has(id));
        if ((index < rows.length - 1 || forced) && joins(row)) run.push(row);
        else close();
      });
      close();
      if (opened.size > 0) openedReadRuns.set(key, opened);
      return placed;
    },
  };
}

export { READ_RUN_MIN_ROWS, READ_RUN_PEEL_MIN, READ_RUN_PEEL_SIZE, createReadRuns };
export type { ReadRuns, RunPlace };

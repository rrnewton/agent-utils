// Which column the code is working on (`#229 desktop-two-column`; `#624 voice-page-modularity`, moved from the
// legacy script into view/channel): the left one, except inside `inColumn(columns.side, ...)`, which makes the
// right column the one worked on, runs, and puts the left column back before it returns, so nothing is left
// pointing at the right column between one event and the next. Each column says whether it has been worked
// on and left since it was last made afresh (`left`). The columns and their records are the page's.

/** A column, as far as working on it reads it. */
export interface WorkedColumn {
  /** Whether it has been worked on and left since it was last made afresh. */
  left: boolean;
}

/** The working column's commands and questions. */
export interface ColumnWork<C extends WorkedColumn> {
  /** The column being worked on now. */
  active(): C;
  /** Whether that is the right column (for `el`, which asks on every call). */
  side(): boolean;
  /**
   * Work on column `column` from now on: its records are the ones the page reaches. The column put aside has
   * been worked on and `left`; the one taken up has not.
   */
  useColumn(column: C): void;
  /**
   * Run `fn` working on column `column`, and work on the one before again however it returns. What `fn`
   * returns is returned: a promise from an `async` function, whose work after its first wait is no longer in here.
   */
  inColumn<T>(column: C, fn: () => T): T;
}

/** Working on the left column. */
export function createColumnWork<C extends WorkedColumn>(columns: { readonly main: C; readonly side: C }): ColumnWork<C> {
  /** The column the code is working on now. The left one, except inside `inColumn`. */
  let activeColumn: C = columns.main;
  /** Whether that is the right column. */
  let sideActive = false;

  function useColumn(column: C): void {
    if (column === activeColumn) return;
    activeColumn.left = true;
    column.left = false;
    activeColumn = column;
    sideActive = column === columns.side;
  }

  return {
    active: () => activeColumn,
    side: () => sideActive,
    useColumn,
    inColumn(column, fn) {
      const was = activeColumn;
      if (column === was) return fn();
      useColumn(column);
      try {
        return fn();
      } finally {
        useColumn(was);
      }
    },
  };
}
